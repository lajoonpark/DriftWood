//! System-owned path hard rules — THE one editable list.
//!
//! DriftWood must never let a model call an OS-owned location safe to
//! delete. Apple's folders under `~/Library` exist for services that
//! outlive any one app: call history, device backups, keychains, mail,
//! messages. A reverse-DNS `com.apple.*` folder, or a daemon/service
//! directory named `Call*`, `Mobile*`, `AddressBook`, etc., is owned by
//! macOS, not by a user-facing app. These rules run before any model call
//! and send a match straight to Source, so such items cost no tokens and
//! can never be rated deletable.
//!
//! WHY these and not others: the criterion is *ownership by the OS* — the
//! vendor ships macOS itself (Apple) or ships a daemon/service that is not
//! a user app. Ordinary third-party vendors are deliberately NOT on this
//! list; they are judged by the model like any other folder. Whole-suite
//! vendors whose state can be load-bearing across apps (Microsoft) stay on
//! the softer `Current` floor in `orphan.rs`, not here.
//!
//! To extend the rules, edit the three lists below. `*` is a trailing
//! wildcard: `com.apple.*` and `Call*`.

use std::path::Path;

/// Protected locations under the home directory: an exact relative path or
/// any descendant (component boundary). These are personal/system data that
/// must never be flagged for deletion.
pub const PROTECTED_HOME_SUFFIXES: &[&str] = &[
    "Library/Keychains",
    "Library/Application Support/Knowledge",
    "Library/Application Support/MobileSync",
    "Library/Application Support/MobileDevice",
    "Library/Application Support/SyncServices",
    "Library/Application Support/CallHistoryDB",
    "Library/Application Support/CallHistoryTransactions",
    "Library/Application Support/com.apple.sharedfilelist",
    "Library/Mail",
    "Library/Mail Downloads",
    "Library/Messages",
    "Library/Accounts",
    "Library/Cookies",
    "Library/Safari",
    "Library/Passwords",
    "Library/IdentityServices",
    "Library/HomeKit",
    "Library/Sharing",
    "Library/Suggestions",
];

/// Apple daemon/service directory names (matched on the folder's own name,
/// case-insensitively; trailing `*` is a wildcard). Covers the `Call*`,
/// `Mobile*` families the user sees under Application Support plus the
/// other service-owned directories.
pub const APPLE_DIR_PATTERNS: &[&str] = &[
    "Call*",
    "Mobile*",
    "AddressBook",
    "FaceTime",
    "Knowledge",
    "SyncServices",
    "CloudDocs",
    "iLifeMediaBrowser",
    "GameKit",
    "SyncedPreferences",
    "ubd",
    "IdentityServices",
    "HomeKit",
    "Sharing",
    "Suggestions",
    "DoNotDisturb",
    "Biome",
    "DuetExpertCenter",
    "com.apple",
];

/// Owner patterns matched against a folder's reverse-DNS name. Anything
/// Apple-owned is OS-owned: straight to Source.
pub const SYSTEM_OWNER_PATTERNS: &[&str] = &["com.apple.*"];

/// Whole-suite vendors that may hold load-bearing cross-app state. These
/// are floored at `Current` (Tier 3), not forced to Source — unlike Apple,
/// they make ordinary apps the user installed. Kept here so all the
/// system/vendor ownership patterns have one home.
pub const SUITE_VENDOR_PATTERNS: &[&str] = &["com.microsoft.*"];

/// Match a raw (lowercased here) folder name against a pattern list.
/// A trailing `*` matches any continuation; otherwise the match is exact.
fn matches_any(name: &str, patterns: &[&str]) -> bool {
    let n = name.to_ascii_lowercase();
    patterns.iter().any(|p| {
        let p = p.to_ascii_lowercase();
        match p.strip_suffix('*') {
            // `com.apple.*` → prefix `com.apple.` (and the bare parent,
            // handled by the exact `com.apple` entry in APPLE_DIR_PATTERNS).
            Some(prefix) => n.starts_with(prefix),
            None => n == p,
        }
    })
}

/// The pattern that marks this folder name as OS-owned, if any. The
/// returned string is the matched pattern, for the report's reason text.
pub fn matching_system_owner(name: &str) -> Option<String> {
    SYSTEM_OWNER_PATTERNS
        .iter()
        .chain(APPLE_DIR_PATTERNS.iter())
        .find(|p| matches_any(name, std::slice::from_ref(p)))
        .map(|p| p.to_string())
}

/// Is this folder name an OS-owned Apple daemon/service directory?
pub fn is_apple_dir_name(name: &str) -> bool {
    matches_any(name, APPLE_DIR_PATTERNS)
}

/// The system-owned pattern for a whole path, if any. `com.apple.*`
/// identifiers count anywhere; Apple daemon/service NAMES (`Call*`,
/// `Mobile*`, …) only count under the user's `~/Library`, so an ordinary
/// folder the user happens to name `Sharing` or `Suggestions` is not
/// dragged to Source.
pub fn system_owned_for_path(path: &Path, home: &Path) -> Option<String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(p) = SYSTEM_OWNER_PATTERNS
        .iter()
        .find(|p| matches_any(&name, std::slice::from_ref(p)))
    {
        return Some(p.to_string());
    }
    let under_library =
        path.starts_with(home.join("Library")) || path.starts_with(Path::new("/Library"));
    if under_library {
        return APPLE_DIR_PATTERNS
            .iter()
            .find(|p| matches_any(&name, std::slice::from_ref(p)))
            .map(|p| p.to_string());
    }
    None
}

/// Is this path under a protected home-directory suffix?
pub fn is_protected_home_path(path: &Path, home: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(home) else {
        return false;
    };
    let rel = rel.to_string_lossy();
    PROTECTED_HOME_SUFFIXES
        .iter()
        .any(|suffix| rel == *suffix || rel.starts_with(&format!("{suffix}/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_suffixes_match_component_boundaries() {
        let home = Path::new("/Users/x");
        assert!(is_protected_home_path(
            &home.join("Library/Application Support/CallHistoryTransactions"),
            home
        ));
        assert!(is_protected_home_path(
            &home.join("Library/Application Support/MobileSync/Backup.1"),
            home
        ));
        assert!(!is_protected_home_path(&home.join("Library/Caches/com.foo"), home));
        // A sibling whose name merely starts with the suffix word is not in.
        assert!(!is_protected_home_path(
            &home.join("Library/Application Support/CallHistoryDB-extra"),
            home
        ));
    }

    #[test]
    fn apple_owner_and_dir_patterns() {
        assert!(matching_system_owner("com.apple.HomeKit").is_some());
        assert!(matching_system_owner("CallHistoryTransactions").is_some());
        assert!(matching_system_owner("MobileSync").is_some());
        assert!(matching_system_owner("AddressBook").is_some());
        // Ordinary third-party apps are not OS-owned.
        assert!(matching_system_owner("com.google.Chrome").is_none());
        assert!(matching_system_owner("MyNotes").is_none());
    }

    #[test]
    fn apple_dir_names_only_count_under_library() {
        let home = Path::new("/Users/x");
        assert!(system_owned_for_path(
            &home.join("Library/Application Support/CallHistoryTransactions"),
            home
        )
        .is_some());
        assert!(system_owned_for_path(&home.join("Library/Caches/com.apple.foo"), home).is_some());
        // A user folder that merely shares a daemon's name is not OS-owned.
        assert!(system_owned_for_path(&home.join("Documents/Sharing"), home).is_none());
    }
}
