//! The engine: the ONLY two async entry points both wrappers call.
//!
//! `run_scan(config, sink, handle)` → Report
//! `apply_correction(request, memory_dir)` → updated rules
//!
//! Pipeline order matters (plan §3.1): walk → hard rules → orphans → score.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use chrono::Utc;

use crate::config::ScanConfig;
use crate::events::{EventSink, Phase, ScanEvent};
use crate::reason::{self, ReasonOutcome};
use crate::report::{self, AssembleInput, CounterState, Report, ReportEntry, ReportWarning};
use crate::rules::{self, Rule, RuleSource};
use crate::scan::{hard_rules, orphan, scope, spotlight};
use crate::score::{self, banding, ScoreInput};
use crate::types::{
    Band, Candidate, Kind, KindStats, OrphanStatus, Tier, TierSource,
};
use crate::{DriftError, Result};

/// Handle for cancelling a running scan. Cancellation is checked between
/// phases and between LLM batches — prompt, never mid-batch (edge §4.8).
#[derive(Clone, Default)]
pub struct ScanHandle {
    cancel: Arc<AtomicBool>,
}

impl ScanHandle {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Single-flight lock (edge case §4.8): one scan per install.
pub struct ScanLock {
    path: PathBuf,
}

impl ScanLock {
    pub fn acquire(base_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(base_dir)?;
        let path = base_dir.join("scan.lock");
        let acquired = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path);
        match acquired {
            Ok(_) => {
                let pid = std::process::id();
                let stamp = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                std::fs::write(&path, format!("{pid} {stamp}\n"))?;
                Ok(Self { path })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Stale lock (crashed scan older than 6h)? Steal it.
                let stale = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .map(|elapsed| elapsed > Duration::from_secs(6 * 3600))
                    .unwrap_or(false);
                if stale {
                    let _ = std::fs::remove_file(&path);
                    return Self::acquire(base_dir);
                }
                Err(DriftError::SingleFlight(path.display().to_string()))
            }
            Err(e) => Err(e.into()),
        }
    }
}

impl Drop for ScanLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub async fn run_scan(
    config: ScanConfig,
    sink: Arc<dyn EventSink>,
    handle: ScanHandle,
) -> Result<Report> {
    let _lock = if config.persist {
        Some(ScanLock::acquire(&crate::paths::base_dir())?)
    } else {
        None
    };

    let scan_id = report::new_scan_id();
    let tuning = config.tuning.clone();
    let counters = Arc::new(CounterState::default());
    let home = crate::paths::home_dir();
    let now = Utc::now();

    // ---- Phase: Enumerating ------------------------------------------------
    sink.emit(ScanEvent::Phase {
        phase: Phase::Enumerating,
    });
    let roots = scope::resolve_roots(&config.scopes, &home);
    if roots.is_empty() {
        return Err(DriftError::Scan(
            "no scope roots exist for the selected categories".into(),
        ));
    }

    let mut warns: Vec<ReportWarning> = Vec::new();
    let mut units: Vec<scope::Unit> = Vec::new();
    for root in &roots {
        if handle.is_cancelled() {
            return Err(DriftError::Cancelled);
        }
        let mut warn_list: Vec<String> = Vec::new();
        units.extend(scope::enumerate_units(root, &tuning.walk, &mut |w| {
            warn_list.push(w)
        }));
        for w in warn_list {
            sink.emit(ScanEvent::warn(w.clone()));
        }
    }
    counters
        .candidates_found
        .store(units.len() as u64, Ordering::Relaxed);
    sink.emit(ScanEvent::CandidatesFound {
        total: units.len() as u64,
    });

    if units.is_empty() {
        sink.emit(ScanEvent::notice("Clear waters — nothing to classify"));
        return finish(
            AssembleInput {
                scan_id,
                spotlight_available: true,
                privacy_tier: config.privacy_tier,
                model: None,
                llm_cost_usd: 0.0,
                cost_cap_hit: false,
                stopped_early: false,
                counters: counters.snapshot(),
                entries: vec![],
                warnings: vec![],
            },
            config.persist,
        );
    }

    // ---- Phase: Wading in (metadata) ---------------------------------------
    sink.emit(ScanEvent::Phase { phase: Phase::Wading });
    let spotlight_ok = spotlight::spotlight_available().await;
    if !spotlight_ok {
        warns.push(ReportWarning {
            kind: "murky_spotlight".into(),
            message: "The river ran murky — recent-use data was incomplete (Spotlight indexing unavailable); age signals rely on file dates only.".into(),
        });
    }

    // Spotlight recency sets per root (positive recency only ever filters).
    let mut recent: std::collections::HashSet<PathBuf> = Default::default();
    if spotlight_ok {
        for root in &roots {
            if handle.is_cancelled() {
                return Err(DriftError::Cancelled);
            }
            recent.extend(
                spotlight::recently_used_under(&root.path, tuning.walk.recency_days).await,
            );
        }
    }

    let unit_paths: Vec<PathBuf> = units.iter().map(|u| u.path.clone()).collect();
    let last_used = spotlight::query_last_used(&unit_paths, tuning.walk.mdls_batch_size).await;

    // File dates via metadata (mtime is never "last used"; created date is
    // the Downloads exception).
    let mut modified: HashMap<PathBuf, chrono::DateTime<Utc>> = HashMap::new();
    let mut created: HashMap<PathBuf, chrono::DateTime<Utc>> = HashMap::new();
    for p in &unit_paths {
        if let Ok(meta) = std::fs::symlink_metadata(p) {
            if let Ok(t) = meta.modified() {
                modified.insert(p.clone(), t.into());
            }
            if let Ok(t) = meta.created() {
                created.insert(p.clone(), t.into());
            }
        }
    }

    // ---- Phase: Filtering (hard rules + orphans) ---------------------------
    sink.emit(ScanEvent::Phase {
        phase: Phase::Filtering,
    });
    let mut dropped_recency = 0usize;
    let mut survivors: Vec<scope::Unit> = Vec::new();

    // Mark units touched recently: exact unit paths, plus any unit that
    // is an ancestor of a recently-used inner path (a folder whose
    // contents are being written is still in the current — the safe
    // direction is to leave it alone). O(recent × path-depth).
    let mut recently_touched: std::collections::HashSet<PathBuf> = Default::default();
    if spotlight_ok {
        let unit_set: std::collections::HashSet<&PathBuf> =
            units.iter().map(|u| &u.path).collect();
        for p in &recent {
            if unit_set.contains(p) {
                recently_touched.insert(p.clone());
            } else {
                let mut anc = p.parent();
                while let Some(a) = anc {
                    if unit_set.contains(&a.to_path_buf()) {
                        recently_touched.insert(a.to_path_buf());
                        break;
                    }
                    anc = a.parent();
                }
            }
        }
    }

    for unit in &units {
        let lu = last_used.get(&unit.path).copied().flatten();
        let from_spotlight = lu.is_some() && spotlight_ok;
        if hard_rules::still_in_current(lu, from_spotlight, now, tuning.walk.recency_days)
            || recently_touched.contains(&unit.path)
        {
            dropped_recency += 1;
            continue;
        }
        survivors.push(unit.clone());
    }
    sink.emit(ScanEvent::notice(format!(
        "still in the current: {dropped_recency} recently-used items filtered out"
    )));

    if survivors.is_empty() {
        return finish(
            AssembleInput {
                scan_id,
                spotlight_available: spotlight_ok,
                privacy_tier: config.privacy_tier,
                model: None,
                llm_cost_usd: 0.0,
                cost_cap_hit: false,
                stopped_early: false,
                counters: counters.snapshot(),
                entries: vec![],
                warnings: warns,
            },
            config.persist,
        );
    }

    // Orphan detection: installed apps snapshot + never-orphan list.
    let installed = orphan::enumerate_installed_apps(&home, 4000);
    let never_orphan = orphan::load_never_orphan_list(&crate::paths::memory_dir());
    let mut orphan_status: HashMap<PathBuf, OrphanStatus> = HashMap::new();
    for unit in &survivors {
        let status = if unit.kind == Kind::File {
            OrphanStatus::Unknown
        } else {
            let name = unit
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            orphan::classify_orphan(&name, &installed, &never_orphan)
        };
        orphan_status.insert(unit.path.clone(), status);
    }

    // ---- Sizing (bounded, concurrent, counted) -----------------------------
    let mut measures: HashMap<PathBuf, (u64, Option<KindStats>)> = HashMap::new();
    let mut join = tokio::task::JoinSet::new();
    let max_concurrent = 8usize;
    let mut inflight = 0usize;

    for unit in survivors.clone() {
        if handle.is_cancelled() {
            return Err(DriftError::Cancelled);
        }
        let caps = tuning.walk.clone();
        let counters = counters.clone();
        let sink = sink.clone();
        let path = unit.path.clone();
        let is_file = unit.kind == Kind::File;

        if is_file {
            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                let size = meta.len();
                counters.files_searched.fetch_add(1, Ordering::Relaxed);
                counters.bytes_searched.fetch_add(size, Ordering::Relaxed);
                measures.insert(path, (size, None));
            }
            continue;
        }

        while inflight >= max_concurrent {
            if let Some(done) = join.join_next().await {
                inflight -= 1;
                if let Ok((path, size, stats)) = done {
                    measures.insert(path, (size, Some(stats)));
                }
            }
        }
        inflight += 1;
        join.spawn_blocking(move || {
            let m = crate::scan::walk::measure_folder(&path, &caps, |files, bytes| {
                counters.files_searched.fetch_add(files, Ordering::Relaxed);
                counters.bytes_searched.fetch_add(bytes, Ordering::Relaxed);
                sink.emit(ScanEvent::FilesSearched {
                    total: counters.files_searched.load(Ordering::Relaxed),
                });
                sink.emit(ScanEvent::BytesSearched {
                    total: counters.bytes_searched.load(Ordering::Relaxed),
                });
            });
            let stats = KindStats {
                children: m.stats.children,
                files: m.stats.files,
                cache_like_ratio: m.stats.cache_like_ratio,
                truncated: m.stats.truncated,
            };
            (path, m.bytes, stats)
        });
    }
    while let Some(done) = join.join_next().await {
        if let Ok((path, size, stats)) = done {
            measures.insert(path, (size, Some(stats)));
        }
    }

    // ---- Phase: Scoring + banding ------------------------------------------
    sink.emit(ScanEvent::Phase { phase: Phase::Scoring });
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);

    let mut candidates: Vec<Candidate> = Vec::with_capacity(survivors.len());
    for (idx, unit) in survivors.iter().enumerate() {
        let (size_bytes, stats) = measures
            .get(&unit.path)
            .cloned()
            .unwrap_or((0, None));
        let kind_stats = stats.filter(|_| unit.kind != Kind::File);

        let cache_like_ratio = match unit.kind {
            Kind::File => {
                let ext = unit
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(crate::scan::walk::is_cache_like_ext)
                    .unwrap_or(false);
                if ext {
                    1.0
                } else {
                    0.0
                }
            }
            _ => kind_stats.as_ref().map(|s| s.cache_like_ratio).unwrap_or(0.0),
        };

        // Age reference: Spotlight last-used, else modified, else created.
        // The Downloads exception treats creation as meaningful.
        let lu = last_used.get(&unit.path).copied().flatten();
        let age_ref = match lu {
            Some(t) => Some(t),
            None => modified.get(&unit.path).copied().or_else(|| created.get(&unit.path).copied()),
        };
        let age_days = age_ref
            .map(|t| (now - t).num_days().max(0) as f64);

        // Depth relative to home (fallback: absolute component count).
        let depth = unit
            .path
            .strip_prefix(&home)
            .map(|p| p.components().count())
            .unwrap_or_else(|_| unit.path.components().count())
            .max(1);

        let input = ScoreInput {
            kind: unit.kind,
            size_bytes,
            age_days,
            scope_category: unit.category,
            root_cache_like: unit.root.cache_like,
            orphan_status: orphan_status
                .get(&unit.path)
                .copied()
                .unwrap_or(OrphanStatus::Unknown),
            depth,
            cache_like_ratio,
            children: kind_stats.as_ref().map(|s| s.children).unwrap_or(0),
        };
        let components = score::compute(&input, &tuning.weights);

        candidates.push(Candidate {
            id: report::candidate_id(&unit.path, nonce.wrapping_add(idx as u64)),
            path: unit.path.to_string_lossy().into_owned(),
            kind: unit.kind,
            size_bytes,
            kind_stats,
            last_used_date: lu,
            last_used_from_spotlight: lu.is_some() && spotlight_ok,
            modified_date: modified.get(&unit.path).copied(),
            created_date: created.get(&unit.path).copied(),
            orphan_status: input.orphan_status,
            scope_category: unit.category,
            score: components.total(),
            score_components: components,
            band: Band::Middle,
        });
    }

    // Banding (Decision #4).
    let mut indexed: Vec<(usize, f64)> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| (i, c.score))
        .collect();
    let bands = banding::assign_bands(&mut indexed, &tuning.banding);
    for (c, band) in candidates.iter_mut().zip(bands) {
        c.band = band;
    }

    counters
        .recoverable_bytes
        .store(candidates.iter().map(|c| c.size_bytes).sum(), Ordering::Relaxed);
    counters.emit(&*sink);

    // ---- Local rules pin tiers before any LLM call (Phase 7) ---------------
    let mut pinned: HashMap<String, (Tier, String)> = HashMap::new(); // id → (tier, rule_id)
    let mut never_flagged: std::collections::HashSet<String> = Default::default();
    for c in &candidates {
        if hard_rules::is_never_flagged(Path::new(&c.path), &home)
            || hard_rules::is_group_container(Path::new(&c.path), &home)
        {
            never_flagged.insert(c.id.clone());
            continue;
        }
        let features = rules::extract_features(
            Path::new(&c.path),
            c.size_bytes,
            c.orphan_status,
            c.band,
        );
        if let Some(rule) = config
            .rules
            .iter()
            .find(|r| rules::rule_matches(r, &features))
        {
            pinned.insert(c.id.clone(), (rule.tier, rule.id.clone()));
        }
    }

    // ---- Folder-type auto-High (newideas Fix C) ----------------------------
    // Cache/log roots in the Low scope are driftwood by definition — the
    // relative banding quantiles would otherwise send hundreds of obvious
    // cache folders to the LLM. Upgraded AFTER banding + pinning so
    // assign_bands can't overwrite it and user rules still see the
    // quantile band in their features; rules and never-flag still win.
    let mut auto_highed = 0usize;
    for c in candidates.iter_mut() {
        if c.band == Band::Middle && is_definitionally_driftwood(Path::new(&c.path), &home, c.orphan_status) {
            c.band = Band::High;
            auto_highed += 1;
        }
    }
    if auto_highed > 0 {
        sink.emit(ScanEvent::notice(format!(
            "{auto_highed} cache-root folders scored straight to driftwood (no AI needed)"
        )));
    }

    // ---- Stage 2 (LLM) ------------------------------------------------------
    let mut outcome: Option<ReasonOutcome> = None;
    let middle: Vec<Candidate> = candidates
        .iter()
        .filter(|c| c.band == Band::Middle && !pinned.contains_key(&c.id) && !never_flagged.contains(&c.id))
        .cloned()
        .collect();

    if config.stage2 {
        if let Some(api_key) = config.api_key.clone().filter(|k| !k.is_empty()) {
            // Few-shot: privacy-gated; union of feature-similar past
            // corrections across the middle band, deduped.
            let mut few_shot: Vec<serde_json::Value> = Vec::new();
            if crate::reason::prompt::few_shot_allowed(config.privacy_tier, tuning.reasoning.few_shot)
                && !middle.is_empty()
            {
                let mut seen = std::collections::HashSet::new();
                match crate::rules::MemoryStore::open(crate::paths::memory_dir()) {
                    Ok(store) => match store.load_preferences() {
                        Ok(prefs) => {
                            for c in middle.iter().take(tuning.reasoning.few_shot_count * 4) {
                                let features = rules::extract_features(
                                    Path::new(&c.path),
                                    c.size_bytes,
                                    c.orphan_status,
                                    c.band,
                                );
                                for p in rules::few_shot_examples(
                                    &prefs,
                                    &features,
                                    tuning.reasoning.few_shot_count,
                                ) {
                                    let key = serde_json::to_string(&p.candidate_features)
                                        .unwrap_or_default();
                                    if seen.insert(key) {
                                        few_shot.push(crate::reason::prompt::few_shot_value(
                                            &p.candidate_features,
                                            p.corrected_tier as u8,
                                        ));
                                    }
                                }
                                if few_shot.len() >= tuning.reasoning.few_shot_count {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            sink.emit(ScanEvent::warn(format!("preferences unreadable: {e}")))
                        }
                    },
                    Err(e) => sink.emit(ScanEvent::warn(format!("memory folder unavailable: {e}"))),
                }
            }

            match reason::reason_middle_band(
                middle.clone(),
                config.privacy_tier,
                &tuning.reasoning,
                &config.model,
                &api_key,
                !config.allow_non_zdr,
                &scan_id,
                &few_shot,
                sink.clone(),
                handle.cancel.clone(),
            )
            .await
            {
                Ok(o) => outcome = Some(o),
                // Cancel during reasoning is no longer fatal: the partial
                // outcome (fallback-labeled remainder) is returned as the
                // report. Early-phase cancels above still abort outright —
                // there is nothing worth reporting yet.
                Err(e) => {
                    sink.emit(ScanEvent::warn(format!("river crossing failed: {e}")));
                    warns.push(ReportWarning {
                        kind: "llm_failures".into(),
                        message: format!("Stage 2 could not complete: {e}"),
                    });
                }
            }
        } else {
            warns.push(ReportWarning {
                kind: "stage2_no_key".into(),
                message: "No OpenRouter key configured — middle-band items use heuristic tiers only.".into(),
            });
        }
    }

    // ---- Assemble entries ----------------------------------------------------
    sink.emit(ScanEvent::Phase {
        phase: Phase::Assembling,
    });
    let mut entries: Vec<ReportEntry> = Vec::with_capacity(candidates.len());
    let mut fallback_count = 0usize;
    for c in &candidates {
        let (tier, tier_source, summary, reasoning, confidence, model_used) =
            if never_flagged.contains(&c.id) {
                (
                    Tier::Source,
                    TierSource::NeverFlag,
                    "Protected location — never flagged for deletion.".to_string(),
                    String::new(),
                    1.0,
                    None,
                )
            } else if let Some((tier, rule_id)) = pinned.get(&c.id) {
                (
                    *tier,
                    TierSource::Rule,
                    format!("Pinned by your rule {rule_id}."),
                    String::new(),
                    1.0,
                    None,
                )
            } else {
                match (c.band, &outcome) {
                    (Band::High, _) => (
                        Tier::Driftwood,
                        TierSource::AutoHigh,
                        "High drift score — clear driftwood, safe to clear.".to_string(),
                        String::new(),
                        1.0,
                        None,
                    ),
                    (Band::Low, _) => (
                        Tier::Source,
                        TierSource::AutoLow,
                        "Low drift score — still part of your everyday water.".to_string(),
                        String::new(),
                        1.0,
                        None,
                    ),
                    (Band::Middle, Some(o)) if o.judgments.contains_key(&c.id) => {
                        let j = &o.judgments[&c.id];
                        (
                            Tier::try_from(j.tier).unwrap_or(Tier::Current),
                            TierSource::Llm,
                            j.summary.clone(),
                            j.reasoning.clone(),
                            j.confidence,
                            Some(config.model.clone()),
                        )
                    }
                    (Band::Middle, Some(o)) if o.propagated_judgments.contains_key(&c.id) => {
                        // One shared cluster verdict, honestly labeled —
                        // never presented as an independent LLM judgment.
                        let j = &o.propagated_judgments[&c.id];
                        (
                            Tier::try_from(j.tier).unwrap_or(Tier::Current),
                            TierSource::LlmPropagated,
                            format!("{} (shared with its folder's cluster — one judgment covers the group.)", j.summary),
                            j.reasoning.clone(),
                            j.confidence,
                            Some(config.model.clone()),
                        )
                    }
                    (Band::Middle, _) => {
                        fallback_count += 1;
                        (
                            reason::fallback_tier(c.score),
                            TierSource::Fallback,
                            "Snagged — the river ran rough here; heuristic estimate only."
                                .to_string(),
                            String::new(),
                            0.3,
                            None,
                        )
                    }
                }
            };

        entries.push(ReportEntry {
            candidate: c.clone(),
            tier,
            tier_source,
            summary,
            reasoning,
            confidence,
            llm_model: model_used,
            privacy_tier_used: config.privacy_tier,
            rule_id: pinned.get(&c.id).map(|(_, r)| r.clone()),
        });
    }

    if let Some(o) = &outcome {
        if o.cost_cap_hit {
            warns.push(ReportWarning {
                kind: "cost_cap".into(),
                message: "Snagged — ran out of river: the per-scan cost cap was hit; remaining items fell back to heuristic tiers.".into(),
            });
        }
        if o.cancelled {
            warns.push(ReportWarning {
                kind: "stopped_early".into(),
                message: "You pulled the scan ashore — it stopped early. Everything judged so far is in this report; unjudged items use heuristic tiers (marked as fallback).".into(),
            });
        }
        if fallback_count > 0 {
            warns.push(ReportWarning {
                kind: "llm_failures".into(),
                message: format!("{fallback_count} items could not be reasoned about and use heuristic tiers (marked as fallback)."),
            });
        }
        sink.emit(ScanEvent::notice(format!(
            "river crossing cost {}",
            reason::format_cost(o.total_cost_usd)
        )));
    } else if !config.stage2 && !middle.is_empty() {
        warns.push(ReportWarning {
            kind: "stage2_skipped".into(),
            message: format!(
                "Stage 2 disabled — {} middle-band items use heuristic tier estimates.",
                middle.len()
            ),
        });
    }

    finish(
        AssembleInput {
            scan_id,
            spotlight_available: spotlight_ok,
            privacy_tier: config.privacy_tier,
            model: if config.stage2 { Some(config.model.clone()) } else { None },
            llm_cost_usd: outcome.as_ref().map(|o| o.total_cost_usd).unwrap_or(0.0),
            cost_cap_hit: outcome.as_ref().map(|o| o.cost_cap_hit).unwrap_or(false),
            stopped_early: outcome.as_ref().map(|o| o.cancelled).unwrap_or(false),
            counters: counters.snapshot(),
            entries,
            warnings: warns,
        },
        config.persist,
    )
}

fn finish(input: AssembleInput, persist: bool) -> Result<Report> {
    let r = report::assemble(input);
    if persist {
        if let Err(e) = r.persist() {
            // Persistence failure must not lose the scan result.
            eprintln!("warning: could not persist report: {e}");
        }
    }
    Ok(r)
}

/// Cache roots whose contents are driftwood by definition: caches, logs,
/// temp dirs, and folders left behind by uninstalled apps under
/// Application Support. Their top-level units are never sent to the LLM.
fn is_definitionally_driftwood(path: &Path, home: &Path, orphan: OrphanStatus) -> bool {
    let cache_roots = [
        home.join("Library/Caches"),
        home.join("Library/Logs"),
        PathBuf::from("/tmp"),
        PathBuf::from("/private/tmp"),
    ];
    if cache_roots.iter().any(|r| path.starts_with(r)) {
        return true;
    }
    // Orphaned Application Support: the owning app is gone.
    orphan == OrphanStatus::Orphaned
        && path.starts_with(home.join("Library/Application Support"))
}

// ---------------------------------------------------------------------------
// apply_correction — the second entry point (plan §1)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CorrectionRequest {
    /// Candidate path (from the report entry).
    pub path: String,
    pub size_bytes: u64,
    pub orphan: OrphanStatus,
    pub score_band: Band,
    pub original_tier: Tier,
    pub corrected_tier: Tier,
    #[allow(dead_code)]
    pub note: Option<String>,
}

/// Record a user tier correction: append to preferences.jsonl, re-distill
/// rules.json, and audit-log. Returns the updated rule set.
pub async fn apply_correction(
    request: CorrectionRequest,
    memory_dir_override: Option<PathBuf>,
) -> Result<Vec<Rule>> {
    let dir = memory_dir_override.unwrap_or_else(crate::paths::memory_dir);
    let store = rules::MemoryStore::open(&dir)?;

    let features = rules::extract_features(
        Path::new(&request.path),
        request.size_bytes,
        request.orphan,
        request.score_band,
    );
    let pref = rules::Preference {
        timestamp: Utc::now(),
        candidate_features: features,
        original_tier: request.original_tier,
        corrected_tier: request.corrected_tier,
        note: request.note,
    };
    store.append_preference(&pref)?;

    let prefs = store.load_preferences()?;
    let existing = store.load_rules()?;
    let distilled = rules::distill(&prefs, &existing);
    store.save_rules(&distilled)?;

    store.log_session(
        "correction",
        serde_json::json!({
            "path": request.path,
            "original_tier": request.original_tier as u8,
            "corrected_tier": request.corrected_tier as u8,
            "rules_count": distilled.len(),
        }),
    );

    Ok(distilled)
}

/// Re-run distillation only (used by the app's background distiller).
pub async fn redistill(memory_dir_override: Option<PathBuf>) -> Result<Vec<Rule>> {
    let dir = memory_dir_override.unwrap_or_else(crate::paths::memory_dir);
    let store = rules::MemoryStore::open(&dir)?;
    let prefs = store.load_preferences()?;
    let existing = store.load_rules()?;
    let distilled = rules::distill(&prefs, &existing);
    store.save_rules(&distilled)?;
    Ok(distilled)
}

/// Load the current rules snapshot (for ScanConfig.rules).
pub fn load_rules(memory_dir_override: Option<PathBuf>) -> Result<Vec<Rule>> {
    let dir = memory_dir_override.unwrap_or_else(crate::paths::memory_dir);
    let store = rules::MemoryStore::open(&dir)?;
    store.load_rules()
}

/// Rule source helper for the MCP wrapper.
pub fn rule_source_is_user(rule: &Rule) -> bool {
    rule.source == RuleSource::User
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/Users/tester")
    }

    #[test]
    fn cache_roots_are_driftwood() {
        let h = home();
        assert!(is_definitionally_driftwood(
            &h.join("Library/Caches/com.google.Chrome"),
            &h,
            OrphanStatus::Unknown
        ));
        assert!(is_definitionally_driftwood(
            &h.join("Library/Logs/DiagnosticReports"),
            &h,
            OrphanStatus::Unknown
        ));
        assert!(is_definitionally_driftwood(
            Path::new("/tmp/build-cache"),
            &h,
            OrphanStatus::Unknown
        ));
        assert!(is_definitionally_driftwood(
            Path::new("/private/tmp/build-cache"),
            &h,
            OrphanStatus::Unknown
        ));
    }

    #[test]
    fn orphaned_app_support_is_driftwood() {
        let h = home();
        assert!(is_definitionally_driftwood(
            &h.join("Library/Application Support/OldApp"),
            &h,
            OrphanStatus::Orphaned
        ));
        // Active app support folders are NOT auto-high.
        assert!(!is_definitionally_driftwood(
            &h.join("Library/Application Support/ActiveApp"),
            &h,
            OrphanStatus::Active
        ));
    }

    #[test]
    fn personal_folders_are_not_auto_high() {
        let h = home();
        assert!(!is_definitionally_driftwood(
            &h.join("Library/Application Support/ActiveApp"),
            &h,
            OrphanStatus::Unknown
        ));
        assert!(!is_definitionally_driftwood(
            &h.join("Documents/notes"),
            &h,
            OrphanStatus::Unknown
        ));
        // A cache-named folder OUTSIDE the cache roots is not auto-high.
        assert!(!is_definitionally_driftwood(
            &h.join("Library/Containers/com.app/cache"),
            &h,
            OrphanStatus::Unknown
        ));
    }
}
