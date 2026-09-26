//! Scope enumeration (plan §3.1.1): resolve the three preset categories
//! into concrete roots.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::types::ScopeCategory;

/// A concrete scan root. `cache_like` drives the cache-location score
/// component and the junk-root hard-rule flagging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeRoot {
    pub path: PathBuf,
    pub category: ScopeCategory,
    pub cache_like: bool,
}

impl ScopeRoot {
    pub fn label(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }
}

/// Resolve the selected categories into roots (notes §9 presets).
/// Missing roots are skipped silently (e.g. no ~/Movies).
pub fn resolve_roots(categories: &[ScopeCategory], home: &std::path::Path) -> Vec<ScopeRoot> {
    let mut roots = Vec::new();
    let lib = home.join("Library");

    for cat in categories {
        match cat {
            ScopeCategory::Low => {
                for (p, cache_like) in [
                    (lib.join("Caches"), true),
                    (PathBuf::from("/tmp"), true),
                    (lib.join("Logs"), true),
                    (lib.join("Application Support"), false),
                ] {
                    roots.push(ScopeRoot {
                        path: p,
                        category: ScopeCategory::Low,
                        cache_like,
                    });
                }
            }
            ScopeCategory::Medium => {
                for (p, cache_like) in [
                    (home.join("Downloads"), false),
                    (lib.join("Containers"), true),
                ] {
                    roots.push(ScopeRoot {
                        path: p,
                        category: ScopeCategory::Medium,
                        cache_like,
                    });
                }
            }
            ScopeCategory::High => {
                for name in ["Documents", "Pictures", "Movies", "Music"] {
                    roots.push(ScopeRoot {
                        path: home.join(name),
                        category: ScopeCategory::High,
                        cache_like: false,
                    });
                }
            }
        }
    }
    roots.retain(|r| r.path.is_dir());
    roots
}

/// A candidate unit: one top-level entry of a scope root. Deeper levels are
/// aggregated into the unit (edge case §4.4), so one huge cache never
/// explodes into thousands of candidates.
#[derive(Debug, Clone)]
pub struct Unit {
    pub path: PathBuf,
    pub kind: crate::types::Kind,
    pub category: ScopeCategory,
    pub root: ScopeRoot,
}

/// Enumerate candidate units under a root: immediate children only,
/// capped, symlinks and dotfiles skipped. Permission errors are
/// skip-and-count, never abort (edge case §4.3).
pub fn enumerate_units(
    root: &ScopeRoot,
    caps: &crate::config::WalkCaps,
    warn: &mut dyn FnMut(String),
) -> Vec<Unit> {
    use crate::types::Kind;

    let mut units = Vec::new();
    let read = match std::fs::read_dir(&root.path) {
        Ok(r) => r,
        Err(e) => {
            warn(format!("wading past {}: {e}", root.path.display()));
            return units;
        }
    };

    for entry in read.flatten() {
        if units.len() >= caps.max_entries_per_root {
            warn(format!(
                "capped enumeration of {} at {} entries",
                root.path.display(),
                caps.max_entries_per_root
            ));
            break;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        // Never follow symlinks at the unit level: they may point anywhere.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let kind = if meta.is_dir() {
            // A `.app` directory is an app bundle only if it actually has
            // bundle structure — cache folders named `com.vendor.app` are
            // NOT apps.
            if path.extension().map(|e| e == "app").unwrap_or(false)
                && path.join("Contents/MacOS").is_dir()
            {
                Kind::App
            } else {
                Kind::Folder
            }
        } else {
            Kind::File
        };
        // Application Support / Caches / Logs / Containers: folders only.
        if root.cache_like && kind == Kind::File {
            continue;
        }
        units.push(Unit {
            path,
            kind,
            category: root.category,
            root: root.clone(),
        });
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Kind;
    use std::fs;

    fn scratch() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir_all(home.join("Library/Caches")).unwrap();
        fs::create_dir_all(home.join("Downloads")).unwrap();
        (dir, home)
    }

    #[test]
    fn resolves_low_and_skips_missing() {
        let (_d, home) = scratch();
        let roots = resolve_roots(&[ScopeCategory::Low], &home);
        let paths: Vec<String> = roots.iter().map(|r| r.path.display().to_string()).collect();
        assert!(paths.iter().any(|p| p.ends_with("Library/Caches")));
        assert!(roots.iter().any(|r| r.cache_like));
        assert!(roots.iter().any(|r| r.path == std::path::PathBuf::from("/tmp")));
    }

    #[test]
    fn enumerates_units_skips_dotfiles_and_symlinks() {
        let (_d, home) = scratch();
        let caches = home.join("Library/Caches");
        fs::create_dir_all(caches.join("com.vendor.app")).unwrap();
        fs::write(caches.join(".DS_Store"), "x").unwrap();
        fs::write(caches.join("stray-file.txt"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc", caches.join("evil-link")).unwrap();

        let root = resolve_roots(&[ScopeCategory::Low], &home)
            .into_iter()
            .find(|r| r.path.ends_with("Caches"))
            .unwrap();
        let mut warns = Vec::new();
        let units = enumerate_units(&root, &crate::config::WalkCaps::default(), &mut |w| {
            warns.push(w)
        });
        let names: Vec<&str> = units
            .iter()
            .map(|u| u.path.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, vec!["com.vendor.app"], "cache roots keep dirs only");
        assert_eq!(units[0].kind, Kind::Folder);
    }

    #[test]
    fn missing_root_is_skipped() {
        let _dir = tempfile::tempdir().unwrap();
        let roots = resolve_roots(&[ScopeCategory::High], &_dir.path());
        assert!(roots.is_empty(), "no Documents/Pictures in scratch dir");
    }
}
