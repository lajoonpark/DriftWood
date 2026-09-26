//! Local rule engine + memory folder (notes §10, plan Phase 7).
//!
//! Memory layout under `<base>/memory/`:
//! - `preferences.jsonl` — append-only raw user corrections
//! - `rules.json`        — distilled deterministic rules (user-editable)
//! - `session-log.jsonl` — audit trail
//! - `never-orphan.json` — optional extension of the never-orphan list
//!
//! A matching rule PINS the tier and skips the LLM for that candidate.
//! Rules are plain JSON; hot-reload on mtime change.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{Band, OrphanStatus, Tier};

/// A distilled or user-authored rule (plan §2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ext: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_band: Option<SizeBand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orphan: Option<OrphanStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_band: Option<Band>,
    pub tier: Tier,
    pub support_count: u32,
    pub created_at: DateTime<Utc>,
    /// `distilled` rules are regenerated from preferences; `user` rules are
    /// preserved across distillation runs.
    #[serde(default = "default_source")]
    pub source: RuleSource,
}

fn default_source() -> RuleSource {
    RuleSource::Distilled
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleSource {
    Distilled,
    User,
}

/// Size bands used in rules and preference features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SizeBand {
    /// < 1 MiB
    Xs,
    /// 1 MiB – 64 MiB
    S,
    /// 64 MiB – 1 GiB
    M,
    /// 1 GiB – 10 GiB
    L,
    /// > 10 GiB
    Xl,
}

pub fn size_band(size_bytes: u64) -> SizeBand {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    match size_bytes {
        0..MIB => SizeBand::Xs,
        x if x < 64 * MIB => SizeBand::S,
        x if x < GIB => SizeBand::M,
        x if x < 10 * GIB => SizeBand::L,
        _ => SizeBand::Xl,
    }
}

/// One raw correction line in preferences.jsonl (plan §2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preference {
    pub timestamp: DateTime<Utc>,
    pub candidate_features: CandidateFeatures,
    pub original_tier: Tier,
    pub corrected_tier: Tier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Features extracted from a candidate at correction time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateFeatures {
    pub ext: Option<String>,
    pub parent_folder: String,
    pub size_band: SizeBand,
    pub orphan: OrphanStatus,
    pub score_band: Band,
}

/// An audit trail line in session-log.jsonl.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    pub detail: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Rule matching
// ---------------------------------------------------------------------------

/// Does this rule match the candidate features? Every SPECIFIED field must
/// match (all fields absent = never matches: a rule must constrain
/// something).
pub fn rule_matches(rule: &Rule, features: &CandidateFeatures) -> bool {
    let mut any = false;
    if let Some(ext) = &rule.ext {
        any = true;
        if features.ext.as_deref().map(|e| e.to_lowercase()) != Some(ext.to_lowercase()) {
            return false;
        }
    }
    if let Some(folder) = &rule.folder_name {
        any = true;
        if !folder.eq_ignore_ascii_case(&features.parent_folder) {
            return false;
        }
    }
    if let Some(parent) = &rule.parent_folder {
        any = true;
        if !parent.eq_ignore_ascii_case(&features.parent_folder) {
            return false;
        }
    }
    if let Some(size_band) = rule.size_band {
        any = true;
        if features.size_band != size_band {
            return false;
        }
    }
    if let Some(orphan) = rule.orphan {
        any = true;
        if features.orphan != orphan {
            return false;
        }
    }
    if let Some(score_band) = rule.score_band {
        any = true;
        if features.score_band != score_band {
            return false;
        }
    }
    any
}

/// Extract features from a candidate path + metadata.
pub fn extract_features(
    path: &Path,
    size_bytes: u64,
    orphan: OrphanStatus,
    score_band: Band,
) -> CandidateFeatures {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_lowercase()));
    let parent_folder = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    CandidateFeatures {
        ext,
        parent_folder,
        size_band: size_band(size_bytes),
        orphan,
        score_band,
    }
}

// ---------------------------------------------------------------------------
// Memory store
// ---------------------------------------------------------------------------

pub struct MemoryStore {
    dir: PathBuf,
}

impl MemoryStore {
    pub fn open(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn rules_path(&self) -> PathBuf {
        self.dir.join("rules.json")
    }

    pub fn preferences_path(&self) -> PathBuf {
        self.dir.join("preferences.jsonl")
    }

    pub fn session_log_path(&self) -> PathBuf {
        self.dir.join("session-log.jsonl")
    }

    /// Load rules.json. Missing file → empty rule set.
    pub fn load_rules(&self) -> crate::Result<Vec<Rule>> {
        let path = self.rules_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let text = std::fs::read_to_string(&path)?;
        serde_json::from_str(&text).map_err(Into::into)
    }

    /// Write rules.json atomically (tmp + rename) so user edits / hot
    /// reload never see a torn file.
    pub fn save_rules(&self, rules: &[Rule]) -> crate::Result<()> {
        let path = self.rules_path();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(rules)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Append a correction to preferences.jsonl (append-only).
    pub fn append_preference(&self, pref: &Preference) -> crate::Result<()> {
        let mut line = serde_json::to_string(pref)?;
        line.push('\n');
        use std::io::Write;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.preferences_path())?
            .write_all(line.as_bytes())?;
        Ok(())
    }

    pub fn load_preferences(&self) -> crate::Result<Vec<Preference>> {
        let path = self.preferences_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let text = std::fs::read_to_string(&path)?;
        let mut out = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            out.push(serde_json::from_str(line)?);
        }
        Ok(out)
    }

    /// Append an audit event to session-log.jsonl (best-effort; never
    /// fails a user-facing operation).
    pub fn log_session(&self, kind: &str, detail: serde_json::Value) {
        let event = SessionEvent {
            timestamp: Utc::now(),
            kind: kind.to_string(),
            detail,
        };
        if let Ok(mut line) = serde_json::to_string(&event) {
            line.push('\n');
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.session_log_path())
            {
                use std::io::Write;
                let _ = f.write_all(line.as_bytes());
            }
        }
    }

    /// Hot-reload check: rules.json mtime.
    pub fn rules_fingerprint(&self) -> Option<(std::time::SystemTime, u64)> {
        let meta = std::fs::metadata(self.rules_path()).ok()?;
        let mtime = meta.modified().ok()?;
        Some((mtime, meta.len()))
    }
}

// ---------------------------------------------------------------------------
// Distiller (no LLM involved)
// ---------------------------------------------------------------------------

/// Distill corrections into rules: features shared by ≥2 corrections with a
/// consistent corrected tier become a rule. Existing `user`-source rules
/// are preserved; `distilled` rules are regenerated from scratch.
pub fn distill(preferences: &[Preference], existing_rules: &[Rule]) -> Vec<Rule> {
    let user_rules: Vec<Rule> = existing_rules
        .iter()
        .filter(|r| r.source == RuleSource::User)
        .cloned()
        .collect();

    // Group corrections by feature dimension. Specific dimensions (ext,
    // folder) distill at low support; broad dimensions (size band, orphan
    // status, score band) need many more corrections — a broad rule from a
    // tiny sample would pin huge swaths of the candidate list and defeat
    // the LLM stage entirely. The plan's example rule shape
    // ({"ext": ".dmg", "folder": "Downloads"}) is the combined ext+folder
    // group.
    const SUPPORT_EXT: usize = 2;
    const SUPPORT_FOLDER: usize = 2;
    const SUPPORT_COMBINED: usize = 2;
    const SUPPORT_SIZE: usize = 4;
    const SUPPORT_ORPHAN: usize = 4;
    const SUPPORT_SCORE: usize = 5;

    let mut groups: std::collections::HashMap<(String, String), Vec<&Preference>> =
        std::collections::HashMap::new();

    for pref in preferences {
        if pref.original_tier == pref.corrected_tier {
            continue;
        }
        let f = &pref.candidate_features;
        if let Some(ext) = &f.ext {
            groups
                .entry(("ext".into(), ext.clone()))
                .or_default()
                .push(pref);
        }
        if !f.parent_folder.is_empty() {
            groups
                .entry(("folder".into(), f.parent_folder.clone()))
                .or_default()
                .push(pref);
            if let Some(ext) = &f.ext {
                groups
                    .entry((
                        "ext+folder".into(),
                        format!("{ext}\u{1f}{f}", ext = ext, f = f.parent_folder),
                    ))
                    .or_default()
                    .push(pref);
            }
        }
        let sb = format!("{:?}", f.size_band).to_lowercase();
        groups.entry(("size".into(), sb)).or_default().push(pref);
        let ob = format!("{:?}", f.orphan).to_lowercase();
        groups.entry(("orphan".into(), ob)).or_default().push(pref);
        let bb = format!("{:?}", f.score_band).to_lowercase();
        groups.entry(("score".into(), bb)).or_default().push(pref);
    }

    let mut distilled: Vec<Rule> = Vec::new();
    let mut next_id: u32 = 0;
    for ((dim, value), prefs) in groups {
        let required = match dim.as_str() {
            "ext" => SUPPORT_EXT,
            "folder" => SUPPORT_FOLDER,
            "ext+folder" => SUPPORT_COMBINED,
            "size" => SUPPORT_SIZE,
            "orphan" => SUPPORT_ORPHAN,
            "score" => SUPPORT_SCORE,
            _ => usize::MAX,
        };
        if prefs.len() < required {
            continue;
        }
        // Consistency: all corrections agree on the corrected tier.
        let tier = prefs[0].corrected_tier;
        if !prefs.iter().all(|p| p.corrected_tier == tier) {
            continue;
        }
        let rule = match dim.as_str() {
            "ext" => Rule {
                id: format!("distilled-ext-{}", slug(&value, &mut next_id)),
                ext: Some(value.clone()),
                folder_name: None,
                parent_folder: None,
                size_band: None,
                orphan: None,
                score_band: None,
                tier,
                support_count: prefs.len() as u32,
                created_at: prefs[0].timestamp,
                source: RuleSource::Distilled,
            },
            "folder" => Rule {
                id: format!("distilled-folder-{}", slug(&value, &mut next_id)),
                ext: None,
                folder_name: Some(value.clone()),
                parent_folder: None,
                size_band: None,
                orphan: None,
                score_band: None,
                tier,
                support_count: prefs.len() as u32,
                created_at: prefs[0].timestamp,
                source: RuleSource::Distilled,
            },
            "ext+folder" => {
                let (ext, folder) = value
                    .split_once('\u{1f}')
                    .map(|(e, f)| (e.to_string(), f.to_string()))
                    .unwrap_or_default();
                Rule {
                    id: format!(
                        "distilled-ext-folder-{}",
                        slug(&format!("{ext}-{folder}"), &mut next_id)
                    ),
                    ext: Some(ext.clone()),
                    folder_name: Some(folder.clone()),
                    parent_folder: None,
                    size_band: None,
                    orphan: None,
                    score_band: None,
                    tier,
                    support_count: prefs.len() as u32,
                    created_at: prefs[0].timestamp,
                    source: RuleSource::Distilled,
                }
            }
            "size" => Rule {
                id: format!("distilled-size-{}", slug(&value, &mut next_id)),
                ext: None,
                folder_name: None,
                parent_folder: None,
                size_band: Some(
                    serde_json::from_value(serde_json::Value::String(value.clone()))
                        .unwrap_or(SizeBand::M),
                ),
                orphan: None,
                score_band: None,
                tier,
                support_count: prefs.len() as u32,
                created_at: prefs[0].timestamp,
                source: RuleSource::Distilled,
            },
            "orphan" => Rule {
                id: format!("distilled-orphan-{}", slug(&value, &mut next_id)),
                ext: None,
                folder_name: None,
                parent_folder: None,
                size_band: None,
                orphan: Some(
                    serde_json::from_value(serde_json::Value::String(value.clone()))
                        .unwrap_or(OrphanStatus::Unknown),
                ),
                score_band: None,
                tier,
                support_count: prefs.len() as u32,
                created_at: prefs[0].timestamp,
                source: RuleSource::Distilled,
            },
            "score" => Rule {
                id: format!("distilled-score-{}", slug(&value, &mut next_id)),
                ext: None,
                folder_name: None,
                parent_folder: None,
                size_band: None,
                orphan: None,
                score_band: Some(
                    serde_json::from_value(serde_json::Value::String(value.clone()))
                        .unwrap_or(Band::Middle),
                ),
                tier,
                support_count: prefs.len() as u32,
                created_at: prefs[0].timestamp,
                source: RuleSource::Distilled,
            },
            _ => continue,
        };
        distilled.push(rule);
    }

    let mut out = user_rules;
    out.append(&mut distilled);
    out
}

fn slug(value: &str, counter: &mut u32) -> String {
    let clean: String = value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let clean = clean.trim_matches('-');
    *counter += 1;
    format!("{}-{}", clean.to_lowercase(), *counter)
}

/// Retrieve the N most feature-similar past corrections for few-shot
/// injection (privacy-gated by the caller).
pub fn few_shot_examples(
    preferences: &[Preference],
    features: &CandidateFeatures,
    n: usize,
) -> Vec<Preference> {
    let mut scored: Vec<(u32, &Preference)> = preferences
        .iter()
        .filter(|p| p.original_tier != p.corrected_tier)
        .map(|p| {
            let f = &p.candidate_features;
            let mut score = 0;
            if f.ext.is_some() && f.ext == features.ext {
                score += 2;
            }
            if f.parent_folder == features.parent_folder {
                score += 2;
            }
            if f.size_band == features.size_band {
                score += 1;
            }
            if f.orphan == features.orphan {
                score += 1;
            }
            if f.score_band == features.score_band {
                score += 1;
            }
            (score, p)
        })
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.timestamp.cmp(&a.1.timestamp)));
    scored.into_iter().take(n).map(|(_, p)| p.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn features(ext: Option<&str>, parent: &str, size: u64, orphan: OrphanStatus, band: Band) -> CandidateFeatures {
        CandidateFeatures {
            ext: ext.map(String::from),
            parent_folder: parent.into(),
            size_band: size_band(size),
            orphan,
            score_band: band,
        }
    }

    #[test]
    fn size_bands() {
        assert_eq!(size_band(0), SizeBand::Xs);
        assert_eq!(size_band(512 * 1024), SizeBand::Xs);
        assert_eq!(size_band(10 * 1024 * 1024), SizeBand::S);
        assert_eq!(size_band(100 * 1024 * 1024), SizeBand::M);
        assert_eq!(size_band(2 * 1024 * 1024 * 1024), SizeBand::L);
        assert_eq!(size_band(50 * 1024 * 1024 * 1024u64), SizeBand::Xl);
    }

    #[test]
    fn rule_matching_requires_all_specified_fields() {
        let rule = Rule {
            id: "r1".into(),
            ext: Some(".dmg".into()),
            folder_name: Some("Downloads".into()),
            parent_folder: None,
            size_band: None,
            orphan: None,
            score_band: None,
            tier: Tier::MessageInABottle,
            support_count: 2,
            created_at: Utc::now(),
            source: RuleSource::Distilled,
        };
        assert!(rule_matches(
            &rule,
            &features(Some(".dmg"), "Downloads", 5_000_000, OrphanStatus::Unknown, Band::Middle)
        ));
        assert!(!rule_matches(
            &rule,
            &features(Some(".dmg"), "Documents", 5_000_000, OrphanStatus::Unknown, Band::Middle)
        ));
        assert!(!rule_matches(
            &rule,
            &features(Some(".zip"), "Downloads", 5_000_000, OrphanStatus::Unknown, Band::Middle)
        ));
        // ext match is case-insensitive
        assert!(rule_matches(
            &rule,
            &features(Some(".DMG"), "downloads", 5_000_000, OrphanStatus::Unknown, Band::Middle)
        ));
    }

    #[test]
    fn rule_with_no_constraints_never_matches() {
        let rule = Rule {
            id: "r0".into(),
            ext: None,
            folder_name: None,
            parent_folder: None,
            size_band: None,
            orphan: None,
            score_band: None,
            tier: Tier::Source,
            support_count: 1,
            created_at: Utc::now(),
            source: RuleSource::User,
        };
        assert!(!rule_matches(&rule, &features(Some(".x"), "y", 1, OrphanStatus::Unknown, Band::Middle)));
    }

    fn pref(ext: Option<&str>, parent: &str, orig: Tier, corr: Tier) -> Preference {
        Preference {
            timestamp: Utc::now(),
            candidate_features: features(ext, parent, 1_000_000, OrphanStatus::Unknown, Band::Middle),
            original_tier: orig,
            corrected_tier: corr,
            note: None,
        }
    }

    #[test]
    fn distiller_needs_two_consistent_corrections() {
        let prefs = vec![
            pref(Some(".dmg"), "Downloads", Tier::Source, Tier::MessageInABottle),
            pref(Some(".dmg"), "Downloads", Tier::Source, Tier::MessageInABottle),
            pref(Some(".zip"), "Documents", Tier::Driftwood, Tier::Current),
        ];
        let rules = distill(&prefs, &[]);
        let ext_rule = rules.iter().find(|r| r.ext.as_deref() == Some(".dmg")).unwrap();
        assert_eq!(ext_rule.tier, Tier::MessageInABottle);
        assert_eq!(ext_rule.support_count, 2);
        // Single .zip correction → no rule.
        assert!(!rules.iter().any(|r| r.ext.as_deref() == Some(".zip")));
        // folder rule for Downloads from the two .dmg corrections
        assert!(rules.iter().any(|r| r.folder_name.as_deref() == Some("Downloads")));
        // the plan's example shape: combined ext+folder rule
        let combined = rules
            .iter()
            .find(|r| r.ext.as_deref() == Some(".dmg") && r.folder_name.as_deref() == Some("Downloads"));
        assert!(combined.is_some(), "combined ext+folder rule distilled");
        // broad dimensions (size/orphan/score) stay quiet at support 3
        assert!(!rules.iter().any(|r| r.score_band.is_some()));
        assert!(!rules.iter().any(|r| r.size_band.is_some()));
    }

    #[test]
    fn distiller_skips_inconsistent_tiers() {
        let prefs = vec![
            pref(Some(".log"), "Logs", Tier::Source, Tier::Driftwood),
            pref(Some(".log"), "Logs", Tier::Source, Tier::Current),
        ];
        assert!(distill(&prefs, &[]).is_empty());
    }

    #[test]
    fn distiller_preserves_user_rules() {
        let mut user_rule = pref(Some(".x"), "y", Tier::Current, Tier::Driftwood);
        user_rule.candidate_features.parent_folder = "y".into();
        let existing = vec![Rule {
            id: "mine".into(),
            ext: Some(".xyz".into()),
            folder_name: None,
            parent_folder: None,
            size_band: None,
            orphan: None,
            score_band: None,
            tier: Tier::Source,
            support_count: 1,
            created_at: Utc::now(),
            source: RuleSource::User,
        }];
        let rules = distill(&[user_rule], &existing);
        assert!(rules.iter().any(|r| r.id == "mine"));
    }

    #[test]
    fn few_shot_similarity_ranking() {
        let prefs = vec![
            pref(Some(".dmg"), "Downloads", Tier::Source, Tier::MessageInABottle),
            pref(Some(".png"), "Documents", Tier::Source, Tier::Driftwood),
        ];
        let shots = few_shot_examples(&prefs, &features(Some(".dmg"), "Downloads", 1_000_000, OrphanStatus::Unknown, Band::Middle), 1);
        assert_eq!(shots.len(), 1);
        assert_eq!(shots[0].candidate_features.ext.as_deref(), Some(".dmg"));
    }

    #[test]
    fn memory_store_roundtrip_and_append() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(dir.path()).unwrap();
        store.append_preference(&pref(Some(".a"), "b", Tier::Current, Tier::Driftwood)).unwrap();
        store.append_preference(&pref(Some(".c"), "d", Tier::Source, Tier::Source)).unwrap();
        let prefs = store.load_preferences().unwrap();
        assert_eq!(prefs.len(), 2);

        let rules = vec![Rule {
            id: "t".into(),
            ext: Some(".t".into()),
            folder_name: None,
            parent_folder: None,
            size_band: None,
            orphan: None,
            score_band: None,
            tier: Tier::Driftwood,
            support_count: 1,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            source: RuleSource::User,
        }];
        store.save_rules(&rules).unwrap();
        assert_eq!(store.load_rules().unwrap().len(), 1);
        assert!(store.rules_fingerprint().is_some());
    }

    #[test]
    fn extract_features_paths() {
        let f = extract_features(
            Path::new("/Users/x/Downloads/installer.dmg"),
            2_000_000,
            OrphanStatus::Unknown,
            Band::Middle,
        );
        assert_eq!(f.ext.as_deref(), Some(".dmg"));
        assert_eq!(f.parent_folder, "Downloads");
        assert_eq!(f.size_band, SizeBand::S);
    }
}
