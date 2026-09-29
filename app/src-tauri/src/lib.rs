use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use tauri::Emitter;

use driftwood_core::types::{PrivacyTier, ScopeCategory};
use driftwood_core::{
    apply_correction, load_rules, run_scan, CorrectionRequest, DriftError, DriftTuning, EventSink,
    Report, ScanConfig, ScanEvent, ScanHandle, Tier,
};

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

/// Reveal a path in Finder (`open -R`).
#[tauri::command]
fn reveal_path(path: String) {
    let _ = std::process::Command::new("open").arg("-R").arg(&path).status();
}

/// What a bulk reveal actually did, so the frontend can tell the user the
/// truth about windows opened and items left out.
#[derive(Debug, serde::Serialize)]
struct RevealSummary {
    /// Finder windows that were opened (one per parent directory).
    windows: usize,
    /// Items preselected across those windows.
    items: usize,
    /// Parent directories left out by the window cap.
    skipped_groups: usize,
    /// Items in those left-out directories.
    skipped_items: usize,
}

/// Reveal many paths in Finder. Grouped by parent directory (one window
/// per folder, every child preselected so the user selects all and
/// Cmd+Deletes themselves — DriftWood never deletes), capped at a handful
/// of windows with the largest groups first, and performed with ONE `open`
/// invocation. The user is told what opened and what was left out.
#[tauri::command]
fn reveal_paths(paths: Vec<String>) -> Result<RevealSummary, String> {
    use std::collections::BTreeMap;

    /// Reasonable ceiling: a mass reveal must never blanket the screen in
    /// Finder windows; overflow groups are surfaced, not silently dropped.
    const MAX_WINDOWS: usize = 6;

    let mut groups: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    let mut total_items = 0usize;
    for p in &paths {
        if let Some(parent) = std::path::Path::new(p).parent() {
            groups
                .entry(parent.to_path_buf())
                .or_default()
                .push(p.clone());
            total_items += 1;
        }
    }
    if groups.is_empty() {
        return Ok(RevealSummary {
            windows: 0,
            items: 0,
            skipped_groups: 0,
            skipped_items: 0,
        });
    }

    // Largest groups first (overflow is predictable, not arbitrary),
    // tie-broken by path so the order is stable.
    let mut ordered: Vec<&Vec<String>> = groups.values().collect();
    ordered.sort_by(|a, b| {
        b.len()
            .cmp(&a.len())
            .then_with(|| a.first().cmp(&b.first()))
    });

    let picked: Vec<&Vec<String>> = ordered.iter().take(MAX_WINDOWS).copied().collect();
    let skipped_groups = ordered.len() - picked.len();
    let skipped_items = ordered
        .iter()
        .skip(MAX_WINDOWS)
        .map(|g| g.len())
        .sum::<usize>();

    // ONE `open` invocation for the whole action — a mass reveal must not
    // spawn a process per folder.
    let mut cmd = std::process::Command::new("open");
    cmd.arg("-R");
    for group in &picked {
        for p in group.iter() {
            cmd.arg(p);
        }
    }
    let status = cmd
        .status()
        .map_err(|e| format!("could not open Finder: {e}"))?;
    if !status.success() {
        return Err("Finder did not accept the reveal".into());
    }

    Ok(RevealSummary {
        windows: picked.len(),
        items: total_items - skipped_items,
        skipped_groups,
        skipped_items,
    })
}

/// Scan options as sent by the frontend. Anything the shell does not send
/// (rules, walk caps, weights) keeps the core defaults.
#[derive(Debug, Deserialize)]
struct ScanConfigDto {
    scopes: Vec<ScopeCategory>,
    privacy_tier: PrivacyTier,
    stage2: bool,
    model: Option<String>,
    api_key: Option<String>,
    cost_cap_usd: Option<f64>,
    /// Danger zone: when true, Stage-2 calls are not restricted to
    /// zero-data-retention providers. Opt-in only; default stays protected.
    allow_non_zdr: Option<bool>,
}

/// Handle of the currently running scan, so `cancel_scan` can reach it.
#[derive(Default)]
struct ScanState {
    handle: Mutex<Option<ScanHandle>>,
}

/// Bridges core scan events to the `scan-event` window event the frontend
/// listens on.
struct TauriSink(tauri::AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, event: ScanEvent) {
        if let Err(e) = self.0.emit("scan-event", &event) {
            eprintln!("warning: could not emit scan event: {e}");
        }
    }
}

/// Run a scan. Emits progress on `scan-event`; resolves to the report in
/// the shape the frontend expects. A cancel during Stage 2 resolves to a
/// partial report (`stopped_early`, fallback-labeled remainder) instead of
/// discarding the work; only early-phase cancels resolve to "cancelled".
#[tauri::command]
async fn start_scan(
    app: tauri::AppHandle,
    state: tauri::State<'_, ScanState>,
    config: ScanConfigDto,
) -> Result<serde_json::Value, String> {
    let mut tuning = DriftTuning::default();
    if let Some(cap) = config.cost_cap_usd {
        if cap > 0.0 {
            tuning.reasoning.cost_cap_usd = cap;
        }
    }
    // The key comes from the app settings (frontend) or the environment.
    let api_key = config
        .api_key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .or_else(|| {
            std::env::var("OPENROUTER_API_KEY")
                .ok()
                .filter(|k| !k.is_empty())
        });

    let core_config = ScanConfig {
        scopes: config.scopes,
        privacy_tier: config.privacy_tier,
        stage2: config.stage2,
        model: config
            .model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| driftwood_core::default_model().to_string()),
        api_key,
        allow_non_zdr: config.allow_non_zdr.unwrap_or(false),
        rules: load_rules(None).unwrap_or_default(),
        tuning,
        persist: true,
    };

    let handle = ScanHandle::new();
    *state.handle.lock().expect("scan state poisoned") = Some(handle.clone());
    let sink: Arc<dyn EventSink> = Arc::new(TauriSink(app));
    let result = run_scan(core_config, sink, handle).await;
    *state.handle.lock().expect("scan state poisoned") = None;

    match result {
        Ok(report) => {
            let value = serde_json::to_value(&report).map_err(|e| e.to_string())?;
            Ok(report_to_frontend(value))
        }
        // Early-phase cancel (before anything worth reporting exists).
        Err(DriftError::Cancelled) => Err("cancelled".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Cancel the running scan. Stage 2 checks this inside the streaming loop,
/// so an in-flight request is aborted immediately; the scan then resolves
/// to a partial report rather than nothing.
#[tauri::command]
fn cancel_scan(state: tauri::State<'_, ScanState>) {
    if let Some(handle) = state.handle.lock().expect("scan state poisoned").as_ref() {
        handle.cancel();
    }
}

/// The last persisted report, or null when none exists yet.
#[tauri::command]
fn last_report() -> Result<Option<serde_json::Value>, String> {
    match Report::load_last() {
        Ok(report) => {
            let value = serde_json::to_value(&report).map_err(|e| e.to_string())?;
            Ok(Some(report_to_frontend(value)))
        }
        Err(_) => Ok(None),
    }
}

/// Record a user tier correction for a candidate in the last report. The
/// candidate details (path, size, orphan status, band) come from the
/// persisted report; the frontend only sends the opaque id.
#[tauri::command]
async fn correct_tier(id: String, tier: u8, note: Option<String>) -> Result<(), String> {
    let corrected = Tier::try_from(tier).map_err(|e| e.to_string())?;
    let report =
        Report::load_last().map_err(|e| format!("no report to correct against: {e}"))?;
    let entry = report
        .entries
        .iter()
        .find(|e| e.candidate.id == id)
        .ok_or_else(|| format!("unknown candidate id {id}"))?;

    let request = CorrectionRequest {
        path: entry.candidate.path.clone(),
        size_bytes: entry.candidate.size_bytes,
        orphan: entry.candidate.orphan_status,
        score_band: entry.candidate.band,
        original_tier: entry.tier,
        corrected_tier: corrected,
        note,
    };
    apply_correction(request, None)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Map the core report model onto the frontend's wire contract:
/// groups as `{category, count, bytes}`, warnings as plain strings,
/// `cost_cap` as a boolean, and `cache_loc` in score components.
fn report_to_frontend(mut report: serde_json::Value) -> serde_json::Value {
    let obj = match report.as_object_mut() {
        Some(o) => o,
        None => return report,
    };

    if let Some(groups) = obj.get_mut("groups").and_then(serde_json::Value::as_array_mut) {
        for g in groups {
            if let Some(gobj) = g.as_object_mut() {
                if let Some(n) = gobj.remove("item_count") {
                    gobj.insert("count".into(), n);
                }
                if let Some(b) = gobj.remove("total_bytes") {
                    gobj.insert("bytes".into(), b);
                }
                gobj.remove("label");
            }
        }
    }

    if let Some(warnings) = obj.get_mut("warnings").and_then(serde_json::Value::as_array_mut) {
        let messages: Vec<serde_json::Value> = warnings
            .iter()
            .filter_map(|w| w.get("message").cloned())
            .collect();
        obj.insert("warnings".into(), serde_json::Value::Array(messages));
    }

    if let Some(hit) = obj.remove("cost_cap_hit") {
        obj.insert("cost_cap".into(), hit);
    }

    if let Some(entries) = obj.get_mut("entries").and_then(serde_json::Value::as_array_mut) {
        for entry in entries {
            if let Some(components) = entry
                .get_mut("candidate")
                .and_then(|c| c.get_mut("score_components"))
                .and_then(serde_json::Value::as_object_mut)
            {
                if let Some(v) = components.remove("cache_location") {
                    components.insert("cache_loc".into(), v);
                }
            }
        }
    }

    report
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(ScanState::default())
        .invoke_handler(tauri::generate_handler![
            check_full_disk_access,
            open_system_settings,
            reveal_path,
            reveal_paths,
            start_scan,
            cancel_scan,
            last_report,
            correct_tier
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn report_shape_matches_frontend_contract() {
        let report = json!({
            "schema_version": 1,
            "groups": [
                {"category": "low", "label": "Low personal risk", "item_count": 2, "total_bytes": 1000}
            ],
            "entries": [
                {"candidate": {"id": "c-1", "path": "/x", "score_components":
                    {"size": 1.0, "cache_location": 15.0}}},
            ],
            "warnings": [{"kind": "cost_cap", "message": "cap hit"}],
            "cost_cap_hit": true
        });
        let v = report_to_frontend(report);
        assert_eq!(v["groups"][0]["count"], 2);
        assert_eq!(v["groups"][0]["bytes"], 1000);
        assert!(v["groups"][0].get("label").is_none());
        assert_eq!(v["warnings"][0], "cap hit");
        assert_eq!(v["cost_cap"], true);
        assert!(v["cost_cap_hit"].is_null());
        assert_eq!(v["entries"][0]["candidate"]["score_components"]["cache_loc"], 15.0);
        assert!(v["entries"][0]["candidate"]["score_components"]
            .get("cache_location")
            .is_none());
    }
}
