//! Privacy-tier payload construction (plan §3.3.3).
//!
//! - Minimal: kind / size / dates / orphan / ext only — no paths, no
//!   filenames. Redaction is enforced by tests (plan §5).
//! - Standard: + full paths and filenames, never content.
//! - Deep: + depth-1 folder listing capped at 100 entries, then "+N more"
//!   (Decision #5).

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::types::{Candidate, FieldState, Kind, PrivacyTier};

/// Render one tri-state field for the model: the real value when known, or
/// the state's honest text. An unavailable/error field is NEVER rendered as
/// a zero/0 B — the model must see that it was not inspected.
fn field_or_text<T: serde::Serialize>(state: &FieldState, value: impl FnOnce() -> T) -> Value {
    match state {
        FieldState::Known => json!(value()),
        _ => json!(state.describe()),
    }
}

/// Build the JSON payload for one candidate under the given privacy tier.
pub fn build_payload(
    candidate: &Candidate,
    privacy: PrivacyTier,
    deep_listing: Option<&Vec<String>>,
    deep_listing_cap: usize,
) -> Value {
    let ext = Path::new(&candidate.path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_lowercase()));

    let mut payload = json!({
        "id": candidate.id,
        "kind": candidate.kind,
        // Tri-state: a number when measured, otherwise the honest text
        // "unavailable" / "unavailable (permission denied)".
        "size": field_or_text(&candidate.size_state, || candidate.size_bytes),
        "readable": candidate.readable,
        // Each date is labeled with the field it came from. "last_used" is
        // only ever present when Spotlight actually recorded a use;
        // "last_modified"/"created" are filesystem timestamps, NOT usage.
        "last_used": field_or_text(&candidate.last_used_state, || candidate.last_used_date),
        "last_used_source": if candidate.last_used_from_spotlight { "spotlight" } else { "unknown" },
        "last_modified": field_or_text(&candidate.modified_state, || candidate.modified_date),
        "created": field_or_text(&candidate.created_state, || candidate.created_date),
        "orphan_status": candidate.orphan_status,
        "ext": ext,
        "drift_score": (candidate.score * 10.0).round() / 10.0,
        "band": candidate.band,
    });

    // An age that reads as "usage" is only emitted when usage is actually
    // known (Spotlight). A directory's mtime is not last-used, so it is
    // never turned into an age figure the model could read as activity.
    if candidate.last_used_from_spotlight {
        if let Some(d) = candidate.last_used_date {
            payload["age_days_since_last_use"] =
                json!((chrono::Utc::now() - d).num_days().max(0));
        }
    }
    if let Some(reason) = &candidate.read_error {
        payload["read_error"] = json!(reason);
    }

    if candidate.kind != Kind::File {
        payload["children"] = field_or_text(&candidate.children_state, || {
            candidate
                .kind_stats
                .as_ref()
                .map(|s| s.children)
                .unwrap_or(0)
        });
    }

    if privacy != PrivacyTier::Minimal {
        payload["path"] = json!(candidate.path);
    }

    if privacy == PrivacyTier::Deep {
        if let Some(listing) = deep_listing {
            let cap = deep_listing_cap.max(1);
            let (shown, more) = if listing.len() > cap {
                (&listing[..cap], listing.len() - cap)
            } else {
                (listing.as_slice(), 0)
            };
            let mut items: Vec<Value> = shown.iter().map(|n| json!(n)).collect();
            if more > 0 {
                items.push(json!(format!("+{} more", more)));
            }
            payload["folder_listing_depth1"] = json!(items);
        }
    }

    payload
}

/// Build payloads for a batch. `deep_listings` maps candidate id → listing.
pub fn build_batch_payloads(
    candidates: &[Candidate],
    privacy: PrivacyTier,
    deep_listings: &HashMap<String, Vec<String>>,
    deep_listing_cap: usize,
) -> Vec<Value> {
    candidates
        .iter()
        .map(|c| {
            build_payload(
                c,
                privacy,
                deep_listings.get(&c.id),
                deep_listing_cap,
            )
        })
        .collect()
}

/// Collect the depth-1 entry names of a folder for the Deep tier listing.
/// Names + extensions only (Decision #5); dotfiles skipped.
pub fn folder_listing(path: &Path, cap: usize) -> Option<Vec<String>> {
    let mut names = Vec::new();
    let rd = std::fs::read_dir(path).ok()?;
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        names.push(name);
        if names.len() > cap {
            break;
        }
    }
    Some(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Band, FieldState, OrphanStatus, ScopeCategory, ScoreComponents};
    use chrono::{TimeZone, Utc};

    fn candidate(path: &str, score: f64) -> Candidate {
        Candidate {
            id: "c-abc123".into(),
            path: path.into(),
            kind: Kind::Folder,
            size_bytes: 123_456_789,
            kind_stats: Some(crate::types::KindStats {
                children: 42,
                files: 40,
                cache_like_ratio: 0.5,
                truncated: false,
            }),
            last_used_date: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
            last_used_from_spotlight: true,
            modified_date: None,
            created_date: None,
            size_state: FieldState::Known,
            children_state: FieldState::Known,
            modified_state: FieldState::Unavailable,
            created_state: FieldState::Unavailable,
            last_used_state: FieldState::Known,
            readable: true,
            read_error: None,
            orphan_status: OrphanStatus::Orphaned,
            scope_category: ScopeCategory::Low,
            score,
            score_components: ScoreComponents::default(),
            band: Band::Middle,
            auto_high_basis: None,
        }
    }

    #[test]
    fn minimal_redacts_paths_and_filenames() {
        let c = candidate("/Users/x/Library/Caches/com.secretapp/Data/assets/big.bin", 55.0);
        let payload = build_payload(&c, PrivacyTier::Minimal, None, 100);
        let text = serde_json::to_string(&payload).unwrap();
        assert!(!text.contains("com.secretapp"));
        assert!(!text.contains("Library"));
        assert!(!text.contains("assets"));
        assert!(!text.contains("big.bin"));
        assert!(!text.contains("/Users"));
        // but structural fields survive
        assert!(text.contains("\"orphan_status\":\"orphaned\""));
        assert!(text.contains("\"ext\":\".bin\""));
        assert!(text.contains("c-abc123"));
    }

    #[test]
    fn standard_includes_path() {
        let c = candidate("/Users/x/Library/Caches/com.secretapp", 55.0);
        let payload = build_payload(&c, PrivacyTier::Standard, None, 100);
        let text = serde_json::to_string(&payload).unwrap();
        assert!(text.contains("com.secretapp"));
    }

    #[test]
    fn deep_adds_capped_listing() {
        let c = candidate("/Users/x/bigfolder", 55.0);
        let listing: Vec<String> = (0..150).map(|i| format!("entry{i}.dat")).collect();
        let payload = build_payload(&c, PrivacyTier::Deep, Some(&listing), 100);
        let arr = payload["folder_listing_depth1"].as_array().unwrap();
        assert_eq!(arr.len(), 101, "100 entries + '+N more'");
        assert_eq!(arr[100].as_str().unwrap(), "+50 more");
        assert!(!serde_json::to_string(&payload).unwrap().contains("entry149"));
    }

    #[test]
    fn deep_listing_collection() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.cache")), "x").unwrap();
        }
        std::fs::write(dir.path().join(".hidden"), "x").unwrap();
        let listing = folder_listing(dir.path(), 100).unwrap();
        assert_eq!(listing.len(), 5);
    }
}
