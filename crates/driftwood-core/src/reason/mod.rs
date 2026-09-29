//! Stage 2 orchestration (plan §3.3): score-proximity batching, per-item
//! fallback on partial failure, cost accounting against the cap, resume
//! persistence, honest `tier_source` labeling.

pub mod client;
pub mod payload;
pub mod prompt;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::Reasoning;
use crate::events::{EventSink, Phase, ScanEvent};
use crate::types::{Candidate, PrivacyTier, Tier};
use crate::{DriftError, Result};

pub use client::LlmJudgment;

/// HTTP transport over async reqwest (rustls). Streaming: chunks are read
/// incrementally so a cancel can abort the in-flight request instead of
/// waiting out the whole body.
pub struct ReqwestTransport {
    inner: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new(timeout: Duration) -> Result<Self> {
        let inner = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| DriftError::Reason(e.to_string()))?;
        Ok(Self { inner })
    }
}

fn apply_headers(req: reqwest::RequestBuilder, headers: &[(String, String)]) -> reqwest::RequestBuilder {
    let mut req = req;
    for (k, v) in headers {
        req = req.header(k, v);
    }
    req
}

impl client::Transport for ReqwestTransport {
    fn post_json<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        body: String,
    ) -> client::BoxFut<'a, Result<(u16, String)>> {
        let fut = apply_headers(self.inner.post(url), headers).body(body);
        Box::pin(async move {
            match fut.send().await {
                Ok(r) => {
                    let status = r.status().as_u16();
                    let text = r.text().await.unwrap_or_default();
                    Ok((status, text))
                }
                Err(e) => Err(DriftError::Reason(e.to_string())),
            }
        })
    }

    fn post_json_stream<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        body: String,
        on_update: &'a (dyn Fn(&client::StreamUpdate) + Send + Sync),
        cancel: &'a AtomicBool,
    ) -> client::BoxFut<'a, Result<client::StreamedResponse>> {
        let fut = apply_headers(self.inner.post(url), headers).body(body);
        Box::pin(async move {
            let mut resp = fut
                .send()
                .await
                .map_err(|e| DriftError::Reason(e.to_string()))?;
            if cancel.load(Ordering::Relaxed) {
                return Err(DriftError::Cancelled);
            }
            let status = resp.status().as_u16();
            on_update(&client::StreamUpdate::Status(status));
            if status != 200 {
                let text = resp.text().await.unwrap_or_default();
                return Err(DriftError::Reason(format!(
                    "OpenRouter HTTP {status}: {}",
                    truncate(&text, 300)
                )));
            }

            let mut acc = client::StreamedResponse::default();
            // Byte buffer: SSE frames can split a UTF-8 char or a `data:` line
            // across chunk boundaries, so only complete lines are converted.
            let mut buffer: Vec<u8> = Vec::new();

            loop {
                if cancel.load(Ordering::Relaxed) {
                    // Returning drops `resp` and with it the in-flight request.
                    return Err(DriftError::Cancelled);
                }
                let chunk = match resp.chunk().await {
                    Ok(Some(c)) => c,
                    Ok(None) => break,
                    Err(e) => return Err(DriftError::Reason(e.to_string())),
                };
                buffer.extend_from_slice(&chunk);
                let mut start = 0usize;
                for pos in 0..buffer.len() {
                    if buffer[pos] != b'\n' {
                        continue;
                    }
                    let mut line = &buffer[start..pos];
                    if line.last() == Some(&b'\r') {
                        line = &line[..line.len() - 1];
                    }
                    start = pos + 1;
                    let text = String::from_utf8_lossy(line);
                    let data = text
                        .strip_prefix("data: ")
                        .or_else(|| text.strip_prefix("data:"))
                        .map(str::trim_start);
                    let Some(data) = data else { continue };
                    match client::parse_sse_data(data)? {
                        None => {}
                        Some(c) => {
                            if let Some(e) = c.error {
                                // A streamed OpenRouter error is a batch
                                // failure with the normal retry semantics.
                                return Err(DriftError::Reason(format!("OpenRouter error: {e}")));
                            }
                            if let Some(d) = c.delta_content {
                                on_update(&client::StreamUpdate::Delta(d.clone()));
                                acc.content.push_str(&d);
                            }
                            if let Some(u) = c.usage {
                                acc.cost_usd = u.cost_usd;
                                acc.prompt_tokens = u.prompt_tokens;
                                acc.completion_tokens = u.completion_tokens;
                                on_update(&client::StreamUpdate::Usage {
                                    cost_usd: u.cost_usd,
                                    prompt_tokens: u.prompt_tokens,
                                    completion_tokens: u.completion_tokens,
                                });
                            }
                        }
                    }
                }
                if start > 0 {
                    buffer.drain(..start);
                }
            }

            if cancel.load(Ordering::Relaxed) {
                return Err(DriftError::Cancelled);
            }
            Ok(acc)
        })
    }
}

/// A persisted batch judgment (resume support): results are written to
/// disk as they arrive so a crashed scan isn't lost (plan §3.3.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedJudgment {
    pub scan_id: String,
    pub judgment: LlmJudgment,
}

/// Outcome of Stage 2 for the engine.
pub struct ReasonOutcome {
    /// id → judgment for candidates the LLM resolved directly.
    pub judgments: HashMap<String, LlmJudgment>,
    /// ids sharing a cluster representative's verdict (see
    /// `cluster_middle_band`). Labeled `TierSource::LlmPropagated`, never
    /// `Llm` — one shared judgment is not N independent ones.
    pub propagated_judgments: HashMap<String, LlmJudgment>,
    /// ids the LLM failed for (fallback tier used; never silent).
    pub fallback_ids: HashSet<String>,
    pub total_cost_usd: f64,
    pub cost_cap_hit: bool,
    /// True when the user pulled the scan ashore mid-crossing; the report
    /// is still returned (partial), distinctly from a cost-cap stop.
    pub cancelled: bool,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// One dedup cluster: the item actually sent to the model, plus the ids
/// that will inherit its verdict.
pub struct Cluster {
    pub representative: Candidate,
    pub siblings: Vec<String>,
}

fn parent_dir_of(path: &str) -> String {
    std::path::Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Cache-likeness identity of a candidate, banded coarsely: near-identical
/// folders cluster together, genuinely different profiles don't.
fn cache_like_bucket(c: &Candidate) -> u8 {
    let ratio = match &c.kind_stats {
        Some(s) => s.cache_like_ratio,
        None => {
            if c.kind == crate::types::Kind::File {
                let ext = std::path::Path::new(&c.path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(crate::scan::walk::is_cache_like_ext)
                    .unwrap_or(false);
                if ext {
                    1.0
                } else {
                    0.0
                }
            } else {
                0.0
            }
        }
    };
    if ratio < 0.25 {
        0
    } else if ratio < 0.75 {
        1
    } else {
        2
    }
}

/// Group the middle band by `(parent_dir, orphan_status, cache-like
/// bucket)` and keep one representative per group (highest score first).
/// A cluster only ever merges items that would get the same answer anyway
/// — e.g. ~120 `~/Library/Logs/DiagnosticReports` siblings — and the
/// propagated verdict is labeled `LlmPropagated`, never `Llm`.
pub fn cluster_middle_band(middle: Vec<Candidate>) -> Vec<Cluster> {
    let mut sorted = middle;
    sorted.sort_by(|a, b| b.score.total_cmp(&a.score));

    let mut index: HashMap<(String, crate::types::OrphanStatus, u8), usize> = HashMap::new();
    let mut clusters: Vec<Cluster> = Vec::new();
    for c in sorted {
        let key = (
            parent_dir_of(&c.path),
            c.orphan_status,
            cache_like_bucket(&c),
        );
        match index.get(&key) {
            Some(&i) => clusters[i].siblings.push(c.id),
            None => {
                index.insert(key, clusters.len());
                clusters.push(Cluster {
                    representative: c,
                    siblings: Vec::new(),
                })
            }
        }
    }
    clusters
}

/// What one concurrent batch task carries and returns.
struct BatchUnit {
    reps: Vec<Candidate>,
    /// representative id → sibling ids that inherit its verdict.
    siblings: HashMap<String, Vec<String>>,
}

/// Heuristic tier for middle-band fallbacks (LLM failure / cost cap).
/// Score-interpolated, always safe-leaning.
pub fn fallback_tier(score: f64) -> Tier {
    if score >= 60.0 {
        Tier::MessageInABottle
    } else {
        Tier::Current
    }
}

/// Run Stage 2 over the middle band.
#[allow(clippy::too_many_arguments)]
pub async fn reason_middle_band(
    middle: Vec<Candidate>,
    privacy: PrivacyTier,
    cfg: &Reasoning,
    model: &str,
    api_key: &str,
    enforce_zdr: bool,
    scan_id: &str,
    few_shot: &[serde_json::Value],
    sink: Arc<dyn EventSink>,
    cancel: Arc<AtomicBool>,
) -> Result<ReasonOutcome> {
    let transport = Arc::new(ReqwestTransport::new(Duration::from_secs(cfg.timeout_secs))?);
    let persist = Some(batch_persist_path(scan_id));
    reason_middle_band_with(
        middle, privacy, cfg, model, api_key, enforce_zdr, scan_id, few_shot, sink, cancel,
        transport, persist,
    )
    .await
}

/// Default persistence location for batch judgments.
pub fn batch_persist_path(scan_id: &str) -> PathBuf {
    crate::paths::reports_dir().join(format!("batches-{scan_id}.jsonl"))
}

/// Testable core, generic over transport; `persist_path` of `None`
/// disables resume persistence (unit tests).
///
/// Batches fan out with bounded concurrency; results are merged into the
/// same judgment/fallback maps. Cancellation aborts in-flight streams and
/// returns the partial outcome instead of an error — every unjudged item
/// lands in `fallback_ids`, so a cancelled scan still yields a report.
#[allow(clippy::too_many_arguments)]
pub async fn reason_middle_band_with<T: client::Transport + 'static>(
    middle: Vec<Candidate>,
    privacy: PrivacyTier,
    cfg: &Reasoning,
    model: &str,
    api_key: &str,
    enforce_zdr: bool,
    scan_id: &str,
    few_shot: &[serde_json::Value],
    sink: Arc<dyn EventSink>,
    cancel: Arc<AtomicBool>,
    transport: Arc<T>,
    persist_path: Option<PathBuf>,
) -> Result<ReasonOutcome> {
    sink.emit(ScanEvent::Phase {
        phase: Phase::Reasoning,
    });

    if !enforce_zdr {
        sink.emit(ScanEvent::Warn {
            message: "ZDR-only routing is off — judgments may be processed by providers that retain prompts".into(),
        });
    }

    let mut judgments: HashMap<String, LlmJudgment> = HashMap::new();
    let mut propagated: HashMap<String, LlmJudgment> = HashMap::new();
    let mut fallback_ids = HashSet::new();
    let mut total_cost = 0.0f64;
    let mut total_prompt_tokens = 0u64;
    let mut total_completion_tokens = 0u64;
    let mut cap_hit = false;
    let mut cancelled = false;

    // Resume: load previously persisted judgments for this scan id. The
    // loader tolerates a truncated trailing line (a cancelled stream must
    // not poison the resume file) — malformed lines are simply skipped.
    if let Some(path) = &persist_path {
        if let Ok(text) = tokio::fs::read_to_string(path).await {
            let mut resumed = 0;
            for line in text.lines() {
                if let Ok(p) = serde_json::from_str::<PersistedJudgment>(line) {
                    if p.scan_id == scan_id && judgments.insert(p.judgment.id.clone(), p.judgment).is_none() {
                        resumed += 1;
                    }
                }
            }
            if resumed > 0 {
                sink.emit(ScanEvent::notice(format!(
                    "resumed {resumed} judgments from a previous run of this scan"
                )));
            }
        }
    }

    // Cluster dedup, then score-proximity batching over the representatives.
    let clusters = cluster_middle_band(middle);
    let total_items = clusters
        .iter()
        .map(|c| 1 + c.siblings.len() as u64)
        .sum::<u64>();
    let rep_count: u64 = clusters.len() as u64;
    let saved = total_items.saturating_sub(rep_count);
    if saved > 0 {
        sink.emit(ScanEvent::notice(format!(
            "{rep_count} representative items stand in for {total_items} — their siblings share each verdict"
        )));
    }

    let batch_size = cfg.batch_size.clamp(1, 100);
    let mut units: Vec<BatchUnit> = Vec::new();
    for chunk in clusters.chunks(batch_size) {
        let mut reps = Vec::with_capacity(chunk.len());
        let mut siblings = HashMap::new();
        for c in chunk {
            siblings.insert(c.representative.id.clone(), c.siblings.clone());
            reps.push(c.representative.clone());
        }
        units.push(BatchUnit { reps, siblings });
    }
    let total_batches = units.len();
    let max_inflight = cfg.max_concurrent_batches.clamp(1, 8);
    let few_shot: Arc<Vec<serde_json::Value>> = Arc::new(few_shot.to_vec());

    let mut judged_items: u64 = 0;
    let mut next_batch = 0usize;
    let mut inflight = 0usize;
    let mut settled = 0usize; // batches finished (success or failure)
    let mut successful = 0usize; // batches that returned a real cost
    let mut join = tokio::task::JoinSet::new();

    loop {
        // Honest cancel: stop dispatching, drain what's in flight, return
        // the partial result. The in-flight streams abort themselves.
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
        }

        // Projected-cost cap, checked before each dispatch: a single
        // expensive batch may not silently overshoot the ceiling. The
        // projection uses the mean cost of the batches that settled so
        // far; the first batch is always allowed (no estimate exists).
        if !cancelled && !cap_hit && cfg.cost_cap_usd > 0.0 {
            let remaining = total_batches - next_batch;
            let est_per_batch = if successful > 0 {
                total_cost / successful as f64
            } else {
                0.0
            };
            let projected = total_cost + est_per_batch * remaining as f64;
            if total_cost >= cfg.cost_cap_usd || projected >= cfg.cost_cap_usd {
                cap_hit = true;
                sink.emit(ScanEvent::Warn {
                    message: "Snagged — ran out of river: the projected spend would pass the per-scan cost cap; remaining items use heuristic tiers".into(),
                });
            }
        }

        let can_dispatch =
            !cancelled && !cap_hit && next_batch < total_batches && inflight < max_inflight;
        if can_dispatch {
            let unit = std::mem::replace(&mut units[next_batch], BatchUnit {
                reps: Vec::new(),
                siblings: HashMap::new(),
            });
            sink.emit(ScanEvent::BatchStarted {
                index: (next_batch + 1) as u64,
                total_batches: total_batches as u64,
            });
            let batch_no = next_batch + 1;
            join.spawn(run_one_batch(
                unit,
                privacy,
                model.to_string(),
                api_key.to_string(),
                enforce_zdr,
                cfg.clone(),
                few_shot.clone(),
                sink.clone(),
                transport.clone(),
                persist_path.clone(),
                cancel.clone(),
                batch_no,
                total_batches,
            ));
            next_batch += 1;
            inflight += 1;
            continue;
        }

        if inflight > 0 {
            let (batch_no, unit, result) = match join.join_next().await {
                Some(Ok(done)) => done,
                Some(Err(join_err)) => {
                    // A batch task panicked — treat like any failed batch.
                    inflight -= 1;
                    settled += 1;
                    sink.emit(ScanEvent::Warn {
                        message: format!("batch snagged: task failed: {join_err}"),
                    });
                    continue;
                }
                None => break,
            };
            inflight -= 1;
            settled += 1;

            match result {
                Ok((cost, ptok, ctok, accepted)) => {
                    total_cost += cost;
                    total_prompt_tokens += ptok;
                    total_completion_tokens += ctok;
                    successful += 1;
                    let wanted: HashSet<&str> = unit.reps.iter().map(|c| c.id.as_str()).collect();
                    for j in accepted {
                        if wanted.contains(j.id.as_str()) {
                            judgments.insert(j.id.clone(), j);
                        }
                    }
                    // Merge: representative → its own judgment; siblings →
                    // propagated copy (honest labeling happens in the engine).
                    for rep in &unit.reps {
                        if let Some(j) = judgments.get(&rep.id) {
                            judged_items += 1;
                            for sib in unit.siblings.get(&rep.id).into_iter().flatten() {
                                propagated.insert(sib.clone(), j.clone());
                                judged_items += 1;
                            }
                        } else {
                            fallback_ids.insert(rep.id.clone());
                        }
                    }
                }
                Err(DriftError::Cancelled) => {
                    // The user pulled this batch ashore mid-stream; its
                    // candidates land in fallback like any other unanswered
                    // item, and no "snagged" warning is issued for it.
                    for rep in &unit.reps {
                        fallback_ids.insert(rep.id.clone());
                    }
                }
                Err(e) => {
                    sink.emit(ScanEvent::Warn {
                        message: format!("batch snagged: {e}"),
                    });
                    for rep in &unit.reps {
                        fallback_ids.insert(rep.id.clone());
                    }
                }
            }

            sink.emit(ScanEvent::BatchFinished {
                index: batch_no as u64,
                total_batches: total_batches as u64,
            });
            sink.emit(ScanEvent::ReasoningProgress {
                judged: judged_items,
                total: total_items,
                cost_usd: total_cost,
                prompt_tokens: total_prompt_tokens,
                completion_tokens: total_completion_tokens,
            });
            sink.emit(ScanEvent::notice(format!(
                "technical: {settled}/{total_batches} batches settled · {} prompt + {} completion tokens · {} so far",
                total_prompt_tokens, total_completion_tokens,
                format_cost(total_cost)
            )));
            continue;
        }

        break;
    }

    if cancelled {
        sink.emit(ScanEvent::Warn {
            message: "Pulled ashore — the crossing stopped early; unjudged items use heuristic tiers".into(),
        });
    }

    // Safety net: any middle-band item without a direct or propagated
    // verdict → fallback. Nothing silently vanishes.
    for cluster in &clusters {
        let mut ids = Vec::with_capacity(1 + cluster.siblings.len());
        ids.push(cluster.representative.id.clone());
        ids.extend(cluster.siblings.iter().cloned());
        for id in ids {
            if !judgments.contains_key(&id) && !propagated.contains_key(&id) {
                fallback_ids.insert(id);
            }
        }
    }

    Ok(ReasonOutcome {
        judgments,
        propagated_judgments: propagated,
        fallback_ids,
        total_cost_usd: total_cost,
        cost_cap_hit: cap_hit,
        cancelled,
        prompt_tokens: total_prompt_tokens,
        completion_tokens: total_completion_tokens,
    })
}

/// One concurrent batch task: build prompt → streamed POST (retries +
/// backoff inside the task) → tolerant parse (with one truncated-JSON
/// salvage attempt) → persist. Returns (cost, prompt_tokens,
/// completion_tokens, parsed judgments).
#[allow(clippy::too_many_arguments)]
async fn run_one_batch<T: client::Transport>(
    unit: BatchUnit,
    privacy: PrivacyTier,
    model: String,
    api_key: String,
    enforce_zdr: bool,
    cfg: Reasoning,
    few_shot: Arc<Vec<serde_json::Value>>,
    sink: Arc<dyn EventSink>,
    transport: Arc<T>,
    persist_path: Option<PathBuf>,
    cancel: Arc<AtomicBool>,
    batch_no: usize,
    total_batches: usize,
) -> (usize, BatchUnit, Result<(f64, u64, u64, Vec<LlmJudgment>)>) {
    use crate::reason::client as oc;
    use crate::reason::payload as pl;
    use crate::reason::prompt as pr;

    let batch = &unit.reps;
    let deep_cap = crate::config::WalkCaps::default().deep_listing_cap;
    let mut deep_listings = HashMap::new();
    if privacy == PrivacyTier::Deep {
        for c in batch {
            if c.kind != crate::types::Kind::File {
                if let Some(listing) =
                    pl::folder_listing(std::path::Path::new(&c.path), deep_cap)
                {
                    deep_listings.insert(c.id.clone(), listing);
                }
            }
        }
    }

    let payloads = pl::build_batch_payloads(batch, privacy, &deep_listings, deep_cap);
    let user = pr::build_user_prompt(&payloads, few_shot.as_slice());
    let body = serde_json::to_string(&oc::build_request_body(
        &model, pr::SYSTEM_PROMPT, &user, enforce_zdr,
    ))
    .map_err(|e| DriftError::Reason(e.to_string()));
    let body = match body {
        Ok(b) => b,
        Err(e) => return (batch_no, unit, Err(e)),
    };
    let headers = oc::auth_headers(&api_key);

    sink.emit(ScanEvent::notice(format!(
        "traveling to the river: batch {batch_no}/{total_batches} · {} representative candidates",
        batch.len()
    )));

    let mut last_err: Option<DriftError> = None;
    for attempt in 0..=cfg.retries {
        if cancel.load(Ordering::Relaxed) {
            return (batch_no, unit, Err(DriftError::Cancelled));
        }
        if attempt > 0 {
            let backoff = Duration::from_millis(500 * 2u64.pow(attempt - 1));
            tokio::time::sleep(backoff).await;
            sink.emit(ScanEvent::notice(format!(
                "retrying batch {batch_no}/{total_batches} (attempt {})",
                attempt + 1
            )));
        }

        // No-op progress tap: the orchestrator merges results per batch;
        // deltas are already reflected live through the transport stream.
        let tap = |_u: &oc::StreamUpdate| {};
        let stream = transport
            .post_json_stream(oc::OPENROUTER_URL, &headers, body.clone(), &tap, &cancel)
            .await;
        match stream {
            Err(DriftError::Cancelled) => return (batch_no, unit, Err(DriftError::Cancelled)),
            Err(e) => {
                last_err = Some(e);
                continue;
            }
            Ok(resp) => {
                // Tolerant parse, then one salvage attempt for a stream cut
                // mid-JSON. Anything still unanswered falls back below —
                // never silently dropped.
                let parsed = match oc::parse_judgments(&resp.content) {
                    Ok(j) => Ok(j),
                    Err(e) => match oc::salvage_truncated_json(&resp.content) {
                        Some(s) => {
                            sink.emit(ScanEvent::notice(
                                "salvaged a truncated model response — partial batch results kept",
                            ));
                            oc::parse_judgments(&s).map_err(|_| e)
                        }
                        None => Err(e),
                    },
                };
                match parsed {
                    Ok(judgments) => {
                        persist_batch(scan_id_from(persist_path.as_deref()), &judgments, persist_path.as_deref());
                        return (
                            batch_no,
                            unit,
                            Ok((resp.cost_usd, resp.prompt_tokens, resp.completion_tokens, judgments)),
                        );
                    }
                    Err(e) => last_err = Some(e), // whole-batch parse failure → retry
                }
            }
        }
    }

    (
        batch_no,
        unit,
        Err(last_err.unwrap_or_else(|| DriftError::Reason("batch failed".into()))),
    )
}

fn scan_id_from(path: Option<&std::path::Path>) -> Option<String> {
    path.and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("batches-"))
        .and_then(|n| n.strip_suffix(".jsonl"))
        .map(String::from)
}

fn persist_batch(scan_id: Option<String>, judgments: &[LlmJudgment], path: Option<&std::path::Path>) {
    let (Some(scan_id), Some(path)) = (scan_id, path) else {
        return;
    };
    use std::io::Write;
    if std::fs::create_dir_all(path.parent().unwrap_or_else(|| std::path::Path::new("."))).is_err() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        for j in judgments {
            let record = PersistedJudgment {
                scan_id: scan_id.clone(),
                judgment: j.clone(),
            };
            if let Ok(line) = serde_json::to_string(&record) {
                let _ = writeln!(f, "{line}");
            }
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

/// Human-readable cost (for report notices).
pub fn format_cost(usd: f64) -> String {
    format!("${usd:.4}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Band, Kind, OrphanStatus, ScopeCategory, ScoreComponents};

    /// Distinct parent per dir so batching tests get one cluster per item;
    /// plain `candidate` shares `/tmp` so dedup tests get one cluster.
    fn candidate_at(dir: &str, id: &str, score: f64) -> Candidate {
        Candidate {
            id: id.into(),
            path: format!("{dir}/{id}"),
            kind: Kind::Folder,
            size_bytes: 1000,
            kind_stats: None,
            last_used_date: None,
            last_used_from_spotlight: false,
            modified_date: None,
            created_date: None,
            orphan_status: OrphanStatus::Unknown,
            scope_category: ScopeCategory::Low,
            score,
            score_components: ScoreComponents::default(),
            band: Band::Middle,
        }
    }

    fn candidate(id: &str, score: f64) -> Candidate {
        candidate_at("/tmp", id, score)
    }

    fn judgment_for(id: &str, tier: u8) -> String {
        format!(r#"{{"id":"{id}","tier":{tier},"confidence":0.9,"summary":"s","reasoning":"r"}}"#)
    }

    struct OkTransport {
        content: String,
        cost: f64,
    }
    impl client::Transport for OkTransport {
        fn post_json<'a>(
            &'a self,
            _: &'a str,
            _: &'a [(String, String)],
            _: String,
        ) -> client::BoxFut<'a, Result<(u16, String)>> {
            let body = serde_json::json!({
                "choices": [{"message": {"content": self.content}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 5, "cost": self.cost}
            });
            Box::pin(async move { Ok((200, body.to_string())) })
        }
        fn post_json_stream<'a>(
            &'a self,
            _: &'a str,
            _: &'a [(String, String)],
            _: String,
            on_update: &'a (dyn Fn(&client::StreamUpdate) + Send + Sync),
            _cancel: &'a AtomicBool,
        ) -> client::BoxFut<'a, Result<client::StreamedResponse>> {
            let content = self.content.clone();
            let cost = self.cost;
            Box::pin(async move {
                on_update(&client::StreamUpdate::Delta(content.clone()));
                on_update(&client::StreamUpdate::Usage {
                    cost_usd: cost,
                    prompt_tokens: 10,
                    completion_tokens: 5,
                });
                Ok(client::StreamedResponse {
                    content,
                    cost_usd: cost,
                    prompt_tokens: 10,
                    completion_tokens: 5,
                })
            })
        }
    }

    struct FailTransport;
    impl client::Transport for FailTransport {
        fn post_json<'a>(
            &'a self,
            _: &'a str,
            _: &'a [(String, String)],
            _: String,
        ) -> client::BoxFut<'a, Result<(u16, String)>> {
            Box::pin(async { Err(DriftError::Reason("network gone".into())) })
        }
        fn post_json_stream<'a>(
            &'a self,
            _: &'a str,
            _: &'a [(String, String)],
            _: String,
            _: &'a (dyn Fn(&client::StreamUpdate) + Send + Sync),
            _: &'a AtomicBool,
        ) -> client::BoxFut<'a, Result<client::StreamedResponse>> {
            Box::pin(async { Err(DriftError::Reason("network gone".into())) })
        }
    }

    /// Streams one partial delta, then hangs until cancelled — models the
    /// in-flight request a user aborts mid-crossing.
    struct HangTransport;
    impl client::Transport for HangTransport {
        fn post_json<'a>(
            &'a self,
            _: &'a str,
            _: &'a [(String, String)],
            _: String,
        ) -> client::BoxFut<'a, Result<(u16, String)>> {
            Box::pin(async { Err(DriftError::Reason("network gone".into())) })
        }
        fn post_json_stream<'a>(
            &'a self,
            _: &'a str,
            _: &'a [(String, String)],
            _: String,
            on_update: &'a (dyn Fn(&client::StreamUpdate) + Send + Sync),
            cancel: &'a AtomicBool,
        ) -> client::BoxFut<'a, Result<client::StreamedResponse>> {
            Box::pin(async move {
                on_update(&client::StreamUpdate::Delta("{\"id\":\"a\",\"ti".into()));
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(DriftError::Cancelled);
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
        }
    }

    fn sink() -> Arc<crate::events::CollectingSink> {
        crate::events::CollectingSink::new()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn happy_path_assigns_judgments() {
        let content = format!("{},{}", judgment_for("a", 1), judgment_for("b", 4));
        let transport = Arc::new(OkTransport {
            content: format!("[{content}]"),
            cost: 0.01,
        });
        let sink = sink();
        let out = reason_middle_band_with(
            vec![candidate_at("/w1", "a", 50.0), candidate_at("/w2", "b", 45.0)],
            PrivacyTier::Minimal,
            &Reasoning::default(),
            "test-model",
            "key",
            true,
            "test-happy",
            &[],
            sink.clone(),
            Arc::new(AtomicBool::new(false)),
            transport,
            None,
        )
        .await
        .unwrap();
        assert_eq!(out.judgments.len(), 2);
        assert!(out.propagated_judgments.is_empty());
        assert!(out.fallback_ids.is_empty());
        assert!((out.total_cost_usd - 0.01).abs() < 1e-9);
        assert!(!out.cancelled);
        // The UI's knowable-denominator signals were emitted.
        let events = sink.snapshot();
        assert!(events
            .iter()
            .any(|e| matches!(e, ScanEvent::BatchStarted { index: 1, total_batches: 1 })));
        assert!(events
            .iter()
            .any(|e| matches!(e, ScanEvent::BatchFinished { index: 1, total_batches: 1 })));
        assert!(events.iter().any(
            |e| matches!(e, ScanEvent::ReasoningProgress { judged: 2, total: 2, .. })
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_batch_degrades_to_fallback() {
        let transport = Arc::new(FailTransport);
        let cfg = Reasoning {
            retries: 0,
            timeout_secs: 5,
            ..Reasoning::default()
        };
        let out = reason_middle_band_with(
            vec![candidate_at("/w1", "a", 70.0), candidate_at("/w2", "b", 30.0)],
            PrivacyTier::Minimal,
            &cfg,
            "m",
            "k",
            true,
            "test-fail",
            &[],
            sink(),
            Arc::new(AtomicBool::new(false)),
            transport,
            None,
        )
        .await
        .unwrap();
        assert!(out.judgments.is_empty());
        assert!(out.fallback_ids.contains("a"));
        assert!(out.fallback_ids.contains("b"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cost_cap_stops_batches() {
        let content = format!("[{}]", judgment_for("a", 1));
        let transport = Arc::new(OkTransport {
            content,
            cost: 0.60, // exceeds the default 0.50 cap after one batch
        });
        let cfg = Reasoning {
            batch_size: 1, // force a second batch so the projected cap fires
            ..Reasoning::default()
        };
        let out = reason_middle_band_with(
            vec![candidate_at("/w1", "a", 50.0), candidate_at("/w2", "b", 49.0)],
            PrivacyTier::Minimal,
            &cfg,
            "m",
            "k",
            true,
            "test-cap",
            &[],
            sink(),
            Arc::new(AtomicBool::new(false)),
            transport,
            None,
        )
        .await
        .unwrap();
        assert!(out.cost_cap_hit);
        assert!(out.judgments.contains_key("a"));
        assert!(!out.cancelled, "a cap stop is not a cancel");
        assert!(out.fallback_ids.contains("b"), "b never got a batch");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn projected_cost_cap_stops_before_dispatch() {
        let transport = Arc::new(OkTransport {
            content: format!("[{}]", judgment_for("a", 1)),
            cost: 0.10,
        });
        let cfg = Reasoning {
            batch_size: 1,
            // Serial dispatch so the projection sees settled costs between
            // batches: after batch 1 the projected total is 0.10 + 0.10*2 =
            // 0.30 >= 0.25 — the cap trips on the projection, not on the
            // actual spend (which never passes 0.10).
            max_concurrent_batches: 1,
            cost_cap_usd: 0.25,
            ..Reasoning::default()
        };
        let out = reason_middle_band_with(
            vec![
                candidate_at("/w1", "a", 50.0),
                candidate_at("/w2", "b", 49.0),
                candidate_at("/w3", "c", 48.0),
            ],
            PrivacyTier::Minimal,
            &cfg,
            "m",
            "k",
            true,
            "test-projected-cap",
            &[],
            sink(),
            Arc::new(AtomicBool::new(false)),
            transport,
            None,
        )
        .await
        .unwrap();
        assert!(out.cost_cap_hit);
        assert!(out.judgments.contains_key("a"));
        assert!(!out.judgments.contains_key("b"), "b was never dispatched");
        assert!((out.total_cost_usd - 0.10).abs() < 1e-9, "spend never breached the cap");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn partial_ids_fall_back() {
        // Model answered only "a"; "b" must fall back, not vanish.
        let content = format!("[{}]", judgment_for("a", 2));
        let transport = Arc::new(OkTransport {
            content,
            cost: 0.0,
        });
        let out = reason_middle_band_with(
            vec![candidate_at("/w1", "a", 50.0), candidate_at("/w2", "b", 45.0)],
            PrivacyTier::Minimal,
            &Reasoning::default(),
            "m",
            "k",
            true,
            "test-partial",
            &[],
            sink(),
            Arc::new(AtomicBool::new(false)),
            transport,
            None,
        )
        .await
        .unwrap();
        assert!(out.judgments.contains_key("a"));
        assert!(out.fallback_ids.contains("b"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cluster_dedup_propagates_with_honest_source() {
        // Three siblings under /tmp — one representative travels; the
        // verdict is propagated, never re-asked.
        let content = format!("[{}]", judgment_for("a", 1));
        let transport = Arc::new(OkTransport {
            content,
            cost: 0.01,
        });
        let out = reason_middle_band_with(
            vec![
                candidate("a", 60.0),
                candidate("b", 50.0),
                candidate("c", 40.0),
            ],
            PrivacyTier::Minimal,
            &Reasoning::default(),
            "m",
            "k",
            true,
            "test-dedup",
            &[],
            sink(),
            Arc::new(AtomicBool::new(false)),
            transport,
            None,
        )
        .await
        .unwrap();
        assert_eq!(out.judgments.len(), 1, "only the representative was sent");
        assert!(out.judgments.contains_key("a"));
        assert!(out.propagated_judgments.contains_key("b"));
        assert!(out.propagated_judgments.contains_key("c"));
        assert!(out.fallback_ids.is_empty());
        assert!((out.total_cost_usd - 0.01).abs() < 1e-9, "one call, not three");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn truncated_stream_is_salvaged_not_dropped() {
        // A stream cut mid-JSON still yields item "a"; the unrecovered
        // representative falls back rather than vanishing.
        let content = format!(
            "[{},{{\"id\":\"b\",\"ti",
            judgment_for("a", 1)
        );
        let transport = Arc::new(OkTransport {
            content,
            cost: 0.01,
        });
        let out = reason_middle_band_with(
            vec![candidate_at("/w1", "a", 50.0), candidate_at("/w2", "b", 45.0)],
            PrivacyTier::Minimal,
            &Reasoning::default(),
            "m",
            "k",
            true,
            "test-salvage",
            &[],
            sink(),
            Arc::new(AtomicBool::new(false)),
            transport,
            None,
        )
        .await
        .unwrap();
        assert!(out.judgments.contains_key("a"));
        assert!(out.fallback_ids.contains("b"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancellation_mid_stream_returns_partial_outcome() {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(60)).await;
            cancel2.store(true, Ordering::Relaxed);
        });
        let out = reason_middle_band_with(
            vec![candidate("a", 50.0)],
            PrivacyTier::Minimal,
            &Reasoning::default(),
            "m",
            "k",
            true,
            "test-cancel-mid-stream",
            &[],
            sink(),
            cancel,
            Arc::new(HangTransport),
            None,
        )
        .await
        .unwrap();
        assert!(out.cancelled);
        assert!(!out.cost_cap_hit, "a cancel is not a cap hit");
        assert!(out.judgments.is_empty());
        assert!(out.fallback_ids.contains("a"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancellation_before_any_batch_returns_partial_outcome() {
        let transport = Arc::new(FailTransport);
        let cancel = Arc::new(AtomicBool::new(true));
        let out = reason_middle_band_with(
            vec![candidate("a", 50.0)],
            PrivacyTier::Minimal,
            &Reasoning::default(),
            "m",
            "k",
            true,
            "test-cancel",
            &[],
            sink(),
            cancel,
            transport,
            None,
        )
        .await
        .unwrap();
        // Honest cancel: a partial outcome, not a discarded scan.
        assert!(out.cancelled);
        assert!(!out.cost_cap_hit);
        assert!(out.judgments.is_empty());
        assert!(out.fallback_ids.contains("a"));
    }

    #[test]
    fn fallback_tier_mapping() {
        assert_eq!(fallback_tier(70.0), Tier::MessageInABottle);
        assert_eq!(fallback_tier(50.0), Tier::Current);
    }

    #[test]
    fn clustering_groups_by_parent_orphan_and_cache_profile() {
        let same = vec![
            candidate("a", 60.0),
            candidate("b", 50.0),
            candidate_at("/tmp", "c", 40.0),
        ];
        let clusters = cluster_middle_band(same);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].representative.id, "a");
        assert_eq!(clusters[0].siblings, vec!["b".to_string(), "c".to_string()]);

        let distinct = vec![candidate_at("/w1", "a", 60.0), candidate_at("/w2", "b", 50.0)];
        let clusters = cluster_middle_band(distinct);
        assert_eq!(clusters.len(), 2);
    }
}
