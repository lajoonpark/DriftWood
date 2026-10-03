use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use tauri::Emitter;

use driftwood_core::scan::scope::resolve_roots;
use driftwood_core::types::{PrivacyTier, ScopeCategory};
use driftwood_core::{
    apply_correction, load_rules, run_scan, AdjudicationRequest, CorrectionRequest, DriftError,
    DriftTuning, EventSink, Report, ScanConfig, ScanEvent, ScanHandle, Tier,
};

/// Probe a Full Disk Access–protected folder. Without FDA, listing a
/// TCC-protected directory (Safari, Mail, Messages) fails with EPERM;
/// with FDA it succeeds. Metadata (`stat`) alone is not enough — opening
/// the directory is what TCC gates.
#[tauri::command]
fn check_full_disk_access() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    for rel in ["Library/Safari", "Library/Mail", "Library/Messages"] {
        let dir = PathBuf::from(&home).join(rel);
        if !dir.is_dir() {
            continue;
        }
        return match std::fs::read_dir(&dir) {
            Ok(_) => "granted".into(),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => "denied".into(),
            Err(_) => continue,
        };
    }
    "unknown".into()
}

/// Deep-link to the Full Disk Access pane.
#[tauri::command]
fn open_system_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFilesAccess")
        .status();
}

/// Reveal a path in Finder (`open -R`).
#[tauri::command]
fn reveal_path(path: String) {
    let _ = std::process::Command::new("open").arg("-R").arg(&path).status();
}

// ==================== Finder hand-off ====================
//
// The hand-off turns a selection of findings into Finder windows — one per
// container folder, each window's selection preselected to exactly the
// findings inside it — so the user can press ⌘Delete there themselves.
//
// The unit of the hand-off is the containing folder, not the leaf: a
// 200-item Caches finding set is one folder, and the number of windows is
// the number of ⌘Delete round-trips the user has to make. Grouping by
// container turns hundreds of leaves into a handful of windows.
//
// Read-only, always: the only filesystem access here is existence checks
// and one cheap `read_dir` per container for the findings-versus-total
// ratio. Nothing on any scanned path is ever written, moved, or removed —
// this code drives Finder's windows and nothing else.

/// One selected finding as sent by the frontend. Tier and personal-risk
/// scope travel with the path so a container can be judged by what it
/// aggregates, not just by the leaves inside it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct HandoffItem {
    path: String,
    size_bytes: u64,
    tier: u8,
    scope: String,
}

/// A path that could not be handed off, and why. Nothing is ever dropped
/// silently: every input path ends up in a group or in this list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct SkippedPath {
    path: String,
    reason: String,
}

/// One container folder the hand-off opens, with what Finder actually
/// reported as selected. `selected` is a Finder readback, not our request —
/// a mismatch is surfaced, never hidden, and `ok` comes only from that
/// readback, never from the mere absence of an error.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct RevealGroup {
    folder: String,
    /// Driftwood findings in this container.
    findings: usize,
    /// Reclaimable bytes of those findings. Never the folder's own size —
    /// the two figures are labeled separately everywhere in the UI.
    finding_bytes: u64,
    /// Directory entry count of the container, or null when it cannot be
    /// counted cheaply (permissions, or too large to walk). The UI renders
    /// that honestly as "total unknown"; null is never treated as zero.
    total_in_folder: Option<u64>,
    /// What we asked Finder to select.
    requested: usize,
    /// What Finder reported back as selected — may differ from `requested`.
    selected: usize,
    /// Contains a Source-tier or high-personal-risk finding. Such a
    /// container is never merged or promoted by rollup, and the UI warns.
    tainted: bool,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// What a hand-off actually did, Finder-verified.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct RevealSummary {
    groups: Vec<RevealGroup>,
    skipped: Vec<SkippedPath>,
    windows: usize,
    items_requested: usize,
    /// Finder-verified total across all groups.
    items_selected: usize,
}

/// The pre-commit half of the hand-off: the exact grouping and rollup the
/// execution will do, plus folder entry counts, with no Finder involvement.
/// The UI shows this before the user commits.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct PlanGroup {
    folder: String,
    findings: usize,
    finding_bytes: u64,
    total_in_folder: Option<u64>,
    tainted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct HandoffPlan {
    groups: Vec<PlanGroup>,
    skipped: Vec<SkippedPath>,
    windows: usize,
}

/// Why a hand-off failed outright. `automation_denied` is macOS's TCC
/// refusal of Finder control (Apple event error -1743); it gets its own
/// calm explanation in the UI, never a raw osascript error on a dead button.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RevealError {
    AutomationDenied,
    Script { message: String },
}

/// A finding is too personal to fold into a broader container: Source tier,
/// high personal risk, or an unknown risk stamp. Conservative by design —
/// an item we cannot classify gets the warning, never the benefit of the
/// doubt.
fn is_tainted(tier: u8, scope: &str) -> bool {
    tier >= 4 || (scope != "low" && scope != "medium")
}

/// Component-boundary proper-ancestor test on paths as scanned.
/// `Path::starts_with` compares whole components, so `/foo/barbaz` is never
/// mistaken for something inside `/foo/bar`. Paths stay lexical as scanned —
/// no canonicalization: `/tmp` is a symlink and Finder may display the
/// resolved target, but grouping must stay in the user's vocabulary.
fn is_proper_ancestor(anc: &Path, desc: &Path) -> bool {
    anc != desc && desc.starts_with(anc)
}

/// A container folder with the findings that live in it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HandoffGroup {
    folder: PathBuf,
    items: Vec<HandoffItem>,
    tainted: bool,
}

impl HandoffGroup {
    fn finding_bytes(&self) -> u64 {
        self.items.iter().map(|i| i.size_bytes).sum()
    }
}

/// Byte-descending, path-ascending: deterministic, and the first windows
/// cover the majority of the reclaimable space so the user can stop early.
fn sort_groups(groups: &mut [HandoffGroup]) {
    groups.sort_by(|a, b| {
        b.finding_bytes()
            .cmp(&a.finding_bytes())
            .then_with(|| a.folder.cmp(&b.folder))
    });
}

/// Collapse findings into container folders: group by parent directory,
/// then fold any parent that is itself inside another parent into its
/// ancestor — the ancestor's window already contains it. Every input path
/// is accounted for: unparentable paths (the filesystem root itself) come
/// back as skipped with a reason.
fn group_items(items: Vec<HandoffItem>) -> (Vec<HandoffGroup>, Vec<SkippedPath>) {
    let mut by_parent: BTreeMap<PathBuf, HandoffGroup> = BTreeMap::new();
    let mut skipped: Vec<SkippedPath> = Vec::new();
    for item in items {
        let parent = Path::new(&item.path).parent();
        match parent {
            Some(p) if !p.as_os_str().is_empty() => {
                let group = by_parent.entry(p.to_path_buf()).or_insert_with(|| HandoffGroup {
                    folder: p.to_path_buf(),
                    items: Vec::new(),
                    tainted: false,
                });
                group.tainted |= is_tainted(item.tier, &item.scope);
                group.items.push(item);
            }
            _ => skipped.push(SkippedPath {
                path: item.path,
                reason: "the path has no containing folder to open".into(),
            }),
        }
    }

    // Fold descendants into ancestors until stable: a chain a/b/c over the
    // set {a, a/b} folds c into b, then b into a.
    loop {
        let folders: Vec<PathBuf> = by_parent.keys().cloned().collect();
        let mut fold: Option<(PathBuf, PathBuf)> = None;
        'outer: for folder in &folders {
            for other in &folders {
                if other != folder && is_proper_ancestor(other, folder) {
                    fold = Some((folder.clone(), other.clone()));
                    break 'outer;
                }
            }
        }
        let Some((from, into)) = fold else { break };
        let Some(mut folded) = by_parent.remove(&from) else { break };
        if let Some(target) = by_parent.get_mut(&into) {
            target.tainted |= folded.tainted;
            target.items.append(&mut folded.items);
        }
    }

    let mut groups: Vec<HandoffGroup> = by_parent.into_values().collect();
    sort_groups(&mut groups);
    (groups, skipped)
}

/// Rollup target: with container grouping a hand-off needs only a handful
/// of windows. More than this many distinct containers means folders should
/// merge upward — when the guards allow it.
const ROLLUP_TARGET: usize = 12;

/// Merge container folders upward into scanned roots until at most `target`
/// remain. At each step the candidate ancestor covering the most
/// reclaimable bytes wins (ties broken by shorter path, then lexicographic),
/// so the same report always produces the same containers.
///
/// Hard guards, in order:
/// - never to the filesystem root, `$HOME`, or any ancestor outside
///   `allowed` — the directories the scan actually covers;
/// - never across a taint boundary: a container holding any Source-tier or
///   high-personal-risk finding stays exactly as it is, so a broad
///   "clear this" folder can never absorb it;
/// - never into something that is itself a finding.
fn rollup_groups(
    mut groups: Vec<HandoffGroup>,
    allowed: &HashSet<PathBuf>,
    finding_paths: &HashSet<String>,
    target: usize,
) -> Vec<HandoffGroup> {
    while groups.len() > target {
        let mut best: Option<(u64, PathBuf)> = None;
        for candidate in allowed {
            // A container must never be rolled up into a finding.
            if finding_paths.contains(&candidate.to_string_lossy().to_string()) {
                continue;
            }
            let mut covers = 0u64;
            let mut merges_any = false;
            let mut tainted_hit = false;
            for g in &groups {
                if g.folder == *candidate {
                    if g.tainted {
                        tainted_hit = true;
                    }
                    covers += g.finding_bytes();
                } else if is_proper_ancestor(candidate, &g.folder) {
                    if g.tainted {
                        tainted_hit = true;
                    }
                    covers += g.finding_bytes();
                    merges_any = true;
                }
            }
            if tainted_hit || !merges_any {
                continue;
            }
            let better = match &best {
                None => true,
                Some((bytes, path)) => {
                    covers > *bytes
                        || (covers == *bytes
                            && (candidate.components().count() < path.components().count()
                                || (candidate.components().count() == path.components().count()
                                    && candidate < path)))
                }
            };
            if better {
                best = Some((covers, candidate.clone()));
            }
        }
        let Some((_, ancestor)) = best else { break };

        // Merge everything at or under the ancestor into one container.
        let mut merged = HandoffGroup {
            folder: ancestor.clone(),
            items: Vec::new(),
            tainted: false,
        };
        let mut kept = Vec::with_capacity(groups.len());
        for mut g in groups.drain(..) {
            if g.folder == ancestor || is_proper_ancestor(&ancestor, &g.folder) {
                merged.tainted |= g.tainted;
                merged.items.append(&mut g.items);
            } else {
                kept.push(g);
            }
        }
        groups = kept;
        groups.push(merged);
        sort_groups(&mut groups);
    }
    groups
}

/// The directories rollup may merge into: the scan roots themselves. The
/// report does not record which categories a scan used, so the union of
/// all categories is allowed — but rollup can only ever move a container up
/// into an ancestor that already contains it, so an unscanned root is
/// unreachable in practice. `$HOME` and the filesystem root are removed
/// explicitly: rolling the whole home directory into one window is exactly
/// the "clear this" blanket this design must never suggest.
fn allowed_ancestors_for(home: &Path) -> HashSet<PathBuf> {
    let categories = [ScopeCategory::Low, ScopeCategory::Medium, ScopeCategory::High];
    let mut set: HashSet<PathBuf> = resolve_roots(&categories, home)
        .into_iter()
        .map(|r| r.path)
        .collect();
    set.remove(home);
    set.remove(Path::new("/"));
    set.retain(|p| p.is_dir());
    set
}

/// Entry count of a directory, or None when it cannot be known cheaply.
/// A few tens of thousands of entries is the honest ceiling — past it the
/// count reports as unknown rather than blocking the UI on a full walk.
/// Permission trouble reads as None too: an honest "total unknown" never
/// becomes a wrong number, and null must never be read as a ratio of zero.
const DIR_COUNT_CAP: usize = 50_000;

fn count_dir_entries(path: &Path) -> Option<u64> {
    count_dir_entries_capped(path, DIR_COUNT_CAP)
}

fn count_dir_entries_capped(path: &Path, cap: usize) -> Option<u64> {
    if cap == 0 {
        return None;
    }
    let read = std::fs::read_dir(path).ok()?;
    let mut n: u64 = 0;
    for entry in read {
        let _ = entry.ok()?;
        n += 1;
        if n as usize >= cap {
            return None;
        }
    }
    Some(n)
}

/// Existence check between scan and hand-off. Findings vanish constantly —
/// caches get cleared by the very apps that own them — so a miss is a
/// per-path skip with a reason, never a hard failure of the whole hand-off.
fn check_exists(path: &str) -> Result<(), String> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err("vanished since the scan — already gone from disk".into())
        }
        Err(e) => Err(format!("not readable: {e}")),
    }
}

/// Escape a path for embedding in an AppleScript string literal. Paths with
/// quotes, backslashes, newlines, emoji and non-ASCII must survive the
/// round trip intact.
fn apple_script_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

/// Aliases per list-construction step. The final `select` is still one call
/// per window; chunking only keeps each script statement bounded.
const SELECTION_CHUNK: usize = 100;

/// Build the AppleScript program for a hand-off. One window per container,
/// its selection set to the exact list of findings, every count read back
/// from Finder, and only then is the first window activated.
///
/// - Windows are the object references `make` returns — never indices.
/// - Finder windows have no per-window selection property in their
///   dictionary; `select` sets the frontmost window's selection, and the
///   readback is `count of (get selection)` immediately after — the window
///   `select` just revealed. The verified number is the truth.
/// - Aliases are coerced one item at a time, each in its own try: a path
///   that vanishes in the race between the existence check and this script
///   misses alone (`i:MISS:<path>`) instead of failing its whole group's
///   selection — findings disappear constantly, and one of four hundred
///   going must never cost the user the other three hundred ninety-nine.
/// - Each group reports one result line: `i:OK:n`, or `i:ERRWIN:num:msg`
///   (window could not be opened) or `i:ERRSEL:num:msg` (window opened,
///   selection failed). Failure of one group never stops the others.
/// - Finder is never activated until every window exists, so the user does
///   not watch focus jump while the hand-off assembles.
fn build_handoff_script(groups: &[HandoffGroup], chunk: usize) -> String {
    debug_assert!(chunk >= 1);
    let mut s = String::new();
    // Flatten error messages so a newline inside a Finder error string can
    // never corrupt the result lines this program returns.
    s.push_str("on flatten(t)\n");
    s.push_str("\tset AppleScript's text item delimiters to linefeed\n");
    s.push_str("\tset parts to text items of t\n");
    s.push_str("\tset AppleScript's text item delimiters to space\n");
    s.push_str("\tset flat to parts as string\n");
    s.push_str("\tset AppleScript's text item delimiters to tab\n");
    s.push_str("\tset parts to text items of flat\n");
    s.push_str("\tset AppleScript's text item delimiters to space\n");
    s.push_str("\tset flat to parts as string\n");
    s.push_str("\tset AppleScript's text item delimiters to {\"\"}\n");
    s.push_str("\treturn flat\n");
    s.push_str("end flatten\n");
    s.push_str("set out to {}\n");
    s.push_str("set wFirst to missing value\n");
    s.push_str("tell application \"Finder\"\n");
    for (i, group) in groups.iter().enumerate() {
        let folder = apple_script_escape(&group.folder.to_string_lossy());
        s.push_str(&format!("\tset w{i} to missing value\n"));
        s.push_str("\ttry\n");
        s.push_str(&format!("\t\tset w{i} to make new Finder window\n"));
        s.push_str(&format!(
            "\t\tset target of w{i} to (POSIX file \"{folder}\" as alias)\n"
        ));
        s.push_str("\ton error errMsg number errNum\n");
        s.push_str(&format!(
            "\t\tset end of out to \"{i}:ERRWIN:\" & errNum & \":\" & my flatten(errMsg)\n"
        ));
        s.push_str("\tend try\n");
        s.push_str(&format!("\tif w{i} is not missing value then\n"));
        s.push_str(&format!("\t\tset s{i} to {{}}\n"));
        for batch in group.items.chunks(chunk) {
            let paths: Vec<String> = batch
                .iter()
                .map(|it| format!("\"{}\"", apple_script_escape(&it.path)))
                .collect();
            s.push_str(&format!(
                "\t\trepeat with p in {{{}}}\n",
                paths.join(", ")
            ));
            s.push_str("\t\t\ttry\n");
            s.push_str(&format!(
                "\t\t\t\tset end of s{i} to (POSIX file (contents of p) as alias)\n"
            ));
            s.push_str("\t\t\ton error\n");
            s.push_str(&format!(
                "\t\t\t\tset end of out to \"{i}:MISS:\" & my flatten(contents of p)\n"
            ));
            s.push_str("\t\t\tend try\n");
            s.push_str("\t\tend repeat\n");
        }
        s.push_str("\t\ttry\n");
        s.push_str(&format!(
            "\t\t\tif (count of s{i}) > 0 then\n\t\t\t\tselect s{i}\n\t\t\t\tset n{i} to count of (get selection)\n\t\t\telse\n\t\t\t\tset n{i} to 0\n\t\t\tend if\n"
        ));
        s.push_str(&format!("\t\t\tset end of out to \"{i}:OK:\" & n{i}\n"));
        s.push_str(&format!(
            "\t\t\tif wFirst is missing value then set wFirst to w{i}\n"
        ));
        s.push_str("\t\ton error errMsg number errNum\n");
        s.push_str(&format!(
            "\t\t\tset end of out to \"{i}:ERRSEL:\" & errNum & \":\" & my flatten(errMsg)\n"
        ));
        s.push_str("\t\tend try\n");
        s.push_str("\tend if\n");
    }
    s.push_str("\tif wFirst is not missing value then\n");
    s.push_str("\t\ttry\n");
    s.push_str("\t\t\tset index of wFirst to 1\n");
    s.push_str("\t\tend try\n");
    s.push_str("\t\ttry\n");
    s.push_str("\t\t\tactivate wFirst\n");
    s.push_str("\t\tend try\n");
    s.push_str("\tend if\n");
    s.push_str("end tell\n");
    s.push_str("set AppleScript's text item delimiters to linefeed\n");
    s.push_str("return out as string\n");
    s
}

/// One parsed per-group result from the AppleScript run.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GroupOutcome {
    Ok(usize),
    ErrWin { num: i32, message: String },
    ErrSel { num: i32, message: String },
}

/// The full result of one script run: per-group outcomes plus every path
/// that went missing mid-flight (reported per item, per group).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ScriptResult {
    outcomes: Vec<Option<GroupOutcome>>,
    missed: Vec<(usize, String)>,
}

/// Finder error strings are echoed into the summary; keep them one-line,
/// bounded, and free of control characters.
fn sanitize_error(msg: &str) -> String {
    msg.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(160)
        .collect()
}

fn parse_script_output(stdout: &str, group_count: usize) -> ScriptResult {
    let mut result = ScriptResult {
        outcomes: vec![None; group_count],
        missed: Vec::new(),
    };
    for line in stdout.lines() {
        let Some((idx, rest)) = line.split_once(':') else {
            continue;
        };
        let Ok(idx) = idx.parse::<usize>() else {
            continue;
        };
        if idx >= group_count {
            continue;
        }
        let parsed = if let Some(n) = rest.strip_prefix("OK:") {
            n.trim().parse::<usize>().ok().map(GroupOutcome::Ok)
        } else if let Some(nm) = rest.strip_prefix("ERRWIN:") {
            err_parts(nm).map(|(num, message)| GroupOutcome::ErrWin { num, message })
        } else if let Some(nm) = rest.strip_prefix("ERRSEL:") {
            err_parts(nm).map(|(num, message)| GroupOutcome::ErrSel { num, message })
        } else if let Some(missed) = rest.strip_prefix("MISS:") {
            result.missed.push((idx, missed.to_string()));
            None
        } else {
            None
        };
        if let Some(outcome) = parsed {
            result.outcomes[idx] = Some(outcome);
        }
    }
    result
}

fn err_parts(rest: &str) -> Option<(i32, String)> {
    let (num, message) = rest.split_once(':')?;
    let num = num.trim().parse::<i32>().ok()?;
    Some((num, sanitize_error(message)))
}

/// Feed the script to `osascript` on stdin — never argv, a hand-off can
/// carry hundreds of paths — and capture what it reported. No custom event
/// timeout: macOS holds the first event while its consent dialog waits for
/// the user, and that wait should end in a hand-off, not an error.
fn run_osascript(script: &str) -> Result<(String, String), RevealError> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut child = Command::new("osascript")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| RevealError::Script {
            message: sanitize_error(&format!("could not start osascript: {e}")),
        })?;
    {
        let mut stdin = child.stdin.take().expect("osascript stdin was piped");
        stdin.write_all(script.as_bytes()).map_err(|e| RevealError::Script {
            message: sanitize_error(&format!("could not send the script to osascript: {e}")),
        })?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| RevealError::Script {
            message: sanitize_error(&format!("osascript did not finish: {e}")),
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    // A per-group try catches Apple event failures, so a nonzero exit means
    // the program itself never ran (syntax, spawn trouble) — surface it.
    if !output.status.success()
        && !stdout.lines().any(|l| {
            l.contains(":OK:") || l.contains(":ERRWIN:") || l.contains(":ERRSEL:")
        })
    {
        let message = sanitize_error(if stderr.trim().is_empty() {
            "osascript failed without a reason"
        } else {
            stderr.trim()
        });
        if message.contains("-1743") {
            return Err(RevealError::AutomationDenied);
        }
        return Err(RevealError::Script { message });
    }
    Ok((stdout, stderr))
}

/// The pre-commit view of a hand-off: containers, findings-versus-total,
/// taint, and every path that has already vanished — everything the user
/// should see before committing. Reads directories (one cheap count each)
/// and touches nothing else.
#[tauri::command]
async fn plan_handoff(items: Vec<HandoffItem>) -> Result<HandoffPlan, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<HandoffPlan, String> {
        Ok(plan_handoff_blocking(items))
    })
    .await
    .map_err(|e| format!("planning task failed: {e}"))?
}

fn plan_handoff_blocking(items: Vec<HandoffItem>) -> HandoffPlan {
    let (groups, skipped) = plan_groups(items);
    let windows = groups.len();
    HandoffPlan {
        groups: groups
            .into_iter()
            .map(|g| {
                let total_in_folder = count_dir_entries(&g.folder);
                PlanGroup {
                    folder: g.folder.to_string_lossy().into_owned(),
                    findings: g.items.len(),
                    finding_bytes: g.finding_bytes(),
                    total_in_folder,
                    tainted: g.tainted,
                }
            })
            .collect(),
        skipped,
        windows,
    }
}

/// Existence check, container grouping, descendant fold, and rollup — the
/// shared core of planning and execution, so the plan always describes what
/// the execution will do.
fn plan_groups(items: Vec<HandoffItem>) -> (Vec<HandoffGroup>, Vec<SkippedPath>) {
    let mut present = Vec::with_capacity(items.len());
    let mut skipped: Vec<SkippedPath> = Vec::new();
    for item in items {
        match check_exists(&item.path) {
            Ok(()) => present.push(item),
            Err(reason) => skipped.push(SkippedPath {
                path: item.path,
                reason,
            }),
        }
    }
    let (mut groups, mut unparented) = group_items(present);
    skipped.append(&mut unparented);
    let finding_paths: HashSet<String> = groups
        .iter()
        .flat_map(|g| g.items.iter().map(|i| i.path.clone()))
        .collect();
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let allowed = allowed_ancestors_for(&home);
    groups = rollup_groups(groups, &allowed, &finding_paths, ROLLUP_TARGET);
    (groups, skipped)
}

/// Hand a selection off to Finder: one window per container folder with the
/// findings preselected, verified counts read back from Finder, and every
/// skipped path accounted for with a reason. DriftWood touches nothing but
/// Finder's windows — the user presses ⌘Delete themselves.
///
/// `folders` restricts the hand-off to specific containers (the per-folder
/// actions pass exactly one); None hands off every container of the
/// selection. Grouping and rollup are recomputed here so execution always
/// agrees with the plan the user saw.
#[tauri::command]
async fn reveal_paths(
    items: Vec<HandoffItem>,
    folders: Option<Vec<String>>,
) -> Result<RevealSummary, RevealError> {
    tauri::async_runtime::spawn_blocking(move || reveal_paths_blocking(items, folders))
        .await
        .map_err(|e| RevealError::Script {
            message: sanitize_error(&format!("hand-off task failed: {e}")),
        })?
}

fn reveal_paths_blocking(
    items: Vec<HandoffItem>,
    folders: Option<Vec<String>>,
) -> Result<RevealSummary, RevealError> {
    let (mut groups, mut skipped) = plan_groups(items);

    // Folder restriction for the per-container actions. Items in containers
    // the user did not pick are reported, not dropped.
    if let Some(wanted) = &folders {
        let wanted: HashSet<&String> = wanted.iter().collect();
        let mut kept = Vec::new();
        for g in groups {
            if wanted.contains(&g.folder.to_string_lossy().to_string()) {
                kept.push(g);
            } else {
                for item in g.items {
                    skipped.push(SkippedPath {
                        path: item.path,
                        reason: "outside the folders you picked for this hand-off".into(),
                    });
                }
            }
        }
        groups = kept;
    }

    if groups.is_empty() {
        return Ok(RevealSummary {
            groups: Vec::new(),
            skipped,
            windows: 0,
            items_requested: 0,
            items_selected: 0,
        });
    }

    // Folder entry counts for the findings-versus-total ratio, taken now —
    // closer to the moment of deletion than the plan was.
    let totals: Vec<Option<u64>> = groups.iter().map(|g| count_dir_entries(&g.folder)).collect();

    let script = build_handoff_script(&groups, SELECTION_CHUNK);
    let (stdout, _stderr) = run_osascript(&script)?;
    let result = parse_script_output(&stdout, groups.len());

    // A TCC refusal reaches every group identically; nothing was handed
    // off, and the user needs the calm permission explanation, not a table
    // of identical failures.
    if result.outcomes.iter().any(|o| {
        matches!(
            o,
            Some(GroupOutcome::ErrWin { num: -1743, .. }) | Some(GroupOutcome::ErrSel { num: -1743, .. })
        )
    }) {
        return Err(RevealError::AutomationDenied);
    }

    // Paths that vanished in the race between the existence check and the
    // script run: per-item skips with a reason, not a failed group.
    let mut missed_by_group: Vec<usize> = vec![0; groups.len()];
    for (idx, path) in &result.missed {
        if let Some(g) = missed_by_group.get_mut(*idx) {
            *g += 1;
        }
        skipped.push(SkippedPath {
            path: sanitize_error(path),
            reason: "vanished during the hand-off — Finder could not select it".into(),
        });
    }

    let mut reveal_groups = Vec::with_capacity(groups.len());
    let mut windows = 0usize;
    let mut items_requested = 0usize;
    let mut items_selected = 0usize;
    for (i, g) in groups.iter().enumerate() {
        let requested = g.items.len();
        let mut opened = false;
        let (selected, ok, error) = match &result.outcomes[i] {
            // The verdict compares against what could still be selected:
            // a vanished path is accounted for, not a Finder failure.
            Some(GroupOutcome::Ok(n)) => (*n, *n == requested - missed_by_group[i], None),
            Some(GroupOutcome::ErrSel { num, message }) => {
                opened = true;
                (
                    0,
                    false,
                    Some(format!("Finder could not set the selection ({num}): {message}")),
                )
            }
            Some(GroupOutcome::ErrWin { num, message }) => (
                0,
                false,
                Some(format!("Finder could not open the window ({num}): {message}")),
            ),
            None => (
                0,
                false,
                Some("Finder returned no result for this folder".into()),
            ),
        };
        if opened {
            windows += 1;
        }
        items_requested += requested;
        items_selected += selected;
        reveal_groups.push(RevealGroup {
            folder: g.folder.to_string_lossy().into_owned(),
            findings: requested,
            finding_bytes: g.finding_bytes(),
            total_in_folder: totals[i],
            requested,
            selected,
            tainted: g.tainted,
            ok,
            error,
        });
    }

    Ok(RevealSummary {
        groups: reveal_groups,
        skipped,
        windows,
        items_requested,
        items_selected,
    })
}

/// Probe macOS's Automation consent for Finder control. The first probe
/// makes macOS show its consent dialog; a refusal answers -1743. This is a
/// string, not an enum, to mirror `check_full_disk_access`.
#[tauri::command]
async fn check_automation_permission() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        match probe_finder_automation() {
            Ok(o) if o.status.success() => "granted".to_string(),
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                if err.contains("-1743") {
                    "denied".to_string()
                } else {
                    "unknown".to_string()
                }
            }
            Err(_) => "unknown".to_string(),
        }
    })
    .await
    .map_err(|e| format!("probe task failed: {e}"))
}

/// One Apple event to Finder — enough for macOS to ask, and for us to know.
fn probe_finder_automation() -> std::io::Result<std::process::Output> {
    std::process::Command::new("osascript")
        .arg("-e")
        .arg("tell application \"Finder\" to get name")
        .output()
}

/// Deep-link to the Automation pane, where the Finder consent lives.
#[tauri::command]
fn open_automation_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Automation")
        .status();
}

/// Scan options as sent by the frontend. Anything the shell does not send
/// (rules, walk caps, weights) keeps the core defaults.
///
/// `mode` is the v1.4 shape (express / standard / deep_read, matching
/// core's snake_case `ScanMode`). The pre-1.4 `stage2` boolean is still
/// accepted and mapped (true → Standard, false → Express) so a stale
/// cached frontend bundle can never wedge the scan command again — that
/// exact mismatch was issue #2 ("invalid args `config` for command
/// `start_scan`: missing field `stage2`").
#[derive(Debug, Deserialize)]
struct ScanConfigDto {
    scopes: Vec<ScopeCategory>,
    privacy_tier: PrivacyTier,
    #[serde(default)]
    mode: Option<driftwood_core::ScanMode>,
    /// Legacy pre-1.4 field, kept only for tolerance of an older frontend.
    #[serde(default, skip_serializing)]
    stage2: Option<bool>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    cost_cap_usd: Option<f64>,
    /// Danger zone: when true, Stage-2 calls are not restricted to
    /// zero-data-retention providers. Opt-in only; default stays protected.
    #[serde(default)]
    allow_non_zdr: Option<bool>,
}

impl ScanConfigDto {
    /// Resolve how much of the river to run: an explicit `mode` wins; the
    /// legacy `stage2` boolean is the fallback; the default is Standard —
    /// the same default the settings store ships.
    fn resolved_mode(&self) -> driftwood_core::ScanMode {
        self.mode.or_else(|| {
            self.stage2
                .map(|b| if b { driftwood_core::ScanMode::Standard } else { driftwood_core::ScanMode::Express })
        }).unwrap_or(driftwood_core::ScanMode::Standard)
    }
}

/// Handle of the currently running scan, so `cancel_scan` can reach it.
#[derive(Default)]
struct ScanState {
    handle: Mutex<Option<ScanHandle>>,
}

/// Bridges core scan events to the `scan-event` window event the frontend
/// listens on.
struct TauriSink(tauri::AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, event: ScanEvent) {
        if let Err(e) = self.0.emit("scan-event", &event) {
            eprintln!("warning: could not emit scan event: {e}");
        }
    }
}

/// Run a scan. Emits progress on `scan-event`; resolves to the report in
/// the shape the frontend expects. A cancel during Stage 2 resolves to a
/// partial report (`stopped_early`, fallback-labeled remainder) instead of
/// discarding the work; only early-phase cancels resolve to "cancelled".
#[tauri::command]
async fn start_scan(
    app: tauri::AppHandle,
    state: tauri::State<'_, ScanState>,
    config: ScanConfigDto,
) -> Result<serde_json::Value, String> {
    let mut tuning = DriftTuning::default();
    if let Some(cap) = config.cost_cap_usd {
        if cap > 0.0 {
            tuning.reasoning.cost_cap_usd = cap;
        }
    }
    // Resolve the mode before any field of `config` is moved out.
    let mode = config.resolved_mode();

    // The key comes from the app settings (frontend) or the environment.
    let api_key = config
        .api_key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .or_else(|| {
            std::env::var("OPENROUTER_API_KEY")
                .ok()
                .filter(|k| !k.is_empty())
        });

    let core_config = ScanConfig {
        scopes: config.scopes,
        privacy_tier: config.privacy_tier,
        mode,
        model: config
            .model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| driftwood_core::default_model().to_string()),
        api_key,
        allow_non_zdr: config.allow_non_zdr.unwrap_or(false),
        rules: load_rules(None).unwrap_or_default(),
        tuning,
        persist: true,
    };

    let handle = ScanHandle::new();
    *state.handle.lock().expect("scan state poisoned") = Some(handle.clone());
    let sink: Arc<dyn EventSink> = Arc::new(TauriSink(app));
    let result = run_scan(core_config, sink, handle).await;
    *state.handle.lock().expect("scan state poisoned") = None;

    match result {
        Ok(report) => {
            let value = serde_json::to_value(&report).map_err(|e| e.to_string())?;
            Ok(report_to_frontend(value))
        }
        // Early-phase cancel (before anything worth reporting exists).
        Err(DriftError::Cancelled) => Err("cancelled".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Cancel the running scan. Stage 2 checks this inside the streaming loop,
/// so an in-flight request is aborted immediately; the scan then resolves
/// to a partial report rather than nothing.
#[tauri::command]
fn cancel_scan(state: tauri::State<'_, ScanState>) {
    if let Some(handle) = state.handle.lock().expect("scan state poisoned").as_ref() {
        handle.cancel();
    }
}

/// The last persisted report, or null when none exists yet.
#[tauri::command]
fn last_report() -> Result<Option<serde_json::Value>, String> {
    match Report::load_last() {
        Ok(report) => {
            let value = serde_json::to_value(&report).map_err(|e| e.to_string())?;
            Ok(Some(report_to_frontend(value)))
        }
        Err(_) => Ok(None),
    }
}

/// Record a user tier correction for a candidate in the last report. The
/// candidate details (path, size, orphan status, band) come from the
/// persisted report; the frontend only sends the opaque id.
#[tauri::command]
async fn correct_tier(id: String, tier: u8, note: Option<String>) -> Result<(), String> {
    let corrected = Tier::try_from(tier).map_err(|e| e.to_string())?;
    let report =
        Report::load_last().map_err(|e| format!("no report to correct against: {e}"))?;
    let entry = report
        .entries
        .iter()
        .find(|e| e.candidate.id == id)
        .ok_or_else(|| format!("unknown candidate id {id}"))?;

    let request = CorrectionRequest {
        path: entry.candidate.path.clone(),
        size_bytes: entry.candidate.size_bytes,
        orphan: entry.candidate.orphan_status,
        score_band: entry.candidate.band,
        original_tier: entry.tier,
        corrected_tier: corrected,
        note,
    };
    apply_correction(request, None)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Ask-the-river options sent by the frontend. The key resolution mirrors
/// `start_scan`: settings first, environment fallback.
#[derive(Debug, Deserialize)]
struct AdjudicateDto {
    id: String,
    model: Option<String>,
    api_key: Option<String>,
    allow_non_zdr: Option<bool>,
}

/// On-demand adjudication: a second opinion for ONE candidate of the last
/// report. The card's tier never changes; the river's opinion is returned
/// for display, and its cost is folded into the persisted report total so
/// the spend figure stays honest.
#[tauri::command]
async fn adjudicate_candidate(
    app: tauri::AppHandle,
    state: tauri::State<'_, ScanState>,
    request: AdjudicateDto,
) -> Result<serde_json::Value, String> {
    // The persisted report must not change under a running adjudication.
    if state.handle.lock().expect("scan state poisoned").is_some() {
        return Err("a scan is running — wait for it to finish before asking the river".into());
    }
    let api_key = request
        .api_key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .or_else(|| {
            std::env::var("OPENROUTER_API_KEY")
                .ok()
                .filter(|k| !k.is_empty())
        })
        .unwrap_or_default();
    // Adjudication travels under the scan's own privacy tier — the payload
    // redaction the user saw when the item was scanned is the redaction
    // they get when they ask about it.
    let privacy_tier = Report::load_last()
        .map(|r| r.privacy_tier_used)
        .unwrap_or(PrivacyTier::Standard);

    let core_request = AdjudicationRequest {
        candidate_id: request.id,
        model: request
            .model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| driftwood_core::default_model().to_string()),
        api_key,
        privacy_tier,
        enforce_zdr: !request.allow_non_zdr.unwrap_or(false),
    };

    let sink: Arc<dyn EventSink> = Arc::new(TauriSink(app));
    match driftwood_core::adjudicate_candidate(core_request, sink).await {
        Ok(result) => serde_json::to_value(&result).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}
/// Map the core report model onto the frontend's wire contract:
/// groups as `{category, count, bytes}`, warnings as plain strings,
/// `cost_cap` as a boolean, and `cache_loc` in score components.
fn report_to_frontend(mut report: serde_json::Value) -> serde_json::Value {
    let obj = match report.as_object_mut() {
        Some(o) => o,
        None => return report,
    };

    if let Some(groups) = obj.get_mut("groups").and_then(serde_json::Value::as_array_mut) {
        for g in groups {
            if let Some(gobj) = g.as_object_mut() {
                if let Some(n) = gobj.remove("item_count") {
                    gobj.insert("count".into(), n);
                }
                if let Some(b) = gobj.remove("total_bytes") {
                    gobj.insert("bytes".into(), b);
                }
                gobj.remove("label");
            }
        }
    }

    if let Some(warnings) = obj.get_mut("warnings").and_then(serde_json::Value::as_array_mut) {
        let messages: Vec<serde_json::Value> = warnings
            .iter()
            .filter_map(|w| w.get("message").cloned())
            .collect();
        obj.insert("warnings".into(), serde_json::Value::Array(messages));
    }

    if let Some(hit) = obj.remove("cost_cap_hit") {
        obj.insert("cost_cap".into(), hit);
    }

    if let Some(entries) = obj.get_mut("entries").and_then(serde_json::Value::as_array_mut) {
        for entry in entries {
            if let Some(components) = entry
                .get_mut("candidate")
                .and_then(|c| c.get_mut("score_components"))
                .and_then(serde_json::Value::as_object_mut)
            {
                if let Some(v) = components.remove("cache_location") {
                    components.insert("cache_loc".into(), v);
                }
            }
        }
    }

    report
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(ScanState::default())
        .invoke_handler(tauri::generate_handler![
            check_full_disk_access,
            open_system_settings,
            check_automation_permission,
            open_automation_settings,
            reveal_path,
            reveal_paths,
            plan_handoff,
            start_scan,
            cancel_scan,
            last_report,
            correct_tier,
            adjudicate_candidate
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Issue #2: the v1.4 frontend sends `mode` ("express" | "standard" |
    /// "deep_read"), not the old `stage2` boolean — the strict `stage2`
    /// field rejected every scan ("missing field `stage2`"). The DTO must
    /// accept the new shape, the legacy shape, and neither.
    #[test]
    fn scan_config_dto_accepts_mode_and_legacy_stage2() {
        let v: ScanConfigDto = serde_json::from_str(&format!(
            r#"{{"scopes":["low"],"privacy_tier":"standard","mode":"deep_read"}}"#
        ))
        .unwrap();
        assert!(matches!(v.resolved_mode(), driftwood_core::ScanMode::DeepRead));

        let v: ScanConfigDto = serde_json::from_str(&format!(
            r#"{{"scopes":["low"],"privacy_tier":"standard","mode":"express"}}"#
        ))
        .unwrap();
        assert!(matches!(v.resolved_mode(), driftwood_core::ScanMode::Express));

        // Legacy pre-1.4 shape still maps.
        let v: ScanConfigDto =
            serde_json::from_str(r#"{"scopes":["low"],"privacy_tier":"standard","stage2":false}"#)
                .unwrap();
        assert!(matches!(v.resolved_mode(), driftwood_core::ScanMode::Express));
        let v: ScanConfigDto =
            serde_json::from_str(r#"{"scopes":["low"],"privacy_tier":"standard","stage2":true}"#)
                .unwrap();
        assert!(matches!(v.resolved_mode(), driftwood_core::ScanMode::Standard));

        // Neither field → Standard, the settings-store default.
        let v: ScanConfigDto =
            serde_json::from_str(r#"{"scopes":["low"],"privacy_tier":"standard"}"#).unwrap();
        assert!(matches!(v.resolved_mode(), driftwood_core::ScanMode::Standard));
    }

    #[test]
    fn report_shape_matches_frontend_contract() {
        let report = json!({
            "schema_version": 2,
            "groups": [
                {"category": "low", "label": "Low personal risk", "item_count": 2, "total_bytes": 1000}
            ],
            "entries": [
                {"candidate": {"id": "c-1", "path": "/x", "score_components":
                    {"size": 1.0, "cache_location": 15.0}}},
            ],
            "warnings": [{"kind": "cost_cap", "message": "cap hit"}],
            "cost_cap_hit": true
        });
        let v = report_to_frontend(report);
        assert_eq!(v["groups"][0]["count"], 2);
        assert_eq!(v["groups"][0]["bytes"], 1000);
        assert!(v["groups"][0].get("label").is_none());
        assert_eq!(v["warnings"][0], "cap hit");
        assert_eq!(v["cost_cap"], true);
        assert!(v["cost_cap_hit"].is_null());
        assert_eq!(v["entries"][0]["candidate"]["score_components"]["cache_loc"], 15.0);
        assert!(v["entries"][0]["candidate"]["score_components"]
            .get("cache_location")
            .is_none());
    }

    /// Every TierSource the core can emit must survive the frontend mapping
    /// untouched — tier_source is the single honest provenance channel, and
    /// the contract test must not loosen as variants are added.
    #[test]
    fn tier_sources_pass_through_untouched() {
        let sources = [
            "auto_high",
            "argued_auto_high",
            "auto_low",
            "rule",
            "llm",
            "llm_propagated",
            "fallback",
            "heuristic",
            "never_flag",
            "system_floor",
            "adjudication",
        ];
        let entries: Vec<serde_json::Value> = sources
            .iter()
            .enumerate()
            .map(|(i, s)| {
                json!({
                    "candidate": {"id": format!("c-{i}"), "path": format!("/p{i}"),
                        "score_components": {"size": 1.0, "cache_location": 15.0}},
                    "tier_source": s
                })
            })
            .collect();
        let v = report_to_frontend(json!({ "entries": entries, "groups": [], "warnings": [] }));
        for (i, s) in sources.iter().enumerate() {
            assert_eq!(
                v["entries"][i]["tier_source"], *s,
                "tier_source {s} must reach the frontend verbatim"
            );
        }
    }

    /* ---------- hand-off: pure logic ---------- */

    fn item(path: &str, bytes: u64, tier: u8, scope: &str) -> HandoffItem {
        HandoffItem {
            path: path.into(),
            size_bytes: bytes,
            tier,
            scope: scope.into(),
        }
    }

    fn folders_of(groups: &[HandoffGroup]) -> Vec<String> {
        groups
            .iter()
            .map(|g| g.folder.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn taint_rules() {
        assert!(is_tainted(4, "low"), "Source tier taints");
        assert!(is_tainted(1, "high"), "high personal risk taints");
        assert!(is_tainted(2, "somewhere-else"), "unknown scope taints");
        assert!(!is_tainted(3, "medium"));
        assert!(!is_tainted(1, "low"));
    }

    #[test]
    fn groups_collapse_to_distinct_parents_and_fold_descendants() {
        let items = vec![
            item("/a/b/x", 10, 1, "low"),
            item("/a/b/y", 20, 2, "low"),
            item("/a/b/c/z", 30, 1, "low"),
            item("/elsewhere/w", 5, 1, "low"),
        ];
        let (groups, skipped) = group_items(items);
        assert!(skipped.is_empty());
        assert_eq!(folders_of(&groups), vec!["/a/b", "/elsewhere"]);
        let folded = groups.iter().find(|g| g.folder == Path::new("/a/b")).unwrap();
        assert_eq!(folded.items.len(), 3, "descendant folded into ancestor");
        assert_eq!(folded.finding_bytes(), 60);
    }

    #[test]
    fn component_boundary_comparison() {
        // /foo/barbaz must never fold into /foo/bar.
        let items = vec![
            item("/foo/barbaz/x", 10, 1, "low"),
            item("/foo/bar/y", 20, 1, "low"),
        ];
        let (groups, _) = group_items(items);
        assert_eq!(folders_of(&groups), vec!["/foo/bar", "/foo/barbaz"]);
        assert!(is_proper_ancestor(Path::new("/foo/bar"), Path::new("/foo/bar/baz")));
        assert!(!is_proper_ancestor(Path::new("/foo/bar"), Path::new("/foo/barbaz")));
        assert!(!is_proper_ancestor(Path::new("/a"), Path::new("/a")));
    }

    #[test]
    fn ordering_is_bytes_desc_then_path() {
        let items = vec![
            item("/small/a", 1, 1, "low"),
            item("/zbig/a", 100, 1, "low"),
            item("/mid/a", 50, 1, "low"),
        ];
        let (groups, _) = group_items(items);
        assert_eq!(
            folders_of(&groups),
            vec!["/zbig", "/mid", "/small"],
            "biggest reclaimable bytes first so the user can stop early"
        );
    }

    fn allowed(paths: &[&str]) -> HashSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn rollup_merges_into_scanned_root_when_over_target() {
        let items: Vec<HandoffItem> = (1..=5)
            .map(|i| item(&format!("/root/f{i}/leaf"), i as u64 * 10, 1, "low"))
            .chain([
                item("/else/g1/x", 3, 1, "low"),
                item("/else/g2/y", 2, 1, "low"),
            ])
            .collect();
        let (groups, _) = group_items(items);
        assert_eq!(groups.len(), 7);
        let findings: HashSet<String> = Vec::new().into_iter().collect();
        let rolled = rollup_groups(groups, &allowed(&["/root"]), &findings, 3);
        assert_eq!(
            folders_of(&rolled),
            // /root carries the most reclaimable bytes after the merge, so
            // it sorts first and the user's first window is the big one.
            vec!["/root", "/else/g1", "/else/g2"],
            "the scanned root covering the most bytes wins"
        );
        assert_eq!(
            rolled.iter().find(|g| g.folder == Path::new("/root")).unwrap().items.len(),
            5
        );
    }

    #[test]
    fn rollup_never_merges_tainted_groups() {
        let items: Vec<HandoffItem> = (1..=5)
            .map(|i| {
                // f3 holds a Source-tier finding: no ancestor may absorb it.
                let (tier, scope) = if i == 3 { (4u8, "low") } else { (1u8, "low") };
                item(&format!("/root/f{i}/leaf"), i as u64 * 10, tier, scope)
            })
            .collect();
        let (groups, _) = group_items(items);
        assert!(groups.iter().any(|g| g.tainted));
        let findings: HashSet<String> = Vec::new().into_iter().collect();
        let rolled = rollup_groups(groups, &allowed(&["/root"]), &findings, 1);
        assert_eq!(rolled.len(), 5, "taint blocks the whole candidate merge");
        assert_eq!(
            folders_of(&rolled),
            vec![
                "/root/f5", "/root/f4", "/root/f3", "/root/f2", "/root/f1"
            ]
        );
    }

    #[test]
    fn rollup_never_rolls_into_a_finding() {
        // /root/f1 is itself a finding, so it may never become the container
        // that absorbs its own subtree. (Built directly, bypassing the
        // parent-fold in group_items, which would already have merged the
        // subtree into a group that holds the finding.)
        let groups = vec![
            HandoffGroup {
                folder: PathBuf::from("/root/f1"),
                items: vec![item("/root/f1", 10, 1, "low")],
                tainted: false,
            },
            HandoffGroup {
                folder: PathBuf::from("/root/f1/sub"),
                items: vec![item("/root/f1/sub/leaf", 20, 1, "low")],
                tainted: false,
            },
        ];
        let findings: HashSet<String> = ["/root/f1".to_string()].into_iter().collect();
        let rolled = rollup_groups(groups, &allowed(&["/root/f1"]), &findings, 1);
        assert_eq!(
            folders_of(&rolled),
            vec!["/root/f1", "/root/f1/sub"],
            "the candidate ancestor that is a finding is skipped"
        );
    }

    #[test]
    fn rollup_ignores_ancestors_outside_allowed_set() {
        let items: Vec<HandoffItem> = (1..=4)
            .map(|i| item(&format!("/home/x/f{i}/leaf"), i as u64, 1, "low"))
            .collect();
        let (groups, _) = group_items(items);
        let findings: HashSet<String> = Vec::new().into_iter().collect();
        // /home and /home/x are not scanned roots: no rollup, ever, however
        // many containers there are.
        let rolled = rollup_groups(groups, &allowed(&["/root"]), &findings, 1);
        assert_eq!(rolled.len(), 4);
    }

    #[test]
    fn rollup_is_deterministic() {
        let items: Vec<HandoffItem> = (1..=6)
            .map(|i| item(&format!("/root/f{i}/leaf"), (i % 3) as u64 * 10 + 1, 1, "low"))
            .chain([
                item("/root/other/a", 15, 1, "low"),
                item("/root/other/b", 15, 1, "low"),
            ])
            .collect();
        let findings: HashSet<String> = Vec::new().into_iter().collect();
        let (g1, _) = group_items(items.clone());
        let r1 = rollup_groups(g1, &allowed(&["/root"]), &findings, 4);
        let (g2, _) = group_items(items);
        let r2 = rollup_groups(g2, &allowed(&["/root"]), &findings, 4);
        assert_eq!(folders_of(&r1), folders_of(&r2));
        assert_eq!(
            r1.iter().map(|g| g.items.len()).collect::<Vec<_>>(),
            r2.iter().map(|g| g.items.len()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn allowed_ancestors_exclude_home_and_root() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join("Library/Caches")).unwrap();
        std::fs::create_dir_all(home.join("Downloads")).unwrap();
        let set = allowed_ancestors_for(&home);
        assert!(set.contains(&home.join("Library/Caches")));
        assert!(set.contains(&home.join("Downloads")));
        assert!(!set.contains(&home), "never roll up to $HOME itself");
        assert!(!set.contains(Path::new("/")), "never roll up to the filesystem root");
    }

    #[test]
    fn unparentable_paths_are_reported_not_dropped() {
        let items = vec![item("/", 1, 1, "low"), item("/tmp/x", 2, 1, "low")];
        let (groups, skipped) = group_items(items);
        assert_eq!(folders_of(&groups), vec!["/tmp"]);
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].path, "/");
        assert!(!skipped[0].reason.is_empty());
    }

    /* ---------- hand-off: counts, escaping, script ---------- */

    #[test]
    fn count_dir_entries_caps_and_unknown() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..3 {
            std::fs::write(dir.path().join(format!("f{i}")), "x").unwrap();
        }
        assert_eq!(count_dir_entries_capped(dir.path(), 100), Some(3));
        // Past the cap the honest answer is unknown, not a wrong number.
        assert_eq!(count_dir_entries_capped(dir.path(), 2), None);
        assert_eq!(count_dir_entries_capped(dir.path(), 0), None);
        assert_eq!(count_dir_entries(&dir.path().join("missing")), None);
        // A file is not a directory: unknown, never zero.
        assert_eq!(count_dir_entries(&dir.path().join("f0")), None);
    }

    #[test]
    fn count_dir_entries_reports_unknown_on_permissions() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let locked = dir.path().join("locked");
            std::fs::create_dir(&locked).unwrap();
            std::fs::write(locked.join("inner"), "x").unwrap();
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
            let counted = count_dir_entries(&locked);
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(counted, None, "unreadable means unknown, never zero");
        }
    }

    #[test]
    fn apple_script_escaping_survives_hostile_paths() {
        assert_eq!(
            apple_script_escape("/tmp/a\"b\\c\nd\te"),
            "/tmp/a\\\"b\\\\c\\nd\\te"
        );
        assert_eq!(apple_script_escape("/tmp/🎉-世界"), "/tmp/🎉-世界");
        assert_eq!(apple_script_escape("/plain/path"), "/plain/path");
    }

    #[test]
    fn script_shape_windows_objects_and_readbacks() {
        let groups = vec![
            HandoffGroup {
                folder: PathBuf::from("/tmp/a\"quote"),
                items: vec![
                    item("/tmp/a\"quote/p1", 1, 1, "low"),
                    item("/tmp/a\"quote/p2", 2, 1, "low"),
                ],
                tainted: false,
            },
            HandoffGroup {
                folder: PathBuf::from("/tmp/b"),
                items: vec![item("/tmp/b/p3", 3, 1, "low")],
                tainted: true,
            },
        ];
        let script = build_handoff_script(&groups, 100);
        // Escaped folder path, one window per group, one alias coercion per
        // finding — each in its own try, so a vanished path misses alone.
        assert!(script.contains("POSIX file \"/tmp/a\\\"quote\""));
        assert!(script.contains("make new Finder window"));
        assert!(script.contains("set target of w0 to"));
        assert!(script.contains("(POSIX file (contents of p) as alias)"));
        assert!(script.contains("set end of s0 to (POSIX file (contents of p) as alias)"));
        assert!(
            !script.contains("s{i}"),
            "a literal {{i}} in the program would make every alias step error"
        );
        assert!(script.contains("repeat with p in {\"/tmp/a\\\"quote/p1\", \"/tmp/a\\\"quote/p2\"}"));
        assert!(script.contains("repeat with p in {\"/tmp/b/p3\"}"));
        assert!(script.contains(":MISS:\" & my flatten(contents of p)"));
        // Verification is a readback, and both failure kinds are separable.
        assert!(script.contains("set n0 to count of (get selection)"));
        assert!(script.contains(":ERRWIN:"));
        assert!(script.contains(":ERRSEL:"));
        // Windows are objects, never indices.
        assert!(script.contains("set index of wFirst to 1"));
        assert!(script.contains("activate wFirst"));
        // Activation happens once, after every window exists: it must come
        // after the last group's select, not inside a group block.
        let last_select = script.rfind("select s1").unwrap();
        let activate = script.find("activate wFirst").unwrap();
        assert!(activate > last_select);
    }

    #[test]
    fn script_chunks_large_selections() {
        let items: Vec<HandoffItem> = (0..250)
            .map(|i| item(&format!("/tmp/big/p{i}"), 1, 1, "low"))
            .collect();
        let groups = vec![HandoffGroup {
            folder: PathBuf::from("/tmp/big"),
            items,
            tainted: false,
        }];
        let script = build_handoff_script(&groups, 100);
        assert_eq!(
            script.matches("repeat with p in {").count(),
            3,
            "250 paths in chunks of 100 build the list in three steps"
        );
        assert_eq!(script.matches("select s0").count(), 1, "one select per window");
        // A selection that ends up empty still reports a verified zero
        // instead of letting `select {}` fail the group.
        assert!(script.contains("if (count of s0) > 0 then"));
        assert!(script.contains("set n0 to 0"));
    }

    #[test]
    fn script_output_parses_into_verified_groups() {
        let out = "0:OK:12\n1:ERRSEL:-1712:Finder got an error: AppleEvent timed out.\n2:ERRWIN:-1728:Expected end of line but found identifier.\n0:MISS:/tmp/vanished\nnoise";
        let parsed = parse_script_output(out, 4);
        assert_eq!(parsed.outcomes[0], Some(GroupOutcome::Ok(12)));
        assert_eq!(
            parsed.outcomes[1],
            Some(GroupOutcome::ErrSel {
                num: -1712,
                message: "Finder got an error: AppleEvent timed out.".into()
            })
        );
        assert_eq!(
            parsed.outcomes[2],
            Some(GroupOutcome::ErrWin {
                num: -1728,
                message: "Expected end of line but found identifier.".into()
            })
        );
        assert_eq!(
            parsed.outcomes[3], None,
            "a group Finder said nothing about stays unknown"
        );
        assert_eq!(
            parsed.missed,
            vec![(0usize, "/tmp/vanished".to_string())],
            "mid-flight vanish is per item, per group"
        );
    }

    /* ---------- hand-off: wire contract ---------- */

    /// The serialized shapes must stay in lockstep with `app/src/lib/types.ts`.
    #[test]
    fn handoff_shapes_match_frontend_contract() {
        let summary = RevealSummary {
            groups: vec![RevealGroup {
                folder: "/f".into(),
                findings: 2,
                finding_bytes: 30,
                total_in_folder: Some(40),
                requested: 2,
                selected: 2,
                tainted: true,
                ok: true,
                error: None,
            }],
            skipped: vec![SkippedPath {
                path: "/g/one".into(),
                reason: "vanished since the scan".into(),
            }],
            windows: 1,
            items_requested: 2,
            items_selected: 2,
        };
        let v = serde_json::to_value(&summary).unwrap();
        // serde_json orders object keys; assert the exact key set, not the order.
        let group_keys = v["groups"][0].as_object().unwrap();
        let expected_group_keys = [
            "folder",
            "findings",
            "finding_bytes",
            "total_in_folder",
            "requested",
            "selected",
            "tainted",
            "ok",
        ];
        assert_eq!(
            group_keys.len(),
            expected_group_keys.len(),
            "RevealGroup keys must mirror the RevealGroup interface exactly"
        );
        for key in expected_group_keys {
            assert!(group_keys.contains_key(key), "missing key {key}");
        }
        assert_eq!(v["groups"][0]["total_in_folder"], 40);
        let summary_keys = v.as_object().unwrap();
        let expected_summary_keys = ["groups", "skipped", "windows", "items_requested", "items_selected"];
        assert_eq!(summary_keys.len(), expected_summary_keys.len());
        for key in expected_summary_keys {
            assert!(summary_keys.contains_key(key), "missing key {key}");
        }
        assert_eq!(v["skipped"][0]["reason"], "vanished since the scan");

        // total_in_folder null stays null, never zero.
        let unknown = RevealGroup {
            total_in_folder: None,
            ..summary.groups[0].clone()
        };
        assert_eq!(serde_json::to_value(&unknown).unwrap()["total_in_folder"], serde_json::Value::Null);

        let plan = HandoffPlan {
            groups: vec![PlanGroup {
                folder: "/f".into(),
                findings: 2,
                finding_bytes: 30,
                total_in_folder: None,
                tainted: false,
            }],
            skipped: vec![],
            windows: 1,
        };
        let pv = serde_json::to_value(&plan).unwrap();
        let plan_group_keys = pv["groups"][0].as_object().unwrap();
        let expected_plan_group_keys =
            ["folder", "findings", "finding_bytes", "total_in_folder", "tainted"];
        assert_eq!(plan_group_keys.len(), expected_plan_group_keys.len());
        for key in expected_plan_group_keys {
            assert!(plan_group_keys.contains_key(key), "missing key {key}");
        }
        let plan_keys = pv.as_object().unwrap();
        assert_eq!(plan_keys.len(), 3);
        for key in ["groups", "skipped", "windows"] {
            assert!(plan_keys.contains_key(key), "missing key {key}");
        }

        assert_eq!(
            serde_json::to_value(RevealError::AutomationDenied).unwrap(),
            json!({"kind": "automation_denied"})
        );
        assert_eq!(
            serde_json::to_value(RevealError::Script {
                message: "boom".into()
            })
            .unwrap(),
            json!({"kind": "script", "message": "boom"})
        );
    }

    /* ---------- real-machine end-to-end (opt-in) ---------- */

    /// The full pipeline against the real Finder: plan → script → osascript
    /// → verified readback. Opt-in because it opens real Finder windows and
    /// needs Automation consent:
    ///   cargo test -p driftwood-app --lib -- --ignored --nocapture real_finder
    ///
    /// Fixture: one 100-file group (chunked bulk select), one group in a
    /// hostile folder name (quote + space), one small group, and one path
    /// that does not exist — the mid-flight vanish the per-item try exists
    /// for. Asserts exactly what Finder reports, then closes the windows it
    /// opened. The fixture is a tempdir DriftWood created; removing it
    /// touches no scanned path.
    #[test]
    #[ignore = "opens real Finder windows; run with --ignored"]
    fn real_finder_handoff_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let a = root.join("bulk");
        let b = root.join("quote's \"dir\" name");
        let c = root.join("plain");
        for d in [&a, &b, &c] {
            std::fs::create_dir_all(d).unwrap();
        }
        // 99 real files plus the race victim f099: it exists at plan time
        // and is removed before the script runs — the window between the
        // existence check and Finder that the per-item try exists for. (The
        // test owns this fixture file; nothing on a scanned path is ever
        // touched by the product.)
        let mut items: Vec<HandoffItem> = (0..99)
            .map(|i| {
                let p = a.join(format!("f{i:03}.dat"));
                std::fs::write(&p, "x").unwrap();
                item(&p.display().to_string(), 10, 1, "low")
            })
            .collect();
        let race = a.join("f099.dat");
        std::fs::write(&race, "x").unwrap();
        items.push(item(&race.display().to_string(), 10, 1, "low"));
        for (name, tier) in [("one", 1), ("two", 2)] {
            let p = b.join(format!("{name}.dat"));
            std::fs::write(&p, "x").unwrap();
            items.push(item(&p.display().to_string(), 5, tier, "low"));
        }
        for name in ["a", "b", "c"] {
            let p = c.join(format!("{name}.dat"));
            std::fs::write(&p, "x").unwrap();
            items.push(item(&p.display().to_string(), 1, 1, "low"));
        }

        let (groups, skipped) = plan_groups(items);
        assert!(skipped.is_empty(), "everything exists at plan time");
        std::fs::remove_file(&race).unwrap();
        assert_eq!(groups.len(), 3, "one group per fixture folder");
        assert_eq!(groups[0].folder, a, "biggest group first");

        let script = build_handoff_script(&groups, SELECTION_CHUNK);
        let (stdout, _stderr) =
            run_osascript(&script).expect("osascript run (is Automation consent granted?)");
        let result = parse_script_output(&stdout, groups.len());

        println!("stdout: {stdout:?}");
        let missed_a = result
            .missed
            .iter()
            .filter(|(i, _)| *i == 0)
            .count();
        assert_eq!(missed_a, 1, "the vanished path misses alone");
        match &result.outcomes[0] {
            Some(GroupOutcome::Ok(n)) => assert_eq!(*n, 99, "100 files minus the vanish"),
            other => panic!("group 0 should be OK, got {other:?}"),
        }
        for i in [1usize, 2] {
            match &result.outcomes[i] {
                Some(GroupOutcome::Ok(n)) => assert_eq!(*n, groups[i].items.len()),
                other => panic!("group {i} should be OK, got {other:?}"),
            }
        }

        // Close the windows this test opened: exactly those whose target is
        // inside the fixture, by path — never by count or position. Finder
        // reports the resolved path (/private/var/… for /var/…), so both
        // prefixes match.
        let close = format!(
            "tell application \"Finder\"\n  set rootP to \"{}\"\n  set victims to {{}}\n  repeat with i from (count of windows) to 1 by -1\n    try\n      set rp to POSIX path of (target of window i as alias)\n      if rp starts with rootP or rp starts with (\"/private\" & rootP) then\n        set end of victims to window i\n      end if\n    end try\n  end repeat\n  repeat with w in victims\n    close w\n  end repeat\n  return count of victims\nend tell",
            apple_script_escape(&format!("{}/", root.display()))
        );
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&close)
            .output()
            .expect("cleanup osascript");
        println!(
            "closed {} fixture windows",
            String::from_utf8_lossy(&out.stdout).trim()
        );
    }

    /* ---------- the moat ---------- */

    /// The hand-off reads directories and drives Finder's windows; it must
    /// never write to, move, or discard anything on a scanned path. Files
    /// that go through the whole pure pipeline must come out byte-identical.
    #[test]
    fn handoff_pipeline_leaves_files_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("scanned");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let mut originals = Vec::new();
        for (i, rel) in ["one", "two", "sub/three"].iter().enumerate() {
            let path = root.join(rel);
            std::fs::write(&path, format!("content-{i}")).unwrap();
            originals.push((path, format!("content-{i}")));
        }
        let items = vec![
            item(
                &root.join("one").display().to_string(),
                9,
                1,
                "low",
            ),
            item(
                &root.join("two").display().to_string(),
                9,
                2,
                "low",
            ),
            item(
                &root.join("sub/three").display().to_string(),
                9,
                1,
                "low",
            ),
        ];
        // The same core planning and execution use (existence check, fold,
        // rollup, counts), plus the script builder — no Finder, no writes.
        let (groups, skipped) = plan_groups(items);
        assert!(skipped.is_empty(), "fixtures must exist");
        let _totals: Vec<Option<u64>> = groups.iter().map(|g| count_dir_entries(&g.folder)).collect();
        let _script = build_handoff_script(&groups, SELECTION_CHUNK);
        for (path, content) in originals {
            assert!(path.exists(), "{path:?} must still exist");
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                content,
                "{path:?} must be byte-identical"
            );
        }
    }

    /// Moat test, not a formality: the production half of this file may
    /// never name a filesystem-mutation API, so a deletion code path cannot
    /// quietly appear in the hand-off. (The test half is excluded — it must
    /// name the tokens to forbid them.)
    #[test]
    fn handoff_source_contains_no_mutation_apis() {
        let source = include_str!("lib.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("the test module always exists");
        // Strip line comments so prose may still say "never deletes".
        let code: String = production
            .lines()
            .map(|l| match l.find("//") {
                Some(i) => &l[..i],
                None => l,
            })
            .collect::<Vec<_>>()
            .join("\n");
        for token in [
            "remove_file",
            "remove_dir",
            "fs::rename",
            "fs::write",
            "trash",
        ] {
            assert!(
                !code.contains(token),
                "production hand-off code must not contain {token:?} — \
                 DriftWood never deletes, moves, or writes a scanned path"
            );
        }
    }

    /* ---------- requirement 1: measurement ---------- */

    /// Requirement 1's measurement: collapse a path list to its distinct
    /// parent directories and report the items/bytes distribution. Planning
    /// data for deciding how much rollup matters — not part of the hand-off
    /// path itself.
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
    struct ParentStat {
        parent: String,
        items: usize,
        bytes: u64,
    }

    fn parent_stats(items: &[HandoffItem]) -> Vec<ParentStat> {
        let mut by_parent: BTreeMap<String, (usize, u64)> = BTreeMap::new();
        for item in items {
            if let Some(parent) = Path::new(&item.path).parent() {
                let entry = by_parent
                    .entry(parent.to_string_lossy().into_owned())
                    .or_insert((0, 0));
                entry.0 += 1;
                entry.1 += item.size_bytes;
            }
        }
        let mut stats: Vec<ParentStat> = by_parent
            .into_iter()
            .map(|(parent, (items, bytes))| ParentStat { parent, items, bytes })
            .collect();
        stats.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.parent.cmp(&b.parent)));
        stats
    }

    /// Measures the real persisted report on this machine: how many distinct
    /// parent directories the findings collapse to, and the items/bytes
    /// distribution over them. Run with:
    ///   cargo test -p driftwood-app --lib -- --ignored --nocapture
    #[test]
    #[ignore = "measures the real persisted report on this machine"]
    fn measurement_real_report_parent_distribution() {
        let report = Report::load_last().expect("no persisted report on this machine");
        let items: Vec<HandoffItem> = report
            .entries
            .iter()
            .map(|e| HandoffItem {
                path: e.candidate.path.clone(),
                size_bytes: e.candidate.size_bytes,
                tier: e.tier as u8,
                scope: match e.candidate.scope_category {
                    ScopeCategory::Low => "low",
                    ScopeCategory::Medium => "medium",
                    ScopeCategory::High => "high",
                }
                .into(),
            })
            .collect();
        let stats = parent_stats(&items);
        println!("entries: {}", items.len());
        println!("distinct parents: {}", stats.len());
        for s in &stats {
            println!("{:5} items {:10.2} GB  {}", s.items, s.bytes as f64 / 1e9, s.parent);
        }
        assert_eq!(
            stats.iter().map(|s| s.items).sum::<usize>(),
            items.len(),
            "every finding must land in exactly one parent"
        );
    }

    #[test]
    fn parent_stats_splits_and_sorts() {
        let items = vec![
            item("/small/a", 1, 1, "low"),
            item("/zbig/a", 100, 1, "low"),
            item("/zbig/b", 50, 1, "low"),
            item("/small/b", 2, 1, "low"),
        ];
        let stats = parent_stats(&items);
        assert_eq!(
            stats,
            vec![
                ParentStat { parent: "/zbig".into(), items: 2, bytes: 150 },
                ParentStat { parent: "/small".into(), items: 2, bytes: 3 },
            ]
        );
    }
}
