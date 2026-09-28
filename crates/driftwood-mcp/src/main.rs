//! DriftWood MCP server (plan Phase 8): thin rmcp wrapper over the core.
//!
//! Read-only guarantee: no tool exists that can delete, move, or write
//! anything outside DriftWood's own memory folder. `reveal` only opens
//! Finder selection; `correct_tier` writes only to the memory folder.
//!
//! Run with stdio transport, e.g. in `claude_desktop_config.json`:
//! ```json
//! {
//!   "mcpServers": {
//!     "driftwood": {
//!       "command": "/path/to/driftwood-mcp"
//!     }
//!   }
//! }
//! ```

use std::path::Path;
use std::sync::Arc;

use driftwood_core::{
    CorrectionRequest, EventSink, Phase, PrivacyTier, Report, ScanConfig,
    ScanEvent, ScanHandle, ScopeCategory, Tier,
};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt, schemars,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde_json::json;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ScanRequest {
    /// Scope categories: "low", "medium", "high", or "all". Default "low".
    #[schemars(description = "Scope: low | medium | high | all")]
    pub scope: Option<String>,
    /// Privacy tier for Stage 2: minimal | standard | deep. Default standard.
    #[schemars(description = "Privacy tier: minimal | standard | deep")]
    pub privacy_tier: Option<String>,
    /// When true, skip the LLM stage entirely (heuristic tiers only).
    #[schemars(description = "Skip LLM stage (heuristic tiers only)")]
    pub dry_run: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RevealRequest {
    #[schemars(description = "Absolute path to reveal in Finder")]
    pub path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CorrectTierRequest {
    #[schemars(description = "Absolute path of the report entry to correct")]
    pub path: String,
    #[schemars(description = "Corrected tier, 1..4 (1=Driftwood, 2=Message in a Bottle, 3=Current, 4=Source)")]
    pub tier: u8,
    #[schemars(description = "Optional note about why the tier was corrected")]
    pub note: Option<String>,
}

/// Sink that collects scan progress into a compact activity log.
#[derive(Default)]
struct MemorySink {
    lines: std::sync::Mutex<Vec<String>>,
}

impl MemorySink {
    fn summary(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

fn themed(phase: Phase) -> &'static str {
    match phase {
        Phase::Enumerating => "Searching for driftwood",
        Phase::Wading => "Wading in",
        Phase::Filtering => "Following the current",
        Phase::Scoring => "Scoring",
        Phase::Reasoning => "Traveling to the river",
        Phase::Assembling => "Sorting the driftwood",
    }
}

impl EventSink for MemorySink {
    fn emit(&self, event: ScanEvent) {
        let line = match event {
            ScanEvent::Phase { phase } => themed(phase).to_string(),
            ScanEvent::FilesSearched { total } => format!("files searched: {total}"),
            ScanEvent::BytesSearched { total } => format!("bytes searched: {total}"),
            ScanEvent::CandidatesFound { total } => format!("candidates: {total}"),
            ScanEvent::RecoverableBytes { total } => format!("recoverable bytes: {total}"),
            ScanEvent::Notice { message } => message,
            ScanEvent::Warn { message } => format!("warn: {message}"),
            ScanEvent::Error { message } => format!("error: {message}"),
        };
        let mut lines = self.lines.lock().unwrap();
        if lines.len() < 200 {
            lines.push(line);
        }
    }
}

#[derive(Clone)]
pub struct DriftWoodServer {
    tool_router: ToolRouter<Self>,
}

fn parse_scope(s: &str) -> Vec<ScopeCategory> {
    match s.to_lowercase().as_str() {
        "medium" => vec![ScopeCategory::Medium],
        "high" => vec![ScopeCategory::High],
        "all" => vec![ScopeCategory::Low, ScopeCategory::Medium, ScopeCategory::High],
        _ => vec![ScopeCategory::Low],
    }
}

fn parse_privacy(s: &str) -> PrivacyTier {
    match s.to_lowercase().as_str() {
        "minimal" => PrivacyTier::Minimal,
        "deep" => PrivacyTier::Deep,
        _ => PrivacyTier::Standard,
    }
}

fn parse_tier(n: u8) -> Result<Tier, McpError> {
    Tier::try_from(n)
        .map_err(|_| McpError::invalid_params(format!("tier must be 1..4, got {n}"), None))
}

/// Compact summary of a report for tool responses.
fn summarize(report: &Report) -> serde_json::Value {
    json!({
        "scan_id": report.scan_id,
        "generated_at": report.generated_at.to_rfc3339(),
        "spotlight_available": report.spotlight_available,
        "counters": report.counters,
        "llm_cost_usd": report.llm_cost_usd,
        "cost_cap_hit": report.cost_cap_hit,
        "groups": report.groups,
        "tier_counts": {
            "1_driftwood": report.entries.iter().filter(|e| e.tier == Tier::Driftwood).count(),
            "2_message_in_a_bottle": report.entries.iter().filter(|e| e.tier == Tier::MessageInABottle).count(),
            "3_current": report.entries.iter().filter(|e| e.tier == Tier::Current).count(),
            "4_source": report.entries.iter().filter(|e| e.tier == Tier::Source).count(),
        },
        "warnings": report.warnings,
        "full_report_path": driftwood_core::paths::reports_dir().join("last-report.json"),
    })
}

#[tool_router]
impl DriftWoodServer {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    /// Run a scan. Stage 2 requires OPENROUTER_API_KEY in the environment.
    #[tool(description = "Run a read-only DriftWood scan: find unused files, folders, and apps, classify them into four risk tiers, and store a report. Never deletes anything.")]
    async fn scan(
        &self,
        Parameters(req): Parameters<ScanRequest>,
    ) -> Result<CallToolResult, McpError> {
        let scope = req.scope.unwrap_or_else(|| "low".into());
        let privacy = req.privacy_tier.unwrap_or_else(|| "standard".into());
        let dry_run = req.dry_run.unwrap_or(false);

        let sink = Arc::new(MemorySink::default());
        let config = ScanConfig {
            scopes: parse_scope(&scope),
            privacy_tier: parse_privacy(&privacy),
            stage2: !dry_run,
            model: driftwood_core::default_model().to_string(),
            api_key: std::env::var("OPENROUTER_API_KEY").ok().filter(|k| !k.is_empty()),
            allow_non_zdr: false,
            rules: driftwood_core::load_rules(None).unwrap_or_default(),
            tuning: driftwood_core::DriftTuning::default(),
            persist: true,
        };

        let report = driftwood_core::run_scan(config, sink.clone(), ScanHandle::new())
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let mut out = summarize(&report);
        out["activity"] = json!(sink.summary());
        Ok(CallToolResult::success(vec![ContentBlock::text(
            out.to_string(),
        )]))
    }

    /// Return the last persisted report in full.
    #[tool(description = "Get the full JSON of the most recent DriftWood report (tier, summary, reasoning, and metadata for every candidate).")]
    async fn get_report(&self) -> Result<CallToolResult, McpError> {
        let report: Report = Report::load_last()
            .map_err(|e| McpError::internal_error(format!("no report available: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string_pretty(&report)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?,
        )]))
    }

    /// Reveal a path in Finder (read-only; no modification).
    #[tool(description = "Reveal a file or folder in Finder (read-only; never modifies anything).")]
    async fn reveal(
        &self,
        Parameters(req): Parameters<RevealRequest>,
    ) -> Result<CallToolResult, McpError> {
        let path = Path::new(&req.path);
        if !path.exists() {
            return Err(McpError::invalid_params(
                format!("path does not exist: {}", req.path),
                None,
            ));
        }
        let status = tokio::process::Command::new("open")
            .arg("-R")
            .arg(&req.path)
            .status()
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(
            json!({"revealed": req.path, "ok": status.success()}).to_string(),
        )]))
    }

    /// Correct a tier; looks up the original tier from the last report and
    /// feeds the preference learner. Writes ONLY to DriftWood's memory dir.
    #[tool(description = "Correct the risk tier of a reported item. DriftWood learns the preference locally (no LLM, no cloud sync).")]
    async fn correct_tier(
        &self,
        Parameters(req): Parameters<CorrectTierRequest>,
    ) -> Result<CallToolResult, McpError> {
        let corrected = parse_tier(req.tier)?;
        let report: Report = Report::load_last()
            .map_err(|e| McpError::internal_error(format!("no report to correct against: {e}"), None))?;
        let entry = report
            .entries
            .iter()
            .find(|e| e.candidate.path == req.path)
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!("path not in the last report: {}", req.path),
                    None,
                )
            })?;

        let request = CorrectionRequest {
            path: req.path.clone(),
            size_bytes: entry.candidate.size_bytes,
            orphan: entry.candidate.orphan_status,
            score_band: entry.candidate.band,
            original_tier: entry.tier,
            corrected_tier: corrected,
            note: req.note.clone(),
        };

        let rules = driftwood_core::apply_correction(request, None)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![ContentBlock::text(
            json!({
                "corrected": req.path,
                "original_tier": entry.tier as u8,
                "corrected_tier": corrected as u8,
                "rules_count": rules.len(),
            })
            .to_string(),
        )]))
    }
}

/// Read-only resources: the memory folder files.
fn read_memory_file(name: &str) -> String {
    std::fs::read_to_string(driftwood_core::paths::memory_dir().join(name))
        .unwrap_or_else(|_| {
            if name == "rules.json" {
                "[]".to_string()
            } else {
                String::new()
            }
        })
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for DriftWoodServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_resources().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions(
                "DriftWood: read-only macOS drift scanner. Tools: scan, get_report, reveal, correct_tier. Resources: memory://rules, memory://preferences. DriftWood never deletes, moves, or modifies anything.",
            )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![
                Resource::new("memory://rules", "rules").with_description("DriftWood distilled rules (rules.json) — read-only"),
                Resource::new("memory://preferences", "preferences").with_description("DriftWood raw tier corrections (preferences.jsonl) — read-only"),
            ],
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        match request.uri.as_str() {
            "memory://rules" => Ok(
                ReadResourceResult::new(vec![ResourceContents::text(
                    read_memory_file("rules.json"),
                    request.uri,
                )])
                .into(),
            ),
            "memory://preferences" => Ok(
                ReadResourceResult::new(vec![ResourceContents::text(
                    read_memory_file("preferences.jsonl"),
                    request.uri,
                )])
                .into(),
            ),
            _ => Err(McpError::resource_not_found(
                "resource_not_found",
                Some(json!({ "uri": request.uri })),
            )),
        }
    }
}

#[tokio::main]
async fn main() {
    // Protocol goes over stdout — keep stderr for logs only.
    eprintln!("driftwood-mcp: serving over stdio");
    let service = DriftWoodServer::new()
        .serve(stdio())
        .await
        .inspect_err(|e| eprintln!("serving error: {e:?}"));
    match service {
        Ok(s) => {
            let _ = s.waiting().await;
        }
        Err(_) => std::process::exit(1),
    }
}
