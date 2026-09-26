//! Bounded folder measurement (edge case §4.4): aggregate deeper levels
//! into the parent candidate, cap per-dir enumeration, skip-and-count on
//! permission errors, never follow symlinks.

use std::path::Path;

use crate::config::WalkCaps;
use crate::types::KindStats;

/// Cache-like extensions for the file-type histogram (notes §8).
pub const CACHE_LIKE_EXTS: &[&str] = &[
    "cache", "tmp", "temp", "log", "dmp", "bak", "old", "partial", "part", "crdownload",
    "download",
];

pub fn is_cache_like_ext(ext: &str) -> bool {
    CACHE_LIKE_EXTS.contains(&ext.to_ascii_lowercase().as_str())
}

/// Result of a bounded folder measurement.
#[derive(Debug, Clone, Default)]
pub struct FolderMeasure {
    pub stats: KindStats,
    /// Bytes of regular files seen (under-counts truncated walks).
    pub bytes: u64,
}

/// Walk a folder with caps, accumulating child/file counts, bytes and a
/// cache-like extension ratio. Truncated walks are flagged in the stats.
/// Permission errors are skip-and-count, never abort (edge case §4.3).
pub fn measure_folder(
    root: &Path,
    caps: &WalkCaps,
    mut on_progress: impl FnMut(u64, u64),
) -> FolderMeasure {
    let mut measure = FolderMeasure::default();
    let stats = &mut measure.stats;
    let mut ext_hits: u64 = 0;
    let mut ext_sampled: u64 = 0;

    let walker = walkdir::WalkDir::new(root)
        .follow_links(false)
        .max_depth(caps.max_walk_depth);

    let mut seen: u64 = 0;
    for entry in walker {
        if seen >= caps.max_children_when_sizing as u64 {
            stats.truncated = true;
            break;
        }
        seen += 1;
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                // Permission error / vanished file: wading past, not aborting.
                continue;
            }
        };
        if entry.depth() == 0 {
            continue;
        }
        if entry.depth() == 1 {
            stats.children += 1;
        }
        if entry.file_type().is_dir() || entry.file_type().is_symlink() {
            continue;
        }
        stats.files += 1;
        if let Ok(meta) = entry.metadata() {
            measure.bytes = measure.bytes.saturating_add(meta.len());
        }
        if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
            ext_sampled += 1;
            if is_cache_like_ext(ext) {
                ext_hits += 1;
            }
        }
        if seen.is_multiple_of(512) {
            on_progress(seen, measure.bytes);
        }
    }
    on_progress(seen, measure.bytes);

    stats.cache_like_ratio = if ext_sampled > 0 {
        ext_hits as f64 / ext_sampled as f64
    } else {
        0.0
    };
    measure
}

/// Read a file's size via metadata (symlinks not followed).
pub fn file_size(path: &Path) -> Option<u64> {
    std::fs::symlink_metadata(path).ok().map(|m| m.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn measures_counts_and_ratio() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("bundle.cache");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("a.cache"), vec![0u8; 100]).unwrap();
        fs::write(inner.join("b.log"), vec![0u8; 50]).unwrap();
        fs::write(inner.join("c.txt"), vec![0u8; 25]).unwrap();

        let m = measure_folder(&inner, &WalkCaps::default(), |_, _| {});
        assert_eq!(m.stats.children, 3);
        assert_eq!(m.stats.files, 3);
        assert_eq!(m.bytes, 175);
        assert!((m.stats.cache_like_ratio - 2.0 / 3.0).abs() < 1e-9);
        assert!(!m.stats.truncated);
    }

    #[test]
    fn truncation_flag_when_capped() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..50 {
            fs::write(dir.path().join(format!("f{i}")), "x").unwrap();
        }
        let caps = WalkCaps {
            max_children_when_sizing: 10,
            ..WalkCaps::default()
        };
        let m = measure_folder(dir.path(), &caps, |_, _| {});
        assert!(m.stats.truncated);
        assert!(m.stats.files <= 11);
    }

    #[test]
    fn cache_ext_detection() {
        assert!(is_cache_like_ext("LOG"));
        assert!(is_cache_like_ext("tmp"));
        assert!(!is_cache_like_ext("pdf"));
    }
}
