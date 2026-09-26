//! Base-directory resolution. DriftWood writes ONLY under its own base dir
//! (memory + reports); the scanner itself is strictly read-only.

use std::path::PathBuf;

/// Base dir: `$HOME/Library/Application Support/DriftWood`, overridable via
/// `DRIFTWOOD_HOME` (used by tests and sandboxed runs).
pub fn base_dir() -> PathBuf {
    if let Ok(over) = std::env::var("DRIFTWOOD_HOME") {
        if !over.is_empty() {
            return PathBuf::from(over);
        }
    }
    home_dir().join("Library/Application Support/DriftWood")
}

pub fn memory_dir() -> PathBuf {
    base_dir().join("memory")
}

pub fn reports_dir() -> PathBuf {
    base_dir().join("reports")
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

/// Ensure the base dirs exist. Core never writes outside these.
pub fn ensure_dirs() -> std::io::Result<()> {
    for d in [memory_dir(), reports_dir()] {
        std::fs::create_dir_all(&d)?;
    }
    Ok(())
}
