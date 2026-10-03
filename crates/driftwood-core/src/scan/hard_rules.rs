//! Hard rules (plan §3.1.3): the "still in the current" recency filter and
//! the never-flag list. Both are cheap, deterministic, and applied before
//! any scoring or LLM work.

use std::path::Path;

/// The "still in the current" rule (notes §5): drop anything Spotlight
/// says was used within the recency window. A hard filter ONLY on positive
/// Spotlight recency — missing data never blocks candidacy (plan §3.1.2).
pub fn still_in_current(
    last_used: Option<chrono::DateTime<chrono::Utc>>,
    from_spotlight: bool,
    now: chrono::DateTime<chrono::Utc>,
    recency_days: i64,
) -> bool {
    match last_used {
        Some(t) if from_spotlight => now.signed_duration_since(t).num_days() < recency_days,
        _ => false,
    }
}

/// Hardcode-banned paths (edge case §4.10): always tier 4 minimum,
/// regardless of score. The patterns now live in ONE editable place,
/// [`crate::system_paths::PROTECTED_HOME_SUFFIXES`]; this alias keeps the
/// existing name working. Includes iOS device backups (MobileSync),
/// keychains, mail, messages, and call history — never candidates for
/// deletion advice.
pub use crate::system_paths::PROTECTED_HOME_SUFFIXES as NEVER_FLAG_SUFFIXES;

/// Is this path on the never-flag list? `home` is the user's home dir;
/// paths outside home are never flagged unless explicitly absolute.
pub fn is_never_flagged(path: &Path, home: &Path) -> bool {
    crate::system_paths::is_protected_home_path(path, home)
}

/// Group-container paths (`~/Library/Containers/group.*`,
/// `~/Library/Group Containers`) are shared by apps — tier 4 minimum.
pub fn is_group_container(path: &Path, home: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(home) else {
        return false;
    };
    let rel = rel.to_string_lossy();
    rel.starts_with("Library/Containers/group.")
        || rel.starts_with("Library/Group Containers/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn recency_rule_is_spotlight_positive_only() {
        let now = Utc.with_ymd_and_hms(2026, 9, 25, 0, 0, 0).unwrap();
        let five_days_ago = now - chrono::Duration::days(5);
        let forty_days_ago = now - chrono::Duration::days(40);

        assert!(still_in_current(Some(five_days_ago), true, now, 21));
        assert!(!still_in_current(Some(forty_days_ago), true, now, 21));
        // Missing data never blocks candidacy…
        assert!(!still_in_current(None, false, now, 21));
        // …and a recent non-Spotlight date (mtime) never filters either.
        assert!(!still_in_current(Some(five_days_ago), false, now, 21));
    }

    #[test]
    fn never_flag_matching() {
        let home = Path::new("/Users/x");
        assert!(is_never_flagged(&home.join("Library/Keychains"), home));
        assert!(is_never_flagged(
            &home.join("Library/Application Support/MobileSync/Backup.1234"),
            home
        ));
        assert!(!is_never_flagged(&home.join("Library/Caches/com.foo"), home));
        assert!(is_group_container(
            &home.join("Library/Containers/group.com.shared"),
            home
        ));
        assert!(!is_group_container(
            &home.join("Library/Containers/com.solo.app"),
            home
        ));
    }
}
