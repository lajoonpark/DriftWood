use std::path::PathBuf;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            check_full_disk_access,
            open_system_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
