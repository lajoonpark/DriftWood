//! Full Disk Access probe (read-only, plan §3.1.2 support).
//!
//! macOS gates *listing* of TCC-protected directories (Safari, Mail,
//! Messages) behind Full Disk Access. Without it those directories fail
//! with EPERM, and a naive collector would report them as empty. Detecting
//! the permission once at scan start lets DriftWood say plainly why some
//! items could not be inspected, instead of showing misleading sizes.
//!
//! `stat` on those paths still works; it is the directory listing that TCC
//! gates, which is why the probe opens the directory rather than reading
//! metadata.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullDiskAccess {
    Granted,
    Denied,
    Unknown,
}

/// Probe the first existing TCC-protected directory. `Unknown` when none of
/// the probe paths exist (a fresh or unusual account).
pub fn check(home: &Path) -> FullDiskAccess {
    for rel in ["Library/Safari", "Library/Mail", "Library/Messages"] {
        let dir = home.join(rel);
        if !dir.is_dir() {
            continue;
        }
        return match std::fs::read_dir(&dir) {
            Ok(_) => FullDiskAccess::Granted,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => FullDiskAccess::Denied,
            Err(_) => continue,
        };
    }
    FullDiskAccess::Unknown
}
