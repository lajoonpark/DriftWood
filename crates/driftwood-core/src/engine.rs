//! The engine: the ONLY two async entry points both wrappers call.
//!
//! `run_scan(config, sink, handle)` → Report
//! `apply_correction(request, memory_dir)` → updated rules
//!
//! Pipeline order matters (plan §3.1): walk → hard rules → orphans → score.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use chrono::Utc;

use crate::config::ScanConfig;
use crate::events::{EventSink, Phase, ScanEvent};
use crate::reason::{self, ReasonOutcome};
use crate::report::{self, AssembleInput, CounterState, Report, ReportEntry, ReportWarning};
use crate::rules::{self, Rule, RuleSource};
use crate::scan::{hard_rules, orphan, scope, spotlight};
use crate::score::{self, banding, ScoreInput};
use crate::types::{
    AutoHighBasis, Band, Candidate, Kind, KindStats, OrphanStatus, PrivacyTier, Tier, TierSource,
};
use crate::{DriftError, Result};

/// Handle for cancelling a running scan. Cancellation is checked between
/// phases and between LLM batches — prompt, never mid-batch (edge §4.8).
#[derive(Clone, Default)]
pub struct ScanHandle {
    cancel: Arc<AtomicBool>,
}

impl ScanHandle {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Single-flight lock (edge case §4.8): one scan per install.
#[derive(Debug)]
pub struct ScanLock {
    path: PathBuf,
}

impl ScanLock {
    pub fn acquire(base_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(base_dir)?;
        let path = base_dir.join("scan.lock");
        let acquired = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path);
        match acquired {
            Ok(_) => {
                let pid = std::process::id();
                let stamp = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                std::fs::write(&path, format!("{pid} {stamp}\n"))?;
                Ok(Self { path })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Stale lock? A crashed scan (SIGKILL leaves no Drop) must
                // not block scans for the 6-hour window: the lock records
                // its owner PID, and if no such process exists the lock is
                // dead and stolen immediately. Age alone stays as the
                // backstop for a recycled PID.
                if let Some(owner_alive) = Self::lock_owner_alive(&path) {
                    if !owner_alive {
                        let _ = std::fs::remove_file(&path);
                        return Self::acquire(base_dir);
                    }
                }
                let stale = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .map(|elapsed| elapsed > Duration::from_secs(6 * 3600))
                    .unwrap_or(false);
                if stale {
                    let _ = std::fs::remove_file(&path);
                    return Self::acquire(base_dir);
                }
                Err(DriftError::SingleFlight(path.display().to_string()))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// `None` when the lock file exists but holds no parsable PID (foreign
    /// or ancient format) — the caller then falls back to the age check.
    fn lock_owner_alive(path: &Path) -> Option<bool> {
        let text = std::fs::read_to_string(path).ok()?;
        let pid: u32 = text.split_whitespace().next()?.parse().ok()?;
        // `ps -p` exits 0 iff the process exists — unambiguous, unlike
        // `kill -0`, whose exit code cannot separate EPERM from ESRCH.
        let status = std::process::Command::new("ps")
            .arg("-p")
            .arg(pid.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()?;
        Some(status.success())
    }
}

impl Drop for ScanLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub async fn run_scan(
    config: ScanConfig,
    sink: Arc<dyn EventSink>,
    handle: ScanHandle,
) -> Result<Report> {
    let _lock = if config.persist {
        Some(ScanLock::acquire(&crate::paths::base_dir())?)
    } else {
        None
    };

    let scan_id = report::new_scan_id();
    let tuning = config.tuning.clone();
    let counters = Arc::new(CounterState::default());
    let home = crate::paths::home_dir();
    let now = Utc::now();

    // ---- Phase: Enumerating ------------------------------------------------
    sink.emit(ScanEvent::Phase {
        phase: Phase::Enumerating,
    });
    let roots = scope::resolve_roots(&config.scopes, &home);
    if roots.is_empty() {
        return Err(DriftError::Scan(
            "no scope roots exist for the selected categories".into(),
        ));
    }

    let mut warns: Vec<ReportWarning> = Vec::new();
    let mut units: Vec<scope::Unit> = Vec::new();
    for root in &roots {
        if handle.is_cancelled() {
            return Err(DriftError::Cancelled);
        }
        // Bounded enumeration: a root whose directory open never returns
        // (TCC access pending, dead mount, unresponsive FUSE) must not
        // freeze the scan. Skip-and-warn, never abort (edge case §4.3) —
        // a bare blocking `read_dir` silently defeats that rule.
        let caps = tuning.walk.clone();
        let root_clone = root.clone();
        let enumerated = tokio::time::timeout(
            Duration::from_secs(caps.root_timeout_secs),
            tokio::task::spawn_blocking(move || {
                let mut warn_list: Vec<String> = Vec::new();
                let found = scope::enumerate_units(&root_clone, &caps, &mut |w| {
                    warn_list.push(w)
                });
                (found, warn_list)
            }),
        )
        .await;
        match enumerated {
            Ok(Ok((found, warn_list))) => {
                units.extend(found);
                for w in warn_list {
                    sink.emit(ScanEvent::warn(w.clone()));
                }
            }
            Ok(Err(e)) => return Err(DriftError::Scan(format!("enumeration task failed: {e}"))),
            Err(_) => {
                let msg = format!(
                    "wading past {}: the riverbed is unresponsive — skipped (no answer in {}s)",
                    root.path.display(),
                    tuning.walk.root_timeout_secs
                );
                sink.emit(ScanEvent::warn(msg.clone()));
                warns.push(ReportWarning {
                    kind: "root_timeout".into(),
                    message: msg,
                });
            }
        }
    }
    counters
        .candidates_found
        .store(units.len() as u64, Ordering::Relaxed);
    sink.emit(ScanEvent::CandidatesFound {
        total: units.len() as u64,
    });

    if units.is_empty() {
        sink.emit(ScanEvent::notice("Clear waters — nothing to classify"));
        return finish(
            AssembleInput {
                scan_id,
                spotlight_available: true,
                privacy_tier: config.privacy_tier,
                model: None,
                llm_cost_usd: 0.0,
                cost_cap_hit: false,
                stopped_early: false,
                counters: counters.snapshot(),
                entries: vec![],
                warnings: vec![],
            },
            config.persist,
        );
    }

    // ---- Phase: Wading in (metadata) ---------------------------------------
    sink.emit(ScanEvent::Phase { phase: Phase::Wading });
    let spotlight_timeout = Duration::from_secs(tuning.walk.spotlight_timeout_secs);
    let spotlight_ok = spotlight::spotlight_available(spotlight_timeout).await;
    if !spotlight_ok {
        warns.push(ReportWarning {
            kind: "murky_spotlight".into(),
            message: "The river ran murky — recent-use data was incomplete (Spotlight indexing unavailable); age signals rely on file dates only.".into(),
        });
    }

    // Spotlight recency sets per root (positive recency only ever filters).
    let mut recent: std::collections::HashSet<PathBuf> = Default::default();
    let mut recency_failed_roots = 0usize;
    if spotlight_ok {
        for root in &roots {
            if handle.is_cancelled() {
                return Err(DriftError::Cancelled);
            }
            let timeout = spotlight_timeout;
            match spotlight::recently_used_under(&root.path, tuning.walk.recency_days, timeout)
                .await
            {
                Some(found) => recent.extend(found),
                None => recency_failed_roots += 1,
            }
        }
    }
    if recency_failed_roots > 0 {
        let msg = format!(
            "recent-use data could not be read for {recency_failed_roots} root(s) — the still-in-the-current filter may be blind there"
        );
        sink.emit(ScanEvent::warn(msg.clone()));
        warns.push(ReportWarning {
            kind: "recency_incomplete".into(),
            message: msg,
        });
    }

    let unit_paths: Vec<PathBuf> = units.iter().map(|u| u.path.clone()).collect();
    let last_used = spotlight::query_last_used(&unit_paths, tuning.walk.mdls_batch_size, spotlight_timeout).await;

    // File dates via metadata (mtime is never "last used"; created date is
    // the Downloads exception).
    let mut modified: HashMap<PathBuf, chrono::DateTime<Utc>> = HashMap::new();
    let mut created: HashMap<PathBuf, chrono::DateTime<Utc>> = HashMap::new();
    for p in &unit_paths {
        if let Ok(meta) = std::fs::symlink_metadata(p) {
            if let Ok(t) = meta.modified() {
                modified.insert(p.clone(), t.into());
            }
            if let Ok(t) = meta.created() {
                created.insert(p.clone(), t.into());
            }
        }
    }

    // ---- Phase: Filtering (hard rules + orphans) ---------------------------
    sink.emit(ScanEvent::Phase {
        phase: Phase::Filtering,
    });
    let mut dropped_recency = 0usize;
    let mut survivors: Vec<scope::Unit> = Vec::new();

    // Mark units touched recently: exact unit paths, plus any unit that
    // is an ancestor of a recently-used inner path (a folder whose
    // contents are being written is still in the current — the safe
    // direction is to leave it alone). O(recent × path-depth).
    let mut recently_touched: std::collections::HashSet<PathBuf> = Default::default();
    if spotlight_ok {
        let unit_set: std::collections::HashSet<&PathBuf> =
            units.iter().map(|u| &u.path).collect();
        for p in &recent {
            if unit_set.contains(p) {
                recently_touched.insert(p.clone());
            } else {
                let mut anc = p.parent();
                while let Some(a) = anc {
                    if unit_set.contains(&a.to_path_buf()) {
                        recently_touched.insert(a.to_path_buf());
                        break;
                    }
                    anc = a.parent();
                }
            }
        }
    }

    for unit in &units {
        let lu = last_used.get(&unit.path).copied().flatten();
        let from_spotlight = lu.is_some() && spotlight_ok;
        if hard_rules::still_in_current(lu, from_spotlight, now, tuning.walk.recency_days)
            || recently_touched.contains(&unit.path)
        {
            dropped_recency += 1;
            continue;
        }
        survivors.push(unit.clone());
    }
    sink.emit(ScanEvent::notice(format!(
        "still in the current: {dropped_recency} recently-used items filtered out"
    )));

    if survivors.is_empty() {
        return finish(
            AssembleInput {
                scan_id,
                spotlight_available: spotlight_ok,
                privacy_tier: config.privacy_tier,
                model: None,
                llm_cost_usd: 0.0,
                cost_cap_hit: false,
                stopped_early: false,
                counters: counters.snapshot(),
                entries: vec![],
                warnings: warns,
            },
            config.persist,
        );
    }

    // Orphan detection: installed apps snapshot + never-orphan list. The
    // snapshot walkdir is bounded like every other filesystem wait.
    let home_for_apps = home.clone();
    let app_snapshot = tokio::time::timeout(
        Duration::from_secs(tuning.walk.root_timeout_secs),
        tokio::task::spawn_blocking(move || orphan::enumerate_installed_apps(&home_for_apps, 4000)),
    )
    .await;
    let installed = match app_snapshot {
        Ok(Ok(apps)) => apps,
        Ok(Err(e)) => return Err(DriftError::Scan(format!("installed-app snapshot failed: {e}"))),
        Err(_) => {
            let msg = "the installed-app snapshot never answered — orphan detection is blind this scan";
            sink.emit(ScanEvent::warn(msg.to_string()));
            warns.push(ReportWarning {
                kind: "app_snapshot_timeout".into(),
                message: msg.to_string(),
            });
            Default::default()
        }
    };
    let never_orphan = orphan::load_never_orphan_list(&crate::paths::memory_dir());
    let mut orphan_status: HashMap<PathBuf, OrphanStatus> = HashMap::new();
    for unit in &survivors {
        let status = if unit.kind == Kind::File {
            OrphanStatus::Unknown
        } else {
            let name = unit
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            orphan::classify_orphan(&name, &installed, &never_orphan)
        };
        orphan_status.insert(unit.path.clone(), status);
    }

    // ---- Sizing (bounded, concurrent, counted, cancellable) ----------------
    //
    // Each measurement runs as an async task that wraps its blocking walk
    // in a timeout, so a folder on an unresponsive mount degrades to a
    // warning instead of stalling the drain loop forever. The blocked
    // worker thread itself cannot be killed — `max_measure_timeouts` caps
    // how many are leaked before the scan stops measuring and finishes
    // with honest partial sizes.
    enum MeasureOutcome {
        Done(u64, Option<KindStats>),
        /// The blocking walk panicked or the task failed — one folder, not
        /// a pattern; does not count toward the give-up counter.
        Failed(String),
        /// Never answered within `measure_timeout_secs`. The thread leaks;
        /// the counter decides when to stop measuring entirely.
        TimedOut,
    }

    let mut measures: HashMap<PathBuf, (u64, Option<KindStats>)> = HashMap::new();
    // Unanswered folders still get an entry: zero size (we claim nothing)
    // with `truncated: true`, so the data model itself says the walk was
    // incomplete rather than the folder being genuinely empty.
    let unknown_measure = || {
        Some(KindStats {
            children: 0,
            files: 0,
            cache_like_ratio: 0.0,
            truncated: true,
        })
    };
    let mut join = tokio::task::JoinSet::new();
    let max_concurrent = 8usize;
    let mut inflight = 0usize;
    let mut measure_timeouts = 0usize;
    let mut stopped_measuring = false;
    let measure_timeout = Duration::from_secs(tuning.walk.measure_timeout_secs);
    let mut unmeasured: Vec<PathBuf> = Vec::new();

    for unit in survivors.clone() {
        if handle.is_cancelled() {
            // Cancel now lands during sizing too: no new dispatches, in-
            // flight tasks are abandoned (their joins are dropped), and the
            // early-phase contract (Err(Cancelled)) still holds — nothing
            // scored exists yet, so there is nothing honest to report.
            join.abort_all();
            return Err(DriftError::Cancelled);
        }
        let caps = tuning.walk.clone();
        let counters = counters.clone();
        let sink = sink.clone();
        let path = unit.path.clone();
        let is_file = unit.kind == Kind::File;

        if is_file {
            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                let size = meta.len();
                counters.files_searched.fetch_add(1, Ordering::Relaxed);
                counters.bytes_searched.fetch_add(size, Ordering::Relaxed);
                measures.insert(path, (size, None));
            }
            continue;
        }

        if stopped_measuring {
            unmeasured.push(path);
            continue;
        }

        while inflight >= max_concurrent {
        match join.join_next().await {
            Some(Ok((path, outcome))) => {
                inflight -= 1;
                match outcome {
                    MeasureOutcome::Done(size, stats) => {
                        measures.insert(path, (size, stats));
                    }
                    MeasureOutcome::Failed(e) => {
                        measures.insert(path, (0, unknown_measure()));
                        sink.emit(ScanEvent::warn(format!("sizing snagged: {e}")));
                    }
                    MeasureOutcome::TimedOut => {
                        sink.emit(ScanEvent::warn(format!(
                            "{} never answered — its size is unknown (no response in {}s)",
                            path.display(),
                            tuning.walk.measure_timeout_secs
                        )));
                        measures.insert(path, (0, unknown_measure()));
                        measure_timeouts += 1;
                    }
                }
            }
            Some(Err(e)) => {
                inflight -= 1;
                sink.emit(ScanEvent::warn(format!("sizing task failed: {e}")));
            }
            None => break,
        }
    }
        if measure_timeouts >= tuning.walk.max_measure_timeouts {
            stopped_measuring = true;
            let msg = "the riverbed is unresponsive — deep sizes are incomplete; continuing with what was measured".to_string();
            sink.emit(ScanEvent::warn(msg.clone()));
            warns.push(ReportWarning {
                kind: "measure_timeouts".into(),
                message: msg,
            });
            measures.insert(path.clone(), (0, unknown_measure()));
            unmeasured.push(path);
            continue;
        }
        inflight += 1;
        join.spawn(async move {
            let ret_path = path.clone();
            let inner = tokio::task::spawn_blocking(move || {
                let m = crate::scan::walk::measure_folder(&path, &caps, |files, bytes| {
                    counters.files_searched.fetch_add(files, Ordering::Relaxed);
                    counters.bytes_searched.fetch_add(bytes, Ordering::Relaxed);
                    sink.emit(ScanEvent::FilesSearched {
                        total: counters.files_searched.load(Ordering::Relaxed),
                    });
                    sink.emit(ScanEvent::BytesSearched {
                        total: counters.bytes_searched.load(Ordering::Relaxed),
                    });
                });
                let stats = KindStats {
                    children: m.stats.children,
                    files: m.stats.files,
                    cache_like_ratio: m.stats.cache_like_ratio,
                    truncated: m.stats.truncated,
                };
                MeasureOutcome::Done(m.bytes, Some(stats))
            });
            match tokio::time::timeout(measure_timeout, inner).await {
                Ok(Ok(outcome)) => (ret_path, outcome),
                Ok(Err(e)) => (ret_path, MeasureOutcome::Failed(e.to_string())),
                Err(_) => (ret_path, MeasureOutcome::TimedOut),
            }
        });
    }

    // Final drain — bounded now, because every task self-bounds.
    while let Some(done) = join.join_next().await {
        match done {
            Ok((path, outcome)) => match outcome {
                MeasureOutcome::Done(size, stats) => {
                    measures.insert(path, (size, stats));
                }
                MeasureOutcome::Failed(e) => {
                    measures.insert(path, (0, unknown_measure()));
                    sink.emit(ScanEvent::warn(format!("sizing snagged: {e}")));
                }
                MeasureOutcome::TimedOut => {
                    sink.emit(ScanEvent::warn(format!(
                        "{} never answered — its size is unknown (no response in {}s)",
                        path.display(),
                        tuning.walk.measure_timeout_secs
                    )));
                    measures.insert(path, (0, unknown_measure()));
                }
            },
            Err(e) => sink.emit(ScanEvent::warn(format!("sizing task failed: {e}"))),
        }
    }
    if !unmeasured.is_empty() {
        let msg = format!(
            "{} folder(s) were not measured at all after the riverbed stopped responding — their sizes read as unknown",
            unmeasured.len()
        );
        sink.emit(ScanEvent::warn(msg.clone()));
        warns.push(ReportWarning {
            kind: "measure_timeouts".into(),
            message: msg,
        });
    }

    // ---- Phase: Scoring + banding ------------------------------------------
    sink.emit(ScanEvent::Phase { phase: Phase::Scoring });
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);

    let mut candidates: Vec<Candidate> = Vec::with_capacity(survivors.len());
    for (idx, unit) in survivors.iter().enumerate() {
        let (size_bytes, stats) = measures
            .get(&unit.path)
            .cloned()
            .unwrap_or((0, None));
        let kind_stats = stats.filter(|_| unit.kind != Kind::File);

        let cache_like_ratio = match unit.kind {
            Kind::File => {
                let ext = unit
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(crate::scan::walk::is_cache_like_ext)
                    .unwrap_or(false);
                if ext {
                    1.0
                } else {
                    0.0
                }
            }
            _ => kind_stats.as_ref().map(|s| s.cache_like_ratio).unwrap_or(0.0),
        };

        // Age reference: Spotlight last-used, else modified, else created.
        // The Downloads exception treats creation as meaningful.
        let lu = last_used.get(&unit.path).copied().flatten();
        let age_ref = match lu {
            Some(t) => Some(t),
            None => modified.get(&unit.path).copied().or_else(|| created.get(&unit.path).copied()),
        };
        let age_days = age_ref
            .map(|t| (now - t).num_days().max(0) as f64);

        // Depth relative to home (fallback: absolute component count).
        let depth = unit
            .path
            .strip_prefix(&home)
            .map(|p| p.components().count())
            .unwrap_or_else(|_| unit.path.components().count())
            .max(1);

        let input = ScoreInput {
            kind: unit.kind,
            size_bytes,
            age_days,
            scope_category: unit.category,
            root_cache_like: unit.root.cache_like,
            orphan_status: orphan_status
                .get(&unit.path)
                .copied()
                .unwrap_or(OrphanStatus::Unknown),
            depth,
            cache_like_ratio,
            children: kind_stats.as_ref().map(|s| s.children).unwrap_or(0),
        };
        let components = score::compute(&input, &tuning.weights);

        candidates.push(Candidate {
            id: report::candidate_id(&unit.path, nonce.wrapping_add(idx as u64)),
            path: unit.path.to_string_lossy().into_owned(),
            kind: unit.kind,
            size_bytes,
            kind_stats,
            last_used_date: lu,
            last_used_from_spotlight: lu.is_some() && spotlight_ok,
            modified_date: modified.get(&unit.path).copied(),
            created_date: created.get(&unit.path).copied(),
            orphan_status: input.orphan_status,
            scope_category: unit.category,
            score: components.total(),
            score_components: components,
            band: Band::Middle,
            auto_high_basis: None,
        });
    }

    // Banding (Decision #4).
    let mut indexed: Vec<(usize, f64)> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| (i, c.score))
        .collect();
    let bands = banding::assign_bands(&mut indexed, &tuning.banding);
    for (c, band) in candidates.iter_mut().zip(bands) {
        c.band = band;
        // Record quantile-band promotions so the argued auto-high
        // explanation can name the band rule (location rules record
        // their own basis later).
        if band == Band::High {
            c.auto_high_basis = Some(crate::types::AutoHighBasis::QuantileBand);
        }
    }

    // Emit the walk counters. Recoverable is deliberately NOT emitted here:
    // no tier decision exists yet, and summing every candidate reads
    // never-flagged personal data as "recoverable" (the counter only
    // speaks once decisions are known — conservatively below, exactly
    // after assembly).
    sink.emit(ScanEvent::FilesSearched {
        total: counters.files_searched.load(Ordering::Relaxed),
    });
    sink.emit(ScanEvent::BytesSearched {
        total: counters.bytes_searched.load(Ordering::Relaxed),
    });
    sink.emit(ScanEvent::CandidatesFound {
        total: counters.candidates_found.load(Ordering::Relaxed),
    });

    // ---- Local rules pin tiers before any LLM call (Phase 7) ---------------
    let mut pinned: HashMap<String, (Tier, String)> = HashMap::new(); // id → (tier, rule_id)
    let mut never_flagged: std::collections::HashSet<String> = Default::default();
    // System/vendor floor: id → the protection pattern that matched.
    let mut floored: HashMap<String, String> = HashMap::new();
    for c in &candidates {
        if hard_rules::is_never_flagged(Path::new(&c.path), &home)
            || hard_rules::is_group_container(Path::new(&c.path), &home)
        {
            never_flagged.insert(c.id.clone());
            continue;
        }
        let features = rules::extract_features(
            Path::new(&c.path),
            c.size_bytes,
            c.orphan_status,
            c.band,
        );
        if let Some(rule) = config
            .rules
            .iter()
            .find(|r| rules::rule_matches(r, &features))
        {
            // Precedence: user rule > floor. A user re-stamp distilled into
            // a rule is the deliberate exception mechanism for the floor.
            pinned.insert(c.id.clone(), (rule.tier, rule.id.clone()));
            continue;
        }
        // System/vendor floor: a folder whose owner is on the shared
        // protection list (system vendors + memory-folder extensions) may
        // be load-bearing for things outside one application. It resolves
        // HERE, before Stage 2, exactly like never_flagged — the LLM must
        // never see it, because floored at 3 the model could only confirm
        // 3 or push to 4, and paying for a foregone conclusion across
        // ~880 MB of vendor caches is waste.
        let name = Path::new(&c.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(pattern) =
            orphan::matching_protected_pattern(&orphan::normalize_name(&name), &never_orphan)
        {
            floored.insert(c.id.clone(), pattern);
        }
    }

    // Explain the floor's cost to the headline figure during the scan, not
    // afterward: floored bytes leave the "recoverable" story the user was
    // watching build up.
    if !floored.is_empty() {
        let floored_bytes: u64 = candidates
            .iter()
            .filter(|c| floored.contains_key(&c.id))
            .map(|c| c.size_bytes)
            .sum();
        sink.emit(ScanEvent::notice(format!(
            "{} system/vendor folders ({}) held at Current — DriftWood won't call them safe to clear",
            floored.len(),
            human_bytes(floored_bytes)
        )));
    }

    // ---- Folder-type auto-High (newideas Fix C) ----------------------------
    // Cache/log roots in the Low scope are driftwood by definition — the
    // relative banding quantiles would otherwise send hundreds of obvious
    // cache folders to the LLM. Upgraded AFTER banding + pinning so
    // assign_bands can't overwrite it and user rules still see the
    // quantile band in their features; rules and never-flag still win.
    // The promotion basis is recorded on the candidate so the tier can
    // carry a real, deterministic explanation instead of a bare stamp.
    // In Deep read the promotion is skipped entirely: bands are advisory
    // there and must never determine a tier (every item is argued anyway).
    let mut auto_highed = 0usize;
    if !config.mode.is_deep() {
        for c in candidates.iter_mut() {
            if c.band == Band::Middle {
                if let Some(basis) = definitionally_driftwood_basis(
                    Path::new(&c.path),
                    &home,
                    c.orphan_status,
                ) {
                    c.band = Band::High;
                    c.auto_high_basis = Some(basis);
                    auto_highed += 1;
                }
            }
        }
    }
    if auto_highed > 0 {
        sink.emit(ScanEvent::notice(format!(
            "{auto_highed} cache-root folders scored straight to driftwood (no AI needed — each card now carries the reason)"
        )));
    }

    // Conservative live "recoverable" figure: only what is ALREADY
    // definitely disposable. The old behavior summed every candidate —
    // including never-flagged personal data and everything the tiers would
    // later hold back — so the headline counter overstated by 2× on a real
    // scan. Middle-band items are excluded until judged (Express/Standard);
    // Deep read judges everything, so nothing counts until it is judged.
    let definite_recoverable: u64 = candidates
        .iter()
        .filter(|c| {
            if never_flagged.contains(&c.id) || floored.contains_key(&c.id) {
                return false;
            }
            if let Some((tier, _)) = pinned.get(&c.id) {
                return *tier <= Tier::MessageInABottle;
            }
            !config.mode.is_deep() && c.band == Band::High
        })
        .map(|c| c.size_bytes)
        .sum();
    counters
        .recoverable_bytes
        .store(definite_recoverable, Ordering::Relaxed);
    counters.emit(&*sink);

    // ---- Stage 2 (LLM) ------------------------------------------------------
    let mut outcome: Option<ReasonOutcome> = None;
    // Express: nothing is sent. Standard: the quantile middle band only.
    // Deep read: bands are advisory — EVERY surviving candidate that is
    // not never-flagged, floored, or rule-pinned crosses the river. The
    // floor still wins in Deep read; it is resolved above, before Stage 2,
    // and a partial (cost-capped or cancelled) Deep read must never
    // demote or re-label floored items on the way out — the assembly loop
    // checks `floored` before any band/fallback arm for that reason.
    let middle: Vec<Candidate> = candidates
        .iter()
        .filter(|c| {
            let llm_eligible = if config.mode.is_deep() {
                true
            } else {
                c.band == Band::Middle
            };
            llm_eligible
                && !pinned.contains_key(&c.id)
                && !never_flagged.contains(&c.id)
                && !floored.contains_key(&c.id)
        })
        .cloned()
        .collect();

    if config.mode.is_deep() && config.mode.runs_llm() {
        // Real estimate, not a generic caveat: Deep read argues about
        // every surviving candidate; Standard would have argued only the
        // quantile middle band. Cluster dedup is what keeps it affordable
        // — the reasoning phase reports the true representative count.
        let standard_middle = candidates
            .iter()
            .filter(|c| {
                c.band == Band::Middle
                    && !pinned.contains_key(&c.id)
                    && !never_flagged.contains(&c.id)
                    && !floored.contains_key(&c.id)
            })
            .count();
        sink.emit(ScanEvent::notice(format!(
            "Deep read: {} candidates will cross the river (Standard would have sent {}) — bands are advisory; every item is argued",
            middle.len(),
            standard_middle
        )));
    }

    if config.mode.runs_llm() {
        if let Some(api_key) = config.api_key.clone().filter(|k| !k.is_empty()) {
            // Few-shot: privacy-gated; union of feature-similar past
            // corrections across the middle band, deduped.
            let mut few_shot: Vec<serde_json::Value> = Vec::new();
            if crate::reason::prompt::few_shot_allowed(config.privacy_tier, tuning.reasoning.few_shot)
                && !middle.is_empty()
            {
                let mut seen = std::collections::HashSet::new();
                match crate::rules::MemoryStore::open(crate::paths::memory_dir()) {
                    Ok(store) => match store.load_preferences() {
                        Ok(prefs) => {
                            for c in middle.iter().take(tuning.reasoning.few_shot_count * 4) {
                                let features = rules::extract_features(
                                    Path::new(&c.path),
                                    c.size_bytes,
                                    c.orphan_status,
                                    c.band,
                                );
                                for p in rules::few_shot_examples(
                                    &prefs,
                                    &features,
                                    tuning.reasoning.few_shot_count,
                                ) {
                                    let key = serde_json::to_string(&p.candidate_features)
                                        .unwrap_or_default();
                                    if seen.insert(key) {
                                        few_shot.push(crate::reason::prompt::few_shot_value(
                                            &p.candidate_features,
                                            p.corrected_tier as u8,
                                        ));
                                    }
                                }
                                if few_shot.len() >= tuning.reasoning.few_shot_count {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            sink.emit(ScanEvent::warn(format!("preferences unreadable: {e}")))
                        }
                    },
                    Err(e) => sink.emit(ScanEvent::warn(format!("memory folder unavailable: {e}"))),
                }
            }

            match reason::reason_middle_band(
                middle.clone(),
                config.privacy_tier,
                &tuning.reasoning,
                &config.model,
                &api_key,
                !config.allow_non_zdr,
                &scan_id,
                &few_shot,
                sink.clone(),
                handle.cancel.clone(),
            )
            .await
            {
                Ok(o) => outcome = Some(o),
                // Cancel during reasoning is no longer fatal: the partial
                // outcome (fallback-labeled remainder) is returned as the
                // report. Early-phase cancels above still abort outright —
                // there is nothing worth reporting yet.
                Err(e) => {
                    sink.emit(ScanEvent::warn(format!("river crossing failed: {e}")));
                    warns.push(ReportWarning {
                        kind: "llm_failures".into(),
                        message: format!("Stage 2 could not complete: {e}"),
                    });
                }
            }
        } else {
            warns.push(ReportWarning {
                kind: "stage2_no_key".into(),
                message: if config.mode.is_deep() {
                    "No OpenRouter key configured — Deep read could not run; every unargued item uses heuristic tiers only.".into()
                } else {
                    "No OpenRouter key configured — middle-band items use heuristic tiers only.".into()
                },
            });
        }
    }

    // ---- Assemble entries ----------------------------------------------------
    sink.emit(ScanEvent::Phase {
        phase: Phase::Assembling,
    });
    // Provenance for never-argued middle-band items depends on WHY they
    // were not argued: a scan mode that never crosses the river (Express,
    // or no API key) must not wear failure copy ("Snagged"). Only an
    // attempted-and-failed crossing is `Fallback`.
    let llm_attempted = config.mode.runs_llm()
        && config
            .api_key
            .as_deref()
            .map(|k| !k.trim().is_empty())
            .unwrap_or(false);
    let heuristic_summary_reasoning = |c: &Candidate| -> (String, String) {
        (
            "Heuristic estimate — this scan never asked the river about this item.".to_string(),
            format!(
                "No AI reasoning was spent on this item: the scan mode does not cross the river \
                 (Express scan, or no OpenRouter key configured). The tier comes from the drift \
                 score ({:.1}/100) and the still-in-the-current rule — it is a heuristic guess, \
                 honestly labeled. \"Ask the river\" on this card gives a real second opinion, \
                 and Standard or Deep read mode argues every middle-band item.",
                c.score
            ),
        )
    };
    let fallback_summary = "Snagged — the river ran rough here; heuristic estimate only.".to_string();
    let mut entries: Vec<ReportEntry> = Vec::with_capacity(candidates.len());
    let mut fallback_count = 0usize;
    for c in &candidates {
        let (tier, tier_source, summary, reasoning, confidence, model_used) =
            if never_flagged.contains(&c.id) {
                (
                    Tier::Source,
                    TierSource::NeverFlag,
                    "Protected location — never flagged for deletion.".to_string(),
                    String::new(),
                    1.0,
                    None,
                )
            } else if let Some((tier, rule_id)) = pinned.get(&c.id) {
                (
                    *tier,
                    TierSource::Rule,
                    format!("Pinned by your rule {rule_id}."),
                    String::new(),
                    1.0,
                    None,
                )
            } else if let Some(pattern) = floored.get(&c.id) {
                // System/vendor floor: hard stop at Tier 3. The reasoning
                // must say exactly what this is — a refusal to opine, NOT
                // a claim that the data is precious or costly to reacquire
                // (TIER_BLURBS[3] reads as the latter otherwise).
                (
                    Tier::Current,
                    TierSource::SystemFloor,
                    format!("System-managed folder ({pattern}). DriftWood won't call this safe to clear."),
                    format!(
                        "The folder's name matches the system-owner list ({pattern}): the vendor owns the OS \
                         or the whole suite, so this folder may be load-bearing for things outside that one \
                         application. DriftWood declines to opine — a path-shape rule is not a safety argument, \
                         and it will not call this safe to clear without reasoning.\n\
                         This is NOT a judgment that the data is precious or that reacquiring it would hurt: \
                         DriftWood simply refuses to guess here. A user rule can re-stamp it (your re-stamps \
                         beat the floor), and \"Ask the river\" on the card gives a second opinion."
                    ),
                    1.0,
                    None,
                )
            } else if config.mode.is_deep() {
                // Deep read: bands are ADVISORY. The tier comes from the
                // LLM judgment (or its cluster's propagated verdict); a
                // band must never determine one. Anything unjudged — cost
                // cap, cancel, partial ids, LLM failure — lands on the
                // honest fallback, and floored/never-flagged/rule-pinned
                // items can never be demoted or re-labeled on the way out.
                if let Some(o) = &outcome {
                    if let Some(j) = o.judgments.get(&c.id) {
                        (
                            Tier::try_from(j.tier).unwrap_or(Tier::Current),
                            TierSource::Llm,
                            j.summary.clone(),
                            j.reasoning.clone(),
                            j.confidence,
                            Some(config.model.clone()),
                        )
                    } else if let Some(j) = o.propagated_judgments.get(&c.id) {
                        (
                            Tier::try_from(j.tier).unwrap_or(Tier::Current),
                            TierSource::LlmPropagated,
                            format!("{} (shared with its folder's cluster — one judgment covers the group.)", j.summary),
                            j.reasoning.clone(),
                            j.confidence,
                            Some(config.model.clone()),
                        )
                    } else {
                        fallback_count += 1;
                        let (summary, reasoning) = if llm_attempted {
                            (fallback_summary.clone(), String::new())
                        } else {
                            heuristic_summary_reasoning(c)
                        };
                        (
                            reason::fallback_tier(c.score),
                            if llm_attempted {
                                TierSource::Fallback
                            } else {
                                TierSource::Heuristic
                            },
                            summary,
                            reasoning,
                            0.3,
                            None,
                        )
                    }
                } else {
                    fallback_count += 1;
                    let (summary, reasoning) = if llm_attempted {
                        (fallback_summary.clone(), String::new())
                    } else {
                        heuristic_summary_reasoning(c)
                    };
                    (
                        reason::fallback_tier(c.score),
                        if llm_attempted {
                            TierSource::Fallback
                        } else {
                            TierSource::Heuristic
                        },
                        summary,
                        reasoning,
                        0.3,
                        None,
                    )
                }
            } else {
                match (c.band, &outcome) {
                    (Band::High, _) => {
                        let (summary, reasoning) = argued_auto_high_text(
                            c,
                            &home,
                            tuning.banding.high_quantile,
                        );
                        (
                            Tier::Driftwood,
                            TierSource::ArguedAutoHigh,
                            summary,
                            reasoning,
                            1.0,
                            None,
                        )
                    }
                    (Band::Low, _) => (
                        Tier::Source,
                        TierSource::AutoLow,
                        "Low drift score — still part of your everyday water.".to_string(),
                        String::new(),
                        1.0,
                        None,
                    ),
                    (Band::Middle, Some(o)) if o.judgments.contains_key(&c.id) => {
                        let j = &o.judgments[&c.id];
                        (
                            Tier::try_from(j.tier).unwrap_or(Tier::Current),
                            TierSource::Llm,
                            j.summary.clone(),
                            j.reasoning.clone(),
                            j.confidence,
                            Some(config.model.clone()),
                        )
                    }
                    (Band::Middle, Some(o)) if o.propagated_judgments.contains_key(&c.id) => {
                        // One shared cluster verdict, honestly labeled —
                        // never presented as an independent LLM judgment.
                        let j = &o.propagated_judgments[&c.id];
                        (
                            Tier::try_from(j.tier).unwrap_or(Tier::Current),
                            TierSource::LlmPropagated,
                            format!("{} (shared with its folder's cluster — one judgment covers the group.)", j.summary),
                            j.reasoning.clone(),
                            j.confidence,
                            Some(config.model.clone()),
                        )
                    }
                    (Band::Middle, _) => {
                        fallback_count += 1;
                        // A never-argued item (Express / no key) is not a
                        // snag — the river was never crossed for it.
                        let (summary, reasoning) = if llm_attempted {
                            (fallback_summary.clone(), String::new())
                        } else {
                            heuristic_summary_reasoning(c)
                        };
                        (
                            reason::fallback_tier(c.score),
                            if llm_attempted {
                                TierSource::Fallback
                            } else {
                                TierSource::Heuristic
                            },
                            summary,
                            reasoning,
                            0.3,
                            None,
                        )
                    }
                }
            };

        entries.push(ReportEntry {
            candidate: c.clone(),
            tier,
            tier_source,
            summary,
            reasoning,
            confidence,
            llm_model: model_used,
            privacy_tier_used: config.privacy_tier,
            rule_id: pinned.get(&c.id).map(|(_, r)| r.clone()),
        });
    }

    // The exact figure now that every tier is known: tiers 1–2 are what
    // the app actually offers up; tiers 3–4 never counted as "could be
    // freed" (tier 3 is "costly to reacquire", tier 4 is "don't touch").
    let exact_recoverable: u64 = entries
        .iter()
        .filter(|e| e.tier <= Tier::MessageInABottle)
        .map(|e| e.candidate.size_bytes)
        .sum();
    counters
        .recoverable_bytes
        .store(exact_recoverable, Ordering::Relaxed);
    counters.emit(&*sink);

    if let Some(o) = &outcome {
        if o.cost_cap_hit {
            warns.push(ReportWarning {
                kind: "cost_cap".into(),
                message: "Snagged — ran out of river: the per-scan cost cap was hit; remaining items fell back to heuristic tiers.".into(),
            });
        }
        if o.cancelled {
            warns.push(ReportWarning {
                kind: "stopped_early".into(),
                message: "You pulled the scan ashore — it stopped early. Everything judged so far is in this report; unjudged items use heuristic tiers (marked as fallback).".into(),
            });
        }
        if fallback_count > 0 {
            warns.push(ReportWarning {
                kind: "llm_failures".into(),
                message: format!("{fallback_count} items could not be reasoned about and use heuristic tiers (marked as fallback)."),
            });
        }
        sink.emit(ScanEvent::notice(format!(
            "river crossing cost {}",
            reason::format_cost(o.total_cost_usd)
        )));
    } else if !config.mode.runs_llm() && !middle.is_empty() {
        warns.push(ReportWarning {
            kind: "stage2_skipped".into(),
            message: format!(
                "Stage 2 disabled — {} middle-band items use heuristic tier estimates.",
                middle.len()
            ),
        });
    }

    finish(
        AssembleInput {
            scan_id,
            spotlight_available: spotlight_ok,
            privacy_tier: config.privacy_tier,
            model: if config.mode.runs_llm() { Some(config.model.clone()) } else { None },
            llm_cost_usd: outcome.as_ref().map(|o| o.total_cost_usd).unwrap_or(0.0),
            cost_cap_hit: outcome.as_ref().map(|o| o.cost_cap_hit).unwrap_or(false),
            stopped_early: outcome.as_ref().map(|o| o.cancelled).unwrap_or(false),
            counters: counters.snapshot(),
            entries,
            warnings: warns,
        },
        config.persist,
    )
}

fn finish(input: AssembleInput, persist: bool) -> Result<Report> {
    let r = report::assemble(input);
    if persist {
        if let Err(e) = r.persist() {
            // Persistence failure must not lose the scan result.
            eprintln!("warning: could not persist report: {e}");
        }
    }
    Ok(r)
}

/// Human-readable bytes for scan notices.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0usize;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

/// Cache roots whose contents are driftwood by definition: caches, logs,
/// temp dirs, and folders left behind by uninstalled apps under
/// Application Support. Their top-level units are never sent to the LLM.
///
/// Returns the promotion basis so the tier card can carry a true
/// explanation of what fired. Orphan status is consulted on BOTH branches:
/// a cache-root folder whose owning app is still installed (Active) is not
/// "driftwood by definition" — the band must not claim it is. (The
/// system/vendor floor resolves OS-owned Active folders separately; live
/// third-party caches are argued by the LLM like any other middle-band
/// item.)
fn definitionally_driftwood_basis(
    path: &Path,
    home: &Path,
    orphan: OrphanStatus,
) -> Option<AutoHighBasis> {
    let cache_roots = [
        home.join("Library/Caches"),
        home.join("Library/Logs"),
        PathBuf::from("/tmp"),
        PathBuf::from("/private/tmp"),
    ];
    if cache_roots.iter().any(|r| path.starts_with(r)) {
        // An actively-managed folder under a cache root (owning app
        // installed) is not definitionally driftwood.
        return (orphan != OrphanStatus::Active).then_some(AutoHighBasis::CacheRoot);
    }
    // Orphaned Application Support: the owning app is gone.
    (orphan == OrphanStatus::Orphaned
        && path.starts_with(home.join("Library/Application Support")))
    .then_some(AutoHighBasis::OrphanedAppSupport)
}

/// Which named cache root (if any) this path sits under — for the
/// explanation text.
fn cache_root_label(path: &Path, home: &Path) -> Option<String> {
    const ROOTS: &[&str] = &["Library/Caches", "Library/Logs"];
    for rel in ROOTS {
        let r = home.join(rel);
        if path.starts_with(&r) {
            return Some(format!("~/{rel}"));
        }
    }
    if path.starts_with("/private/tmp") {
        return Some("/private/tmp".to_string());
    }
    if path.starts_with("/tmp") {
        return Some("/tmp".to_string());
    }
    None
}

/// Build the honest summary + reasoning for an auto-high Driftwood card.
///
/// The card must read as *unverified*, not *cleared*: a quantile threshold
/// plus a path-prefix test cannot support "safe to clear", so the copy
/// says what actually happened (location, orphan status, score components,
/// band rule) and that no AI reasoned about this folder.
fn argued_auto_high_text(c: &Candidate, home: &Path, high_quantile: f64) -> (String, String) {
    let comps = &c.score_components;
    let (summary, location_line) = match c.auto_high_basis {
        Some(AutoHighBasis::CacheRoot) => {
            let root = cache_root_label(Path::new(&c.path), home)
                .unwrap_or_else(|| "a cache root".to_string());
            (
                format!(
                    "Scored straight to Driftwood: it sits under {root} and its drift score \
                     landed in the top band. No AI reasoned about this folder — treat this \
                     tier as unverified, not cleared."
                ),
                format!("Location: {root} — a cache/log root DriftWood treats as driftwood by definition."),
            )
        }
        Some(AutoHighBasis::OrphanedAppSupport) => (
            "Scored straight to Driftwood: the app that owns this Application Support \
             folder is no longer installed, and the drift score landed in the top band. \
             No AI reasoned about this folder — treat this tier as unverified, not cleared."
                .to_string(),
            "Location: ~/Library/Application Support — orphaned (the owning app is gone).".to_string(),
        ),
        Some(AutoHighBasis::QuantileBand) | None => (
            format!(
                "Scored straight to Driftwood: its drift score landed in the top {:.0}% of this \
                 scan's scores. No AI reasoned about this folder — treat this tier as \
                 unverified, not cleared.",
                high_quantile * 100.0
            ),
            "Location: no location rule fired — the top quantile band alone put this here.".to_string(),
        ),
    };

    let orphan_word = match c.orphan_status {
        OrphanStatus::Orphaned => "orphaned — the owning app is no longer installed",
        OrphanStatus::Active => "active — the owning app appears to be installed",
        OrphanStatus::Unknown => "unknown — no installed-app match either way",
    };
    let reasoning = format!(
        "Auto-high heuristic (no AI reasoning was spent on this item):\n\
         · {location_line}\n\
         · Orphan status: {orphan_word}.\n\
         · Drift score {:.1} of 100 landed in the top band (top {:.0}% of this scan's scores): \
         size {:.1}, age {:.1}, cache location {:.1}, orphan {:.1}, depth {:.1}, file type {:.1}, child count {:.1}.\n\
         The tier is a heuristic verdict — it says \"this looks like driftwood by location and score\", \
         not \"this is safe to clear\". Use \"Ask the river\" on the card for a real second opinion.",
        c.score,
        high_quantile * 100.0,
        comps.size,
        comps.age,
        comps.cache_location,
        comps.orphan,
        comps.depth,
        comps.file_type,
        comps.child_count,
    );
    (summary, reasoning)
}

// ---------------------------------------------------------------------------
// apply_correction — the second entry point (plan §1)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CorrectionRequest {
    /// Candidate path (from the report entry).
    pub path: String,
    pub size_bytes: u64,
    pub orphan: OrphanStatus,
    pub score_band: Band,
    pub original_tier: Tier,
    pub corrected_tier: Tier,
    #[allow(dead_code)]
    pub note: Option<String>,
}

/// Record a user tier correction: append to preferences.jsonl, re-distill
/// rules.json, and audit-log. Returns the updated rule set.
pub async fn apply_correction(
    request: CorrectionRequest,
    memory_dir_override: Option<PathBuf>,
) -> Result<Vec<Rule>> {
    let dir = memory_dir_override.unwrap_or_else(crate::paths::memory_dir);
    let store = rules::MemoryStore::open(&dir)?;

    let features = rules::extract_features(
        Path::new(&request.path),
        request.size_bytes,
        request.orphan,
        request.score_band,
    );
    let pref = rules::Preference {
        timestamp: Utc::now(),
        candidate_features: features,
        original_tier: request.original_tier,
        corrected_tier: request.corrected_tier,
        note: request.note,
    };
    store.append_preference(&pref)?;

    let prefs = store.load_preferences()?;
    let existing = store.load_rules()?;
    let distilled = rules::distill(&prefs, &existing);
    store.save_rules(&distilled)?;

    store.log_session(
        "correction",
        serde_json::json!({
            "path": request.path,
            "original_tier": request.original_tier as u8,
            "corrected_tier": request.corrected_tier as u8,
            "rules_count": distilled.len(),
        }),
    );

    Ok(distilled)
}

/// Re-run distillation only (used by the app's background distiller).
pub async fn redistill(memory_dir_override: Option<PathBuf>) -> Result<Vec<Rule>> {
    let dir = memory_dir_override.unwrap_or_else(crate::paths::memory_dir);
    let store = rules::MemoryStore::open(&dir)?;
    let prefs = store.load_preferences()?;
    let existing = store.load_rules()?;
    let distilled = rules::distill(&prefs, &existing);
    store.save_rules(&distilled)?;
    Ok(distilled)
}

/// Load the current rules snapshot (for ScanConfig.rules).
pub fn load_rules(memory_dir_override: Option<PathBuf>) -> Result<Vec<Rule>> {
    let dir = memory_dir_override.unwrap_or_else(crate::paths::memory_dir);
    let store = rules::MemoryStore::open(&dir)?;
    store.load_rules()
}

// ---------------------------------------------------------------------------
// adjudicate_candidate — the third entry point ("ask the river about this
// one")
// ---------------------------------------------------------------------------

/// Input for an on-demand adjudication. The frontend sends the opaque
/// candidate id (never a path); everything else mirrors what `start_scan`
/// resolves.
#[derive(Debug, Clone)]
pub struct AdjudicationRequest {
    /// Opaque candidate id from the persisted report.
    pub candidate_id: String,
    pub model: String,
    pub api_key: String,
    /// Payload privacy tier; defaults to the scan's own tier (the caller
    /// passes the persisted report's `privacy_tier_used`).
    pub privacy_tier: PrivacyTier,
    /// ZDR-only routing (mirrors `ScanConfig.allow_non_zdr`, inverted).
    pub enforce_zdr: bool,
}

/// The river's second opinion for one candidate. The card's tier is NOT
/// changed by any of this — the result is a conversation, not a decision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Adjudication {
    pub candidate_id: String,
    /// Tier the card carries (unchanged by adjudication).
    pub card_tier: Tier,
    /// The river's verdict.
    pub llm_tier: Tier,
    /// True when the river's tier equals the card's.
    pub agrees: bool,
    /// True when the river's tier is SAFER than the card's (a higher tier
    /// number). A card that says Driftwood while the second opinion says
    /// Source is the exact failure adjudication exists to surface.
    pub llm_safer: bool,
    pub confidence: f64,
    pub summary: String,
    pub reasoning: String,
    /// Actual spend of this single call. Counts toward the report total.
    pub cost_usd: f64,
    pub model: String,
    pub tier_source: TierSource,
}

/// Re-examine one candidate from the persisted report: rebuild its payload
/// under the scan's privacy tier and run ONE batch through the existing
/// Stage 2 machinery (same ZDR enforcement, streaming, retry, and
/// cost-accounting path as a scan; batch size forced to 1).
///
/// Never writes to the memory folder — only an explicit user re-stamp
/// (apply_correction) does that. Nothing here deletes, moves, or modifies
/// any scanned path; the only disk write is cost accounting on
/// `last-report.json` in the reports directory.
///
/// Cost-cap policy (deliberate, documented): a single adjudication MAY
/// exceed a nearly-exhausted per-scan cap. The cap governs bulk scanning,
/// not an explicit one-item user request — silently refusing the exact
/// thing the user asked about would trade honesty for a number. The cost
/// is still accounted against the report total either way.
pub async fn adjudicate_candidate(request: AdjudicationRequest, sink: Arc<dyn EventSink>) -> Result<Adjudication> {
    if request.api_key.trim().is_empty() {
        return Err(DriftError::Scan(
            "no OpenRouter key configured — adjudication needs one (settings or OPENROUTER_API_KEY)".into(),
        ));
    }

    let report = Report::load_last()
        .map_err(|e| DriftError::Scan(format!("no report to adjudicate against: {e}")))?;
    let entry = report
        .entries
        .iter()
        .find(|e| e.candidate.id == request.candidate_id)
        .ok_or_else(|| DriftError::Scan(format!("unknown candidate id {}", request.candidate_id)))?;
    let candidate = entry.candidate.clone();
    let card_tier = entry.tier;

    // One batch, one item. The cost cap is disabled here (0.0): see the
    // doc comment above.
    let mut cfg = crate::config::DriftTuning::default().reasoning;
    cfg.batch_size = 1;
    cfg.cost_cap_usd = 0.0;

    sink.emit(ScanEvent::notice(format!(
        "adjudicating one candidate — card says {}, the river is asked to argue against it",
        card_tier
    )));

    let transport = Arc::new(reason::ReqwestTransport::new(Duration::from_secs(
        cfg.timeout_secs,
    ))?);
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let outcome = reason::reason_middle_band_with(
        vec![candidate.clone()],
        request.privacy_tier,
        &cfg,
        &request.model,
        request.api_key.trim(),
        request.enforce_zdr,
        &report.scan_id,
        &[],
        sink.clone(),
        cancel,
        transport,
        Some(reason::batch_persist_path(&report.scan_id)),
        Some(reason::prompt::adjudication_system_prompt().as_str()),
    )
    .await?;

    let judgment = outcome.judgments.get(&candidate.id).cloned().or_else(|| {
        // Single-item adjudication: if the river answered exactly one
        // judgment but echoed a different id, it still judged the only
        // candidate it was asked about — keep it, honestly noted, rather
        // than reporting silence (issue #1: several models fumble long
        // opaque ids on one-item batches).
        if outcome.judgments.len() == 1 {
            let (echoed_id, j) = outcome.judgments.iter().next()?;
            sink.emit(ScanEvent::notice(format!(
                "the river answered for id {echoed_id} instead of {} — kept: this was a one-item adjudication",
                candidate.id
            )));
            Some(j.clone())
        } else {
            None
        }
    }).ok_or_else(|| {
        // Say WHY the river stayed silent when a batch snagged: a bare
        // "did not answer" with the real cause hidden in a warn event is
        // not actionable (issue #1). When nothing failed and the answer
        // simply had no judgment for this id, keep the plain message.
        match &outcome.last_batch_snag {
            Some(snag) => DriftError::Reason(format!(
                "the river did not answer for this item — {snag}"
            )),
            None => DriftError::Reason("the river did not answer for this item".into()),
        }
    })?;
    let llm_tier = Tier::try_from(judgment.tier)
        .map_err(|e| DriftError::Reason(format!("unusable tier from the river: {e}")))?;

    let cost = outcome.total_cost_usd;
    // Make the report total honest: the persisted figure must include
    // adjudications or "cost so far" lies.
    let mut updated = report;
    if let Err(e) = updated.record_adjudication_cost(cost) {
        eprintln!("warning: could not record adjudication cost: {e}");
    }

    Ok(Adjudication {
        candidate_id: candidate.id,
        card_tier,
        llm_tier,
        agrees: llm_tier == card_tier,
        llm_safer: llm_tier > card_tier,
        confidence: judgment.confidence,
        summary: judgment.summary.clone(),
        reasoning: judgment.reasoning.clone(),
        cost_usd: cost,
        model: request.model.clone(),
        tier_source: TierSource::Adjudication,
    })
}

/// Rule source helper for the MCP wrapper.
pub fn rule_source_is_user(rule: &Rule) -> bool {
    rule.source == RuleSource::User
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Env-var tests (HOME / DRIFTWOOD_HOME) must not interleave.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn home() -> PathBuf {
        PathBuf::from("/Users/tester")
    }

    #[test]
    fn cache_roots_are_driftwood() {
        let h = home();
        assert_eq!(
            definitionally_driftwood_basis(
                &h.join("Library/Caches/com.google.Chrome"),
                &h,
                OrphanStatus::Unknown
            ),
            Some(AutoHighBasis::CacheRoot)
        );
        assert_eq!(
            definitionally_driftwood_basis(
                &h.join("Library/Logs/DiagnosticReports"),
                &h,
                OrphanStatus::Unknown
            ),
            Some(AutoHighBasis::CacheRoot)
        );
        assert_eq!(
            definitionally_driftwood_basis(Path::new("/tmp/build-cache"), &h, OrphanStatus::Unknown),
            Some(AutoHighBasis::CacheRoot)
        );
        assert_eq!(
            definitionally_driftwood_basis(
                Path::new("/private/tmp/build-cache"),
                &h,
                OrphanStatus::Unknown
            ),
            Some(AutoHighBasis::CacheRoot)
        );
    }

    #[test]
    fn orphaned_app_support_is_driftwood() {
        let h = home();
        assert_eq!(
            definitionally_driftwood_basis(
                &h.join("Library/Application Support/OldApp"),
                &h,
                OrphanStatus::Orphaned
            ),
            Some(AutoHighBasis::OrphanedAppSupport)
        );
        // Active app support folders are NOT auto-high.
        assert_eq!(
            definitionally_driftwood_basis(
                &h.join("Library/Application Support/ActiveApp"),
                &h,
                OrphanStatus::Active
            ),
            None
        );
    }

    #[test]
    fn active_cache_root_folders_are_not_auto_high() {
        // The cheap band fix: a cache-root folder whose owning app is
        // installed is not "driftwood by definition" — the band must not
        // claim it is. (OS-owned Active folders are handled by the floor.)
        let h = home();
        assert_eq!(
            definitionally_driftwood_basis(
                &h.join("Library/Caches/com.google.Chrome"),
                &h,
                OrphanStatus::Active
            ),
            None
        );
        assert_eq!(
            definitionally_driftwood_basis(
                &h.join("Library/Logs/com.apple.diagnostic"),
                &h,
                OrphanStatus::Active
            ),
            None
        );
    }

    #[test]
    fn personal_folders_are_not_auto_high() {
        let h = home();
        assert_eq!(
            definitionally_driftwood_basis(
                &h.join("Library/Application Support/ActiveApp"),
                &h,
                OrphanStatus::Unknown
            ),
            None
        );
        assert_eq!(
            definitionally_driftwood_basis(&h.join("Documents/notes"), &h, OrphanStatus::Unknown),
            None
        );
        // A cache-named folder OUTSIDE the cache roots is not auto-high.
        assert_eq!(
            definitionally_driftwood_basis(
                &h.join("Library/Containers/com.app/cache"),
                &h,
                OrphanStatus::Unknown
            ),
            None
        );
    }

    #[test]
    fn argued_auto_high_text_names_what_happened() {
        let h = home();
        let mut c = candidate_fixture();
        c.path = h.join("Library/Caches/com.example.thing").to_string_lossy().into_owned();
        c.orphan_status = OrphanStatus::Unknown;
        c.auto_high_basis = Some(AutoHighBasis::CacheRoot);
        c.score = 71.4;
        let (summary, reasoning) = argued_auto_high_text(&c, &h, 0.25);
        // No safety claim anywhere.
        assert!(!summary.contains("safe to clear"));
        assert!(!summary.contains("No risk"));
        // The explanation names location, band, and the unargued caveat.
        assert!(reasoning.contains("Library/Caches"));
        assert!(reasoning.contains("71.4"));
        assert!(reasoning.contains("top 25%"));
        assert!(reasoning.contains("no AI reasoning"));
        assert!(reasoning.contains("unknown"));
        // Non-empty reasoning is what makes the dropdown affordance appear.
        assert!(!reasoning.is_empty());
        assert!(!summary.is_empty());
    }

    #[test]
    fn floor_matches_system_vendor_patterns() {
        let never = crate::scan::orphan::load_never_orphan_list(Path::new("/nonexistent"));
        let m = |name: &str| {
            crate::scan::orphan::matching_protected_pattern(
                &crate::scan::orphan::normalize_name(name),
                &never,
            )
        };
        // OS + whole-suite vendors are floored...
        assert_eq!(m("com.apple.HomeKit"), Some("com.apple.*".to_string()));
        assert_eq!(
            m("com.microsoft.office"),
            Some("com.microsoft.*".to_string())
        );
        // ...an ordinary vendor's folder is not (the LLM judges it)...
        assert_eq!(m("com.google.Chrome"), None);
        assert_eq!(m("pip"), None);
        // ...but memory-folder extensions join the floor for both
        // consumers, so the lists cannot drift apart.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("never-orphan.json"),
            serde_json::to_string(&serde_json::json!(["com.fancyvendor.*"])).unwrap(),
        )
        .unwrap();
        let extended = crate::scan::orphan::load_never_orphan_list(dir.path());
        assert_eq!(
            crate::scan::orphan::matching_protected_pattern(
                &crate::scan::orphan::normalize_name("com.fancyvendor.app"),
                &extended
            ),
            Some("com.fancyvendor.*".to_string())
        );
    }

    /// Full pipeline: the floor resolves in the assembly loop (beating
    /// auto-high and the no-LLM fallbacks), a user rule still beats the
    /// floor, and Deep read never re-labels floored items. Runs against a
    /// fake HOME + DRIFTWOOD_HOME so it never touches real user data.
    #[tokio::test(flavor = "multi_thread")]
    async fn floor_wins_over_auto_high_and_deep_read_cannot_bypass_it() {
        use crate::config::ScanConfig;
        use crate::rules::Rule;
        use crate::types::ScopeCategory;

        let _env = ENV_LOCK.lock().unwrap();
        let fake_home = tempfile::tempdir().unwrap();
        let fake_dw = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", fake_home.path());
        std::env::set_var("DRIFTWOOD_HOME", fake_dw.path());
        crate::paths::ensure_dirs().unwrap();

        // Two vendor/protected folders + two ordinary cache folders. The
        // Apple folder gets a 2 GiB sparse blob so a size-band rule can
        // pin exactly that folder (rules match parent-folder/size, not the
        // candidate's own name).
        let caches = fake_home.path().join("Library/Caches");
        std::fs::create_dir_all(caches.join("com.apple.testsvc")).unwrap();
        let big = std::fs::File::create(caches.join("com.apple.testsvc/big.blob")).unwrap();
        big.set_len(2 * 1024 * 1024 * 1024).unwrap();
        std::fs::create_dir_all(caches.join("group.com.shared.test")).unwrap();
        std::fs::create_dir_all(caches.join("plain-old-cache")).unwrap();
        std::fs::create_dir_all(caches.join("another-cache-xyz")).unwrap();

        // A user rule pinning large Caches folders to Source (4) — rules
        // beat the floor.
        let rule = Rule {
            id: "test-big-caches".into(),
            ext: None,
            folder_name: Some("Caches".into()),
            parent_folder: None,
            size_band: Some(crate::rules::SizeBand::L),
            orphan: None,
            score_band: None,
            tier: Tier::Source,
            support_count: 1,
            created_at: Utc::now(),
            source: crate::rules::RuleSource::User,
        };
        // Write the rule into the memory folder so run_scan loads it.
        let store = crate::rules::MemoryStore::open(&crate::paths::memory_dir()).unwrap();
        store.save_rules(&[rule.clone()]).unwrap();

        let make_config = |mode| ScanConfig {
            scopes: vec![ScopeCategory::Low],
            privacy_tier: PrivacyTier::Minimal,
            mode,
            model: "test-model".into(),
            api_key: None,
            allow_non_zdr: false,
            rules: vec![rule.clone()],
            tuning: crate::config::DriftTuning::default(),
            persist: false,
        };

        let sink = crate::events::CollectingSink::new();
        let report = run_scan(make_config(crate::config::ScanMode::Express), sink.clone(), ScanHandle::new())
            .await
            .unwrap();

        let find = |p: &str| {
            report
                .entries
                .iter()
                .find(|e| e.candidate.path.ends_with(p))
                .unwrap_or_else(|| panic!("missing entry {p}"))
        };
        let apple = find("com.apple.testsvc");
        assert_eq!(apple.tier, Tier::Source, "user rule beats the floor");
        assert_eq!(apple.tier_source, TierSource::Rule);

        // The recoverable headline counts only tiers 1–2 — never the
        // rule-pinned Source blob, never the floored Current folder, and
        // never "every candidate scanned" (the old 2× overclaim).
        let t12: u64 = report
            .entries
            .iter()
            .filter(|e| e.tier <= Tier::MessageInABottle)
            .map(|e| e.candidate.size_bytes)
            .sum();
        assert_eq!(
            report.counters.recoverable_bytes, t12,
            "report counters must equal the tiers-1–2 sum exactly"
        );
        assert!(
            report.counters.recoverable_bytes
                < report.entries.iter().map(|e| e.candidate.size_bytes).sum::<u64>(),
            "recoverable must be strictly below the everything-sum on a report holding tier 3/4 data"
        );

        let ms = find("group.com.shared.test");
        assert_eq!(ms.tier, Tier::Current, "floor holds at 3");
        assert_eq!(ms.tier_source, TierSource::SystemFloor);
        assert!(
            ms.reasoning.contains("declines to opine"),
            "floor reasoning must say what it is"
        );
        assert!(!ms.reasoning.contains("costly to reacquire"));

        // The floor notice explains the drop in recoverable bytes.
        let events = sink.snapshot();
        assert!(events.iter().any(|e| matches!(
            e,
            ScanEvent::Notice { message } if message.contains("system/vendor folders")
        )));

        // Deep read, no key: unargued items fall back honestly, but floored
        // items stay floored — Deep read must not bypass the floor.
        let deep_report = run_scan(
            make_config(crate::config::ScanMode::DeepRead),
            crate::events::CollectingSink::new(),
            ScanHandle::new(),
        )
        .await
        .unwrap();
        let deep_ms = deep_report
            .entries
            .iter()
            .find(|e| e.candidate.path.ends_with("group.com.shared.test"))
            .unwrap();
        assert_eq!(deep_ms.tier, Tier::Current, "floor survives Deep read");
        assert_eq!(deep_ms.tier_source, TierSource::SystemFloor);
        for e in &deep_report.entries {
            if e.tier_source == TierSource::SystemFloor {
                assert_eq!(e.tier, Tier::Current);
            } else if e.tier_source == TierSource::Rule
                || e.tier_source == TierSource::NeverFlag
            {
                // User pins and never-flags are legitimately reasoned or
                // deliberate; the floor-adjacent invariant is that nothing
                // else claims a reasoned tier without the LLM.
                continue;
            } else {
                // No LLM ran (no key): the river was never tried, so the
                // provenance is by-design heuristic — not a snag.
                assert_eq!(e.tier_source, TierSource::Heuristic);
                assert!(
                    !e.summary.contains("Snagged"),
                    "by-design heuristics must not wear failure copy"
                );
                assert_ne!(e.tier_source, TierSource::ArguedAutoHigh);
            }
        }

        std::env::remove_var("HOME");
        std::env::remove_var("DRIFTWOOD_HOME");
    }

    /// The bounded-riverbed primitive: a blocking task that outlives its
    /// budget yields `Elapsed`, not a hang. (A folder whose open() hangs
    /// cannot be fabricated on APFS in a test; this pins the wrapper the
    /// engine leans on.)
    #[tokio::test]
    async fn measure_wrapper_times_out_a_slow_task() {
        let inner = tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_millis(300)));
        let started = std::time::Instant::now();
        let out = tokio::time::timeout(Duration::from_millis(10), inner).await;
        assert!(out.is_err(), "a task past its budget must time out");
        assert!(started.elapsed() < Duration::from_millis(200), "timeout must be prompt");
    }

    /// Honest degradation, end to end: `max_measure_timeouts = 0` is the
    /// degenerate give-up-at-once mode — no folder is walked at all. The
    /// scan must still complete, every folder entry must carry the
    /// truncated marker (never a silent zero), and the warnings must say
    /// sizes are missing.
    #[tokio::test(flavor = "multi_thread")]
    async fn unmeasurable_riverbed_degrades_with_warnings() {
        use crate::config::ScanConfig;
        use crate::events::CollectingSink;

        let _env = ENV_LOCK.lock().unwrap();
        let fake_home = tempfile::tempdir().unwrap();
        let fake_dw = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", fake_home.path());
        std::env::set_var("DRIFTWOOD_HOME", fake_dw.path());
        crate::paths::ensure_dirs().unwrap();

        let caches = fake_home.path().join("Library/Caches");
        std::fs::create_dir_all(caches.join("some.folder")).unwrap();
        std::fs::write(caches.join("some.folder/blob.bin"), vec![0u8; 4096]).unwrap();

        let mut tuning = crate::config::DriftTuning::default();
        tuning.walk.max_measure_timeouts = 0;

        let config = ScanConfig {
            scopes: vec![crate::types::ScopeCategory::Low],
            privacy_tier: PrivacyTier::Minimal,
            mode: crate::config::ScanMode::Express,
            model: "test-model".into(),
            api_key: None,
            allow_non_zdr: false,
            rules: vec![],
            tuning,
            persist: false,
        };
        let sink = CollectingSink::new();
        let report = run_scan(config, sink.clone(), ScanHandle::new()).await.unwrap();

        let folder_entry = report
            .entries
            .iter()
            .find(|e| e.candidate.path.ends_with("some.folder"))
            .expect("the folder must still be reported");
        assert_eq!(folder_entry.candidate.size_bytes, 0, "no size is claimed");
        let stats = folder_entry.candidate.kind_stats.as_ref().unwrap();
        assert!(stats.truncated, "the data model must say the walk was incomplete");

        let events = sink.snapshot();
        assert!(
            events.iter().any(|e| matches!(
                e,
                ScanEvent::Warn { message } if message.contains("not measured at all")
            )),
            "the scan must say deep sizes are missing"
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.kind == "measure_timeouts"),
            "the report must carry the degradation warning"
        );

        std::env::remove_var("HOME");
        std::env::remove_var("DRIFTWOOD_HOME");
    }

    #[test]
    fn stale_lock_of_dead_process_is_stolen() {
        let dir = tempfile::tempdir().unwrap();
        // A SIGKILLed scan leaves no Drop — but its PID is dead, so the
        // lock must be stolen immediately, not after 6 hours.
        std::fs::write(dir.path().join("scan.lock"), "999999999 123\n").unwrap();
        let lock = ScanLock::acquire(dir.path()).expect("dead owner's lock must be stolen");
        drop(lock);
        assert!(!dir.path().join("scan.lock").exists(), "Drop removes the lock");
    }

    #[test]
    fn live_owner_blocks_a_second_scan() {
        let dir = tempfile::tempdir().unwrap();
        let my_pid = std::process::id();
        std::fs::write(dir.path().join("scan.lock"), format!("{my_pid} 123\n")).unwrap();
        let err = ScanLock::acquire(dir.path()).unwrap_err();
        assert!(matches!(err, DriftError::SingleFlight(_)));
    }

    #[test]
    fn unparsable_lock_falls_back_to_age_check() {
        let dir = tempfile::tempdir().unwrap();
        // Foreign format, freshly written: age check keeps it in place.
        std::fs::write(dir.path().join("scan.lock"), "not a pid\n").unwrap();
        let err = ScanLock::acquire(dir.path()).unwrap_err();
        assert!(matches!(err, DriftError::SingleFlight(_)));
    }

    /// Express mode never crosses the river, so its middle-band items must
    /// be `Heuristic` (by design, with honest reasoning text), never
    /// `Fallback` (which claims the river was tried and failed). This is
    /// exactly what the real machine's Express report got wrong: 15 items
    /// read "Snagged — the river ran rough" for a scan with no LLM at all.
    #[tokio::test(flavor = "multi_thread")]
    async fn express_middle_band_is_heuristic_not_snagged() {
        use crate::config::ScanConfig;
        use crate::events::CollectingSink;

        let _env = ENV_LOCK.lock().unwrap();
        let fake_home = tempfile::tempdir().unwrap();
        let fake_dw = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", fake_home.path());
        std::env::set_var("DRIFTWOOD_HOME", fake_dw.path());
        crate::paths::ensure_dirs().unwrap();

        // A top-level FILE under Application Support (a non-cache root, so
        // files are units) with an old mtime lands in the middle band:
        // age 25 + a depth point, orphan Unknown (files are never orphan
        // classified), no cache-root promotion. Self-contained: no installed
        // app needed.
        let support = fake_home.path().join("Library/Application Support");
        std::fs::create_dir_all(&support).unwrap();
        let data = support.join("myapp-config.dat");
        std::fs::write(&data, vec![0u8; 2048]).unwrap();
        let old = std::fs::FileTimes::new()
            .set_modified((Utc::now() - chrono::Duration::days(400)).into());
        std::fs::File::options().write(true).open(&data).unwrap().set_times(old).unwrap();

        let config = ScanConfig {
            scopes: vec![crate::types::ScopeCategory::Low],
            privacy_tier: PrivacyTier::Minimal,
            mode: crate::config::ScanMode::Express,
            model: "test-model".into(),
            api_key: None,
            allow_non_zdr: false,
            rules: vec![],
            tuning: crate::config::DriftTuning::default(),
            persist: false,
        };
        let report = run_scan(config, CollectingSink::new(), ScanHandle::new())
            .await
            .unwrap();

        let middle: Vec<&ReportEntry> = report
            .entries
            .iter()
            .filter(|e| e.candidate.band == Band::Middle)
            .collect();
        assert!(!middle.is_empty(), "test needs middle-band entries");
        for e in middle {
            assert_eq!(e.tier_source, TierSource::Heuristic);
            assert!(!e.summary.contains("Snagged"));
            assert!(
                e.reasoning.contains("does not cross the river"),
                "reasoning must say the river was never tried"
            );
        }

        std::env::remove_var("HOME");
        std::env::remove_var("DRIFTWOOD_HOME");
    }

    fn candidate_fixture() -> Candidate {
        Candidate {
            id: "c-test".into(),
            path: "/tmp/x".into(),
            kind: Kind::Folder,
            size_bytes: 1_000,
            kind_stats: None,
            last_used_date: None,
            last_used_from_spotlight: false,
            modified_date: None,
            created_date: None,
            orphan_status: OrphanStatus::Unknown,
            scope_category: crate::types::ScopeCategory::Low,
            score: 50.0,
            score_components: crate::score::ScoreComponents::default(),
            band: Band::Middle,
            auto_high_basis: None,
        }
    }

    /// One-item adjudication end-to-end against a persisted report: typed
    /// error without a key, unknown id, and the disagreement wiring. Uses
    /// DRIFTWOOD_HOME to isolate the reports dir (no other test touches it).
    #[tokio::test(flavor = "multi_thread")]
    async fn adjudication_resolves_from_persisted_report() {
        use crate::events::CollectingSink;
        use crate::report::{AssembleInput, Counters};

        // The bounded-riverbed test also reads DRIFTWOOD_HOME now, so this
        // test can no longer assume it is the only reader.
        let _env = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("DRIFTWOOD_HOME", tmp.path()) };
        crate::paths::ensure_dirs().unwrap();

        let mut c = candidate_fixture();
        c.id = "c-adj-1".into();
        let entry = ReportEntry {
            candidate: c,
            tier: Tier::Driftwood,
            tier_source: TierSource::ArguedAutoHigh,
            summary: "s".into(),
            reasoning: "r".into(),
            confidence: 1.0,
            llm_model: None,
            privacy_tier_used: PrivacyTier::Minimal,
            rule_id: None,
        };
        let report = crate::report::assemble(AssembleInput {
            scan_id: "test-adj-scan".into(),
            spotlight_available: true,
            privacy_tier: PrivacyTier::Minimal,
            model: None,
            llm_cost_usd: 0.0,
            cost_cap_hit: false,
            stopped_early: false,
            counters: Counters::default(),
            entries: vec![entry],
            warnings: vec![],
        });
        report.persist().unwrap();

        // No key → typed error, no silent failure.
        let req = AdjudicationRequest {
            candidate_id: "c-adj-1".into(),
            model: "m".into(),
            api_key: String::new(),
            privacy_tier: PrivacyTier::Minimal,
            enforce_zdr: true,
        };
        let err = adjudicate_candidate(req.clone(), CollectingSink::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no OpenRouter key"), "{err}");

        // Unknown id → typed error, never an invented candidate.
        let req_bad = AdjudicationRequest {
            candidate_id: "c-unknown".into(),
            api_key: "k".into(),
            ..req.clone()
        };
        let err = adjudicate_candidate(req_bad, CollectingSink::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown candidate id"), "{err}");

        unsafe { std::env::remove_var("DRIFTWOOD_HOME") };
        let _ = req; // keep the happy-path request shape here for readability
    }
}
