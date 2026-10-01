//! Spotlight-first metadata (plan §3.1.2): `mdutil` availability probe,
//! `mdfind` recency sets, `mdls` last-used batching. All parsing is done by
//! pure functions so golden fixtures can be tested without Spotlight.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};

/// Probe whether Spotlight indexing is enabled for the root volume.
/// When false, the scanner degrades to walkdir + dates and the report is
/// labeled "the river ran murky" (edge case §4.1).
pub async fn spotlight_available(timeout: Duration) -> bool {
    let out = run("mdutil", &["-s", "/"], timeout).await;
    match out {
        Some(s) => s.to_lowercase().contains("indexing enabled"),
        None => false,
    }
}

/// Parse `mdutil -s /` output (pure, golden-testable).
pub fn parse_mdutil_output(output: &str) -> bool {
    output.to_lowercase().contains("indexing enabled")
}

/// Query `mdfind -onlyin <root>` for paths used within the recency window.
/// `None` means the query never answered (timeout/failure) — the caller
/// must treat that as *incomplete recency data*, distinct from an empty
/// result (nothing recent), and warn accordingly.
pub async fn recently_used_under(
    root: &Path,
    recency_days: i64,
    timeout: Duration,
) -> Option<HashSet<PathBuf>> {
    let query = format!("kMDItemLastUsedDate >= $time.today(-{recency_days}d)");
    let out = run(
        "mdfind",
        &["-onlyin", &root.to_string_lossy(), &query],
        timeout,
    )
    .await?;
    Some(
        parse_mdfind_output(&out)
            .into_iter()
            .map(PathBuf::from)
            .collect(),
    )
}

/// Parse `mdfind` output: one absolute path per line.
pub fn parse_mdfind_output(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// Batch-query `mdls -name kMDItemLastUsedDate <paths...>`; returns a map
/// from path → Option<date>. `None` values mean Spotlight had no date for
/// that path — which never blocks candidacy (plan §3.1.2).
pub async fn query_last_used(
    paths: &[PathBuf],
    batch_size: usize,
    timeout: Duration,
) -> HashMap<PathBuf, Option<DateTime<Utc>>> {
    let mut out = HashMap::with_capacity(paths.len());
    for chunk in paths.chunks(batch_size.max(1)) {
        let mut args: Vec<String> = Vec::with_capacity(chunk.len() * 2 + chunk.len());
        for _ in chunk {
            args.push("-name".into());
            args.push("kMDItemLastUsedDate".into());
        }
        for p in chunk {
            args.push(p.to_string_lossy().into_owned());
        }
        let output = run(
            "mdls",
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            timeout,
        )
        .await;
        if let Some(text) = output {
            out.extend(parse_mdls_output(&text));
        }
    }
    out
}

/// Parse `mdls` multi-file output (pure, golden-testable).
///
/// Shape:
/// ```text
/// /path/one:
/// kMDItemLastUsedDate = 2024-01-02 03:04:05 +0000
///
/// /path/two:
/// kMDItemLastUsedDate = null
/// ```
/// Records with no date line, or an unparseable/null date, map to `None`.
pub fn parse_mdls_output(output: &str) -> HashMap<PathBuf, Option<DateTime<Utc>>> {
    let mut map = HashMap::new();
    let mut current: Option<PathBuf> = None;
    for line in output.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(path) = line.strip_suffix(':') {
            if !path.contains(" = ") {
                current = Some(PathBuf::from(path));
                continue;
            }
        }
        if let Some(rest) = line.strip_prefix("kMDItemLastUsedDate") {
            let value = rest.trim_start();
            let value = value.strip_prefix('=').unwrap_or(value).trim();
            let date = parse_mdls_date(value);
            if let Some(p) = current.take() {
                map.insert(p, date);
            }
        }
    }
    map
}

/// Parse the mdls date format `2024-01-02 03:04:05 +0000`. Also tolerates
/// ISO-8601 (`2024-01-02T03:04:05Z`) and `null` / garbage → `None`.
pub fn parse_mdls_date(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    if value.is_empty() || value == "null" || value == "(null)" {
        return None;
    }
    // mdls: "2024-01-02 03:04:05 +0000"
    if let Ok(naive) = NaiveDateTime::parse_from_str(&value[..19.min(value.len())], "%Y-%m-%d %H:%M:%S") {
        let offset = value
            .split_once(char::is_whitespace)
            .and_then(|(_, tz)| tz.trim().parse::<i32>().ok())
            .and_then(|mins| chrono::FixedOffset::east_opt(mins * 3600))
            .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).unwrap());
        if let Some(dt) = offset.from_local_datetime(&naive).single() {
            return Some(dt.with_timezone(&Utc));
        }
    }
    // ISO-8601 fallback
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Some(dt.with_timezone(&Utc));
    }
    None
}

/// Run one Spotlight helper, bounded by `timeout`. `mdfind`/`mdls` on an
/// unresponsive mount block forever without it — a hang reads as "no data"
/// (the safe direction: missing recency never blocks candidacy), and
/// `kill_on_drop` reaps the dropped child.
async fn run(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let child = tokio::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let waited = tokio::time::timeout(timeout, child.wait_with_output()).await;
    match waited {
        Ok(Ok(out)) => {
            if !out.status.success() {
                return None;
            }
            Some(String::from_utf8_lossy(&out.stdout).into_owned())
        }
        // Timeout or wait failure: no data. Dropping the child kills it
        // (kill_on_drop), so a wedged mdfind does not linger.
        _ => None,
    }
}

/// Compute an age reference date: Spotlight last-used, else modified, else
/// created. mtime is never used as "last used" for hard rules — only as a
/// weak scoring signal (edge case §4.2).
pub fn age_reference(
    last_used: Option<chrono::DateTime<Utc>>,
    modified: Option<chrono::DateTime<Utc>>,
    created: Option<chrono::DateTime<Utc>>,
) -> Option<chrono::DateTime<Utc>> {
    last_used.or(modified).or(created)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\
/Users/x/com.vendor.app:
kMDItemLastUsedDate = 2024-01-02 03:04:05 +0000

/Users/x/other.app:
kMDItemLastUsedDate = null
";

    #[test]
    fn parse_mdls_fixture() {
        let map = parse_mdls_output(FIXTURE);
        assert_eq!(map.len(), 2);
        let d = map.get(Path::new("/Users/x/com.vendor.app")).unwrap().unwrap();
        assert_eq!(d.format("%Y-%m-%d").to_string(), "2024-01-02");
        assert!(map.get(Path::new("/Users/x/other.app")).unwrap().is_none());
    }

    #[test]
    fn parse_mdls_null_and_missing() {
        let map = parse_mdls_output("/p:\nkMDItemLastUsedDate = null\n/q:\n");
        assert!(map.get(Path::new("/p")).unwrap().is_none());
        // /q had no date line at all — absent from the map.
        assert!(!map.contains_key(Path::new("/q")));
    }

    #[test]
    fn parse_mdls_iso8601() {
        assert!(parse_mdls_date("2024-06-01T12:00:00Z").is_some());
        assert!(parse_mdls_date("garbage").is_none());
    }

    #[test]
    fn parse_mdfind_lines() {
        let v = parse_mdfind_output("/a\n /b \n\n/c");
        assert_eq!(v, vec!["/a", "/b", "/c"]);
    }

    #[test]
    fn mdutil_parsing() {
        assert!(parse_mdutil_output("/: Indexing enabled."));
        assert!(!parse_mdutil_output("/: Indexing disabled."));
    }

    #[test]
    fn age_reference_prefers_spotlight() {
        use chrono::TimeZone;
        let a = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let m = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(age_reference(Some(a), Some(m), None), Some(a));
        assert_eq!(age_reference(None, Some(m), None), Some(m));
    }

    /// A helper process that never answers in time reads as no data — the
    /// bounded-riverbed contract for Spotlight subprocesses.
    #[tokio::test]
    async fn run_times_out_a_hung_helper() {
        // `sleep` is POSIX; 5s >> the 50ms budget.
        let out = run("sleep", &["5"], Duration::from_millis(50)).await;
        assert!(out.is_none(), "a hung helper must time out, not block");
    }

    #[tokio::test]
    async fn run_returns_output_within_budget() {
        let out = run("echo", &["hello"], Duration::from_secs(5)).await;
        assert_eq!(out.as_deref(), Some("hello\n"));
    }
}
