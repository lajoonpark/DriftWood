//! Orphan detection (plan §3.1.4): classic AppCleaner-style matching of
//! app-data folders against installed apps.
//!
//! Matching rules under normalization:
//! - reverse-DNS match: `com.vendor.app` folder vs installed bundle id
//! - vendor-prefix match: `com.vendor.*` installed ⇒ shared vendor folders
//!   count as ACTIVE (protects shared vendor caches)
//! - fuzzy name match: normalized folder name vs app display name
//! - explicit never-orphan list: `com.apple.*`, group containers, etc.
//!   (data, not code — extendable from the memory folder)

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::types::OrphanStatus;

/// A snapshot of installed applications.
#[derive(Debug, Clone, Default)]
pub struct InstalledApps {
    /// Lower-cased bundle identifiers, e.g. `com.google.chrome`.
    pub bundle_ids: BTreeSet<String>,
    /// Normalized display names, e.g. `googlechrome`.
    pub names: BTreeSet<String>,
    /// Vendor prefixes, e.g. `com.google`.
    pub vendor_prefixes: BTreeSet<String>,
    /// Folder names of installed app bundles, normalized, e.g. `mytool`.
    pub bundle_dirs: BTreeSet<String>,
}

impl InstalledApps {
    pub fn is_empty(&self) -> bool {
        self.bundle_ids.is_empty() && self.names.is_empty()
    }
}

/// System/OS-vendor owner patterns. The patterns themselves live in the
/// single editable `crate::system_paths` module; this alias keeps the old
/// name working for the system floor.
pub use crate::system_paths::SUITE_VENDOR_PATTERNS;
use crate::system_paths::SYSTEM_OWNER_PATTERNS;

/// Default never-orphan entries, EXCLUDING the system-vendor patterns
/// above (those are merged in by [`load_never_orphan_list`], which is the
/// one place the two lists meet). Data, not code: the memory folder may
/// extend this via `never-orphan.json` (plan §3.1.4).
pub const DEFAULT_NEVER_ORPHAN: &[&str] = &[
    "group.*",
    "com.apple",
    "apple",
    // known shared / system dirs
    "MobileSync",
    "MobileDevice",
    "SyncServices",
    "CloudDocs",
    "iLifeMediaBrowser",
    "Knowledge",
    "CallHistory",
    "CallHistoryDB",
    "CallHistoryTransactions",
    "AddressBook",
    "FaceTime",
    "GameKit",
    "SyncedPreferences",
    "ubd",
    "Group Containers",
];

/// Names too generic to judge orphan status for.
const GENERIC_NAMES: &[&str] = &[
    "cache", "caches", "data", "temp", "tmp", "files", "store", "storage", "shared", "common",
    "default", "defaults", "local", "settings", "config", "app", "bin", "lib", "logs", "support",
    "preferences", "saved", "state", "application", "applications", "com", "vendor", "user",
    "users", "sentry", "crash", "crashes", "updates", "updater", "installer", "sdk", "libexec",
];

/// Enumerate installed apps from `/Applications` + `~/Applications`,
/// including one nesting level for organized folders and `.app`-inside-
/// `.app` (runtime-shipped helpers). Read-only; plists that fail to parse
/// are skipped.
pub fn enumerate_installed_apps(home: &Path, app_cap: usize) -> InstalledApps {
    let mut apps = InstalledApps::default();
    let roots = [PathBuf::from("/Applications"), home.join("Applications")];
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .max_depth(4)
        {
            let Ok(entry) = entry else { continue };
            if apps.bundle_ids.len() >= app_cap {
                break;
            }
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("app") {
                continue;
            }
            register_app(&mut apps, path);
        }
    }
    // Derive vendor prefixes from bundle ids: com.vendor.*
    for id in &apps.bundle_ids {
        let parts: Vec<&str> = id.split('.').collect();
        if parts.len() >= 2 {
            apps.vendor_prefixes.insert(format!("{}.{}", parts[0], parts[1]));
        }
    }
    apps
}

fn register_app(apps: &mut InstalledApps, path: &Path) {
    let dir_name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    if let Some(n) = &dir_name {
        apps.bundle_dirs.insert(normalize_name(n));
    }

    // Info.plist for bundle id + display name (best-effort).
    let plist_path = path.join("Contents/Info.plist");
    if let Ok(plist_value) = plist::Value::from_file(&plist_path) {
        let dict = match &plist_value {
            plist::Value::Dictionary(d) => Some(d),
            _ => None,
        };
        if let Some(dict) = dict {
            if let Some(plist::Value::String(id)) = dict.get("CFBundleIdentifier") {
                apps.bundle_ids.insert(id.to_lowercase());
            }
            if let Some(plist::Value::String(name)) = dict
                .get("CFBundleDisplayName")
                .or_else(|| dict.get("CFBundleName"))
            {
                apps.names.insert(normalize_name(name));
            }
        }
    }
    if let Some(n) = dir_name {
        let stem = n.strip_suffix(".app").unwrap_or(&n);
        apps.names.insert(normalize_name(stem));
    }
}

/// Normalize a name for matching: lowercase, keep only alphanumerics and
/// dots (dots matter for reverse-DNS).
pub fn normalize_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_')
        .collect()
}

fn looks_reverse_dns(normalized: &str) -> bool {
    // at least two dot-separated parts, all non-empty, TLD-ish first part
    let parts: Vec<&str> = normalized.split('.').filter(|p| !p.is_empty()).collect();
    parts.len() >= 2 && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// Fuzzy name match: exact normalized equality, containment (min length 4),
/// or full token overlap.
pub fn fuzzy_match(candidate: &str, name: &str) -> bool {
    if candidate == name {
        return true;
    }
    if candidate.len() >= 4 && name.len() >= 4 && (name.contains(candidate) || candidate.contains(name)) {
        return true;
    }
    false
}

/// Load the protection list: system-vendor patterns + default never-orphan
/// entries, merged with `<memory_dir>/never-orphan.json` when present
/// (plain JSON array). This is the single merge point for the two
/// consumers — never-orphan detection and the system floor both match
/// against this list, so a vendor added to the memory-folder file is
/// respected by both or neither.
pub fn load_never_orphan_list(memory_dir: &Path) -> Vec<String> {
    let mut list: Vec<String> = SYSTEM_OWNER_PATTERNS
        .iter()
        .chain(SUITE_VENDOR_PATTERNS.iter())
        .chain(DEFAULT_NEVER_ORPHAN.iter())
        .map(|s| s.to_string())
        .collect();
    let path = memory_dir.join("never-orphan.json");
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(serde_json::Value::Array(items)) = serde_json::from_str(&text) {
            for item in items {
                if let Some(s) = item.as_str() {
                    list.push(s.to_string());
                }
            }
        }
    }
    list
}

/// The pattern (if any) from the protection list that matches this folder
/// name. Used by the system floor: a match means the folder's owner is on
/// the system/vendor list, so DriftWood must not auto-label it safe.
pub fn matching_protected_pattern(normalized_name: &str, never_list: &[String]) -> Option<String> {
    never_list
        .iter()
        .find(|entry| is_never_orphan(normalized_name, std::slice::from_ref(entry)))
        .cloned()
}

/// Is this folder name protected by the never-orphan list? Patterns use
/// `prefix.*` wildcards (matched on raw lowercased text so `*` survives
/// normalization).
pub fn is_never_orphan(normalized_name: &str, never_list: &[String]) -> bool {
    never_list.iter().any(|entry| {
        let raw = entry.trim().to_lowercase();
        if let Some(prefix) = raw.strip_suffix(".*") {
            let p = normalize_name(prefix);
            normalized_name == p || normalized_name.starts_with(&format!("{p}."))
        } else {
            normalized_name == normalize_name(&raw)
        }
    })
}

/// Classify an app-data folder name against installed apps.
/// Folder names like `com.vendor.app` (reverse-DNS) and `Google Chrome`
/// (plain) are both handled. Generic names return `Unknown`.
///
/// `Orphaned` is reserved for POSITIVE evidence that a former owner is
/// gone: a reverse-DNS folder name is a left-behind bundle identifier, so
/// a reverse-DNS name with no matching installed app is orphaned. A plain
/// folder name proves nothing — "no matching app found" alone is NOT
/// evidence, and returns `Unknown`. Anything OS-owned (Apple patterns and
/// system directories) is never orphaned.
pub fn classify_orphan(folder_name: &str, apps: &InstalledApps, never_list: &[String]) -> OrphanStatus {
    let normalized = normalize_name(folder_name);

    if is_never_orphan(&normalized, never_list) {
        return OrphanStatus::Active; // protected: never orphaned
    }
    // OS-owned daemon/service directories are never a user app's leftovers.
    if crate::system_paths::matching_system_owner(folder_name).is_some() {
        return OrphanStatus::Active;
    }

    // Only a bundle-identifier-shaped name is evidence an app once owned
    // this folder. A plain name says nothing about a former install, but a
    // plain name that DOES match an installed app is still Active.
    if !looks_reverse_dns(&normalized) {
        if !is_generic(&normalized)
            && apps
                .names
                .iter()
                .chain(apps.bundle_dirs.iter())
                .any(|n| fuzzy_match(&normalized, n))
        {
            return OrphanStatus::Active;
        }
        return OrphanStatus::Unknown;
    }

    // Exact bundle-id match → active.
    if apps.bundle_ids.contains(&normalized) {
        return OrphanStatus::Active;
    }
    // Vendor-prefix match → active (protect shared vendor folders).
    let parts: Vec<&str> = normalized.split('.').collect();
    if parts.len() >= 2 {
        let vendor = format!("{}.{}", parts[0], parts[1]);
        if apps.vendor_prefixes.contains(&vendor) {
            return OrphanStatus::Active;
        }
    }
    // Last component fuzzy-matches an app name → active.
    let last = parts.last().copied().unwrap_or(&normalized).to_string();
    if last.len() >= 3 && apps.names.iter().any(|n| fuzzy_match(&last, n)) {
        return OrphanStatus::Active;
    }
    if is_generic(&last) {
        return OrphanStatus::Unknown;
    }
    // A reverse-DNS name with no match is a leftover bundle identifier.
    OrphanStatus::Orphaned
}

fn is_generic(normalized: &str) -> bool {
    normalized.len() < 3 || GENERIC_NAMES.contains(&normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OrphanStatus::*;

    fn apps() -> InstalledApps {
        let mut a = InstalledApps::default();
        a.bundle_ids
            .extend(["com.google.chrome".into(), "com.apple.finder".into(), "com.jetbrains.intellij".into()]);
        a.names.extend(["googlechrome".into(), "finder".into(), "intellij".into()]);
        a.vendor_prefixes
            .extend(["com.google".into(), "com.apple".into(), "com.jetbrains".into()]);
        a.bundle_dirs.extend(["googlechrome".into(), "intellij".into()]);
        a
    }

    #[test]
    fn table_orphan_matching() {
        let a = apps();
        let never = load_never_orphan_list(Path::new("/nonexistent"));
        let cases: Vec<(&str, OrphanStatus)> = vec![
            // reverse-DNS matches
            ("com.google.chrome", Active),
            // vendor prefix shared with installed app → protected
            ("com.google.earth", Active),
            // reverse-DNS, no vendor, no name match → orphaned
            ("com.abandonedcorp.oldapp", Orphaned),
            // reverse-DNS whose last part fuzzy-matches an app name
            ("com.misc.intellij", Active),
            // Apple everything is protected
            ("com.apple.helper", Active),
            // group containers protected
            ("group.com.sharedstuff", Active),
            ("com.apple.sharedfilelist", Active),
            // plain names
            ("Google Chrome", Active),
            // A plain name with no match is NOT evidence of orphanhood.
            ("MyOldDeletedGame", Unknown),
            ("Cache", Unknown),
            ("Data", Unknown),
            ("ab", Unknown),
            ("com.vendor.1234", Orphaned),
        ];
        for (name, expected) in cases {
            assert_eq!(classify_orphan(name, &a, &never), expected, "case: {name}");
        }
    }

    #[test]
    fn never_orphan_wildcards() {
        assert!(is_never_orphan("com.apple.safari", &["com.apple.*".into()]));
        assert!(is_never_orphan("com.apple", &["com.apple.*".into()]));
        assert!(!is_never_orphan("com.appleicloud", &["com.apple.*".into()]));
        assert!(is_never_orphan("mobilesync", &["MobileSync".into()]));
    }

    #[test]
    fn normalization() {
        assert_eq!(normalize_name("Google Chrome!"), "googlechrome");
        assert_eq!(normalize_name("My-App_2"), "myapp_2");
    }

    #[test]
    fn fuzzy() {
        assert!(fuzzy_match("intellijidea", "intellij"));
        // containment in either direction is deliberately protective:
        // marking shared/vendor folders ACTIVE is the safe direction.
        assert!(fuzzy_match("idea", "idea2x"));
        assert!(fuzzy_match("same", "same"));
    }
}
