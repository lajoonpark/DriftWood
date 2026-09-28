//! Stage 2 orchestration (plan §3.3): score-proximity batching, per-item
//! fallback on partial failure, cost accounting against the cap, resume
//! persistence, honest `tier_source` labeling.

pub mod client;
pub mod payload;
pub mod prompt;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::Reasoning;
use crate::events::{EventSink, Phase, ScanEvent};
use crate::types::{Candidate, PrivacyTier, Tier};
use crate::{DriftError, Result};

pub use client::LlmJudgment;

/// HTTP transport over reqwest (rustls).
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

impl client::Transport for ReqwestTransport {
    fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: String,
    ) -> Result<(u16, String)> {
        let client = self.inner.clone();
        let url = url.to_string();
        let headers: Vec<(String, String)> = headers.to_vec();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let mut req = client.post(&url);
                for (k, v) in &headers {
                    req = req.header(k, v);
                }
                match req.body(body).send().await {
                    Ok(r) => {
                        let status = r.status().as_u16();
                        let text = r.text().await.unwrap_or_default();
                        Ok((status, text))
                    }
                    Err(e) => Err(DriftError::Reason(e.to_string())),
                }
            })
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
    /// id → judgment for candidates the LLM resolved.
    pub judgments: HashMap<String, LlmJudgment>,
    /// ids the LLM failed for (fallback tier used; never silent).
    pub fallback_ids: HashSet<String>,
    pub total_cost_usd: f64,
    pub cost_cap_hit: bool,
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
    sink: &dyn EventSink,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<ReasonOutcome> {
    let transport = ReqwestTransport::new(Duration::from_secs(cfg.timeout_secs))?;
    let persist = Some(batch_persist_path(scan_id));
    reason_middle_band_with(
        middle, privacy, cfg, model, api_key, enforce_zdr, scan_id, few_shot, sink, cancel,
        &transport, persist,
    )
    .await
}

/// Default persistence location for batch judgments.
pub fn batch_persist_path(scan_id: &str) -> PathBuf {
    crate::paths::reports_dir().join(format!("batches-{scan_id}.jsonl"))
}

/// Testable core, generic over transport; `persist_path` of `None`
/// disables resume persistence (unit tests).
#[allow(clippy::too_many_arguments)]
pub async fn reason_middle_band_with<T: client::Transport>(
    middle: Vec<Candidate>,
    privacy: PrivacyTier,
    cfg: &Reasoning,
    model: &str,
    api_key: &str,
    enforce_zdr: bool,
    scan_id: &str,
    few_shot: &[serde_json::Value],
    sink: &dyn EventSink,
    cancel: &std::sync::atomic::AtomicBool,
    transport: &T,
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
    let mut fallback_ids = HashSet::new();
    let mut total_cost = 0.0f64;
    let mut cap_hit = false;

    // Resume: load previously persisted judgments for this scan id.
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

    // Score-proximity batching: sort by score, chunk contiguous.
    let mut sorted = middle;
    sorted.sort_by(|a, b| b.score.total_cmp(&a.score));
    let batch_size = cfg.batch_size.clamp(1, 100);

    for chunk_start in (0..sorted.len()).step_by(batch_size) {
        if cancel.load(Ordering::Relaxed) {
            return Err(DriftError::Cancelled);
        }

        if cfg.cost_cap_usd > 0.0 && total_cost >= cfg.cost_cap_usd {
            cap_hit = true;
            sink.emit(ScanEvent::Warn {
                message: "Snagged — ran out of river: per-scan cost cap reached; remaining items use heuristic tiers".into(),
            });
            break;
        }

        let batch: Vec<Candidate> =
            sorted[chunk_start..(chunk_start + batch_size).min(sorted.len())].to_vec();
        let pending: Vec<Candidate> = batch
            .iter()
            .filter(|c| !judgments.contains_key(&c.id) && !fallback_ids.contains(&c.id))
            .cloned()
            .collect();
        if pending.is_empty() {
            continue;
        }

        match run_one_batch(&pending, privacy, model, api_key, enforce_zdr, cfg, few_shot, sink, transport, persist_path.as_deref()).await {
            Ok((cost, accepted)) => {
                total_cost += cost;
                let wanted: HashSet<&str> = pending.iter().map(|c| c.id.as_str()).collect();
                for j in accepted {
                    if wanted.contains(j.id.as_str()) {
                        judgments.insert(j.id.clone(), j);
                    }
                }
                // Any pending candidate the model never answered → fallback.
                for c in &pending {
                    if !judgments.contains_key(&c.id) {
                        fallback_ids.insert(c.id.clone());
                    }
                }
            }
            Err(e) => {
                sink.emit(ScanEvent::Warn {
                    message: format!("batch snagged: {e}"),
                });
                for c in &pending {
                    fallback_ids.insert(c.id.clone());
                }
            }
        }
    }

    // Safety net: any middle-band candidate without a judgment → fallback.
    for c in &sorted {
        if !judgments.contains_key(&c.id) {
            fallback_ids.insert(c.id.clone());
        }
    }

    Ok(ReasonOutcome {
        judgments,
        fallback_ids,
        total_cost_usd: total_cost,
        cost_cap_hit: cap_hit,
    })
}

/// One batch: build prompt → POST (retries + backoff) → parse → persist.
/// Returns (call cost, parsed judgments).
#[allow(clippy::too_many_arguments)]
async fn run_one_batch<T: client::Transport>(
    batch: &[Candidate],
    privacy: PrivacyTier,
    model: &str,
    api_key: &str,
    enforce_zdr: bool,
    cfg: &Reasoning,
    few_shot: &[serde_json::Value],
    sink: &dyn EventSink,
    transport: &T,
    persist_path: Option<&std::path::Path>,
) -> Result<(f64, Vec<LlmJudgment>)> {
    use crate::reason::client as oc;
    use crate::reason::payload as pl;
    use crate::reason::prompt as pr;

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
    let user = pr::build_user_prompt(&payloads, few_shot);
    let body = serde_json::to_string(&oc::build_request_body(
        model, pr::SYSTEM_PROMPT, &user, enforce_zdr,
    ))
    .map_err(|e| DriftError::Reason(e.to_string()))?;
    let headers = oc::auth_headers(api_key);

    sink.emit(ScanEvent::notice(format!(
        "traveling to the river: batch of {} candidates",
        batch.len()
    )));

    let mut last_err: Option<DriftError> = None;
    for attempt in 0..=cfg.retries {
        if attempt > 0 {
            let backoff = Duration::from_millis(500 * 2u64.pow(attempt - 1));
            tokio::time::sleep(backoff).await;
            sink.emit(ScanEvent::notice(format!(
                "retrying batch (attempt {})",
                attempt + 1
            )));
        }

        let (status, text) = transport.post_json(oc::OPENROUTER_URL, &headers, body.clone())?;
        if status == 200 {
            let resp = oc::parse_chat_response(&text)?;
            match oc::parse_judgments(&resp.content) {
                Ok(judgments) => {
                    persist_batch(scan_id_from(persist_path), &judgments, persist_path);
                    return Ok((resp.cost_usd, judgments));
                }
                Err(e) => last_err = Some(e), // whole-batch parse failure → retry
            }
        } else {
            last_err = Some(DriftError::Reason(format!(
                "OpenRouter HTTP {status}: {}",
                truncate(&text, 300)
            )));
            // 4xx (except 429) won't fix itself; skip retries.
            if (400..500).contains(&status) && status != 429 {
                break;
            }
        }
    }

    Err(last_err.unwrap_or_else(|| DriftError::Reason("batch failed".into())))
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
    use std::sync::Arc;

    fn candidate(id: &str, score: f64) -> Candidate {
        Candidate {
            id: id.into(),
            path: format!("/tmp/{id}"),
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

    struct OkTransport {
        content: String,
        cost: f64,
    }
    impl client::Transport for OkTransport {
        fn post_json(&self, _: &str, _: &[(String, String)], _: String) -> Result<(u16, String)> {
            let body = serde_json::json!({
                "choices": [{"message": {"content": self.content}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 5, "cost": self.cost}
            });
            Ok((200, body.to_string()))
        }
    }

    struct FailTransport;
    impl client::Transport for FailTransport {
        fn post_json(&self, _: &str, _: &[(String, String)], _: String) -> Result<(u16, String)> {
            Err(DriftError::Reason("network gone".into()))
        }
    }

    fn sink() -> Arc<crate::events::CollectingSink> {
        crate::events::CollectingSink::new()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn happy_path_assigns_judgments() {
        let content = r#"[{"id":"a","tier":1,"confidence":0.9,"summary":"s","reasoning":"r"},{"id":"b","tier":4,"confidence":0.8,"summary":"s","reasoning":"r"}]"#;
        let transport = OkTransport {
            content: content.into(),
            cost: 0.01,
        };
        let out = reason_middle_band_with(
            vec![candidate("a", 50.0), candidate("b", 45.0)],
            PrivacyTier::Minimal,
            &Reasoning::default(),
            "test-model",
            "key",
            true,
            "test-happy",
            &[],
            sink().as_ref(),
            &std::sync::atomic::AtomicBool::new(false),
            &transport,
            None,
        )
        .await
        .unwrap();
        assert_eq!(out.judgments.len(), 2);
        assert!(out.fallback_ids.is_empty());
        assert!((out.total_cost_usd - 0.01).abs() < 1e-9);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_batch_degrades_to_fallback() {
        let transport = FailTransport;
        let cfg = Reasoning {
            retries: 0,
            timeout_secs: 5,
            ..Reasoning::default()
        };
        let out = reason_middle_band_with(
            vec![candidate("a", 70.0), candidate("b", 30.0)],
            PrivacyTier::Minimal,
            &cfg,
            "m",
            "k",
            true,
            "test-fail",
            &[],
            sink().as_ref(),
            &std::sync::atomic::AtomicBool::new(false),
            &transport,
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
        let content = r#"[{"id":"a","tier":1,"confidence":0.9,"summary":"s","reasoning":"r"}]"#;
        let transport = OkTransport {
            content: content.into(),
            cost: 0.60, // exceeds the default 0.50 cap after one batch
        };
        let middle = vec![candidate("a", 50.0), candidate("b", 49.0)];
        let cfg = Reasoning {
            batch_size: 1, // force a second batch so the cap check fires
            ..Reasoning::default()
        };
        let out = reason_middle_band_with(
            middle,
            PrivacyTier::Minimal,
            &cfg,
            "m",
            "k",
            true,
            "test-cap",
            &[],
            sink().as_ref(),
            &std::sync::atomic::AtomicBool::new(false),
            &transport,
            None,
        )
        .await
        .unwrap();
        assert!(out.cost_cap_hit);
        assert!(out.judgments.contains_key("a"));
        assert!(out.fallback_ids.contains("b"), "b never got a batch");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn partial_ids_fall_back() {
        // Model answered only "a"; "b" must fall back, not vanish.
        let content = r#"[{"id":"a","tier":2,"confidence":0.9,"summary":"s","reasoning":"r"}]"#;
        let transport = OkTransport {
            content: content.into(),
            cost: 0.0,
        };
        let out = reason_middle_band_with(
            vec![candidate("a", 50.0), candidate("b", 45.0)],
            PrivacyTier::Minimal,
            &Reasoning::default(),
            "m",
            "k",
            true,
            "test-partial",
            &[],
            sink().as_ref(),
            &std::sync::atomic::AtomicBool::new(false),
            &transport,
            None,
        )
        .await
        .unwrap();
        assert!(out.judgments.contains_key("a"));
        assert!(out.fallback_ids.contains("b"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancellation_between_batches() {
        let transport = FailTransport;
        let cancel = std::sync::atomic::AtomicBool::new(true);
        let result = reason_middle_band_with(
            vec![candidate("a", 50.0)],
            PrivacyTier::Minimal,
            &Reasoning::default(),
            "m",
            "k",
            true,
            "test-cancel",
            &[],
            sink().as_ref(),
            &cancel,
            &transport,
            None,
        )
        .await;
        assert!(matches!(result, Err(DriftError::Cancelled)));
    }

    #[test]
    fn fallback_tier_mapping() {
        assert_eq!(fallback_tier(70.0), Tier::MessageInABottle);
        assert_eq!(fallback_tier(50.0), Tier::Current);
    }
}
