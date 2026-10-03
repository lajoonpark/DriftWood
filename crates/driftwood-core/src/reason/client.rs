//! OpenRouter client (plan §3.3.1): enforces `provider.zdr = true` +
//! `data_collection: deny` on EVERY call unless the user explicitly opted
//! out of ZDR-only routing (settings "Danger zone"), timeouts + retries
//! with backoff, JSON-mode-friendly, cost accounting from `usage`.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use crate::Result;

pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

/// Boxed future so the trait stays object-safe (test doubles) and every
/// returned future is `Send` (spawned into JoinSet tasks).
pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Progress reported during a streamed call. The transport accumulates the
/// content itself; these updates exist for progress UI and contract tests.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamUpdate {
    /// HTTP status of the response (headers received).
    Status(u16),
    /// A `choices[0].delta.content` fragment.
    Delta(String),
    /// Final usage chunk (OpenRouter appends it with `include_usage`).
    Usage {
        cost_usd: f64,
        prompt_tokens: u64,
        completion_tokens: u64,
    },
    /// An OpenRouter `error` object (top-level or a mid-stream chunk).
    Error(String),
}

/// Everything one streamed call produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StreamedResponse {
    /// Accumulated `delta.content` (fed to the tolerant judgment parser).
    pub content: String,
    pub cost_usd: f64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// Transport abstraction so contract tests can replay recorded fixtures.
///
/// The streaming method is the production path: it reports progress as SSE
/// chunks arrive and honors `cancel` between chunks (checked inside the
/// stream loop, so an abort drops the in-flight request immediately rather
/// than waiting for the body to finish). The non-streaming `post_json` is
/// kept for callers/tests that want the plain request/response shape.
pub trait Transport: Send + Sync {
    fn post_json<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        body: String,
    ) -> BoxFut<'a, Result<(u16, String)>>;

    /// Streamed POST. Returns the accumulated content + final usage, or
    /// `Err(Cancelled)` when `cancel` flips mid-stream (the request future
    /// is dropped, aborting the HTTP call).
    fn post_json_stream<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        body: String,
        on_update: &'a (dyn Fn(&StreamUpdate) + Send + Sync),
        cancel: &'a AtomicBool,
    ) -> BoxFut<'a, Result<StreamedResponse>>;
}

/// Build the request body for OpenRouter. Pure function — golden-testable.
///
/// `enforce_zdr` restricts routing to zero-data-retention providers; when
/// false the `provider` block is omitted entirely so OpenRouter routes
/// freely (needed for most free / non-ZDR models). There is no middle
/// ground by design: either the full privacy posture or none of it.
///
/// Streaming is always on (`stream` + `stream_options.include_usage`) so
/// progress and cost land as the call runs; OpenRouter appends a final
/// usage chunk carrying cost + token counts.
pub fn build_request_body(model: &str, system: &str, user: &str, enforce_zdr: bool) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ],
        "response_format": {"type": "json_object"},
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if enforce_zdr {
        body["provider"] = serde_json::json!({
            "zdr": true,
            "data_collection": "deny"
        });
    }
    body
}

pub fn auth_headers(api_key: &str) -> Vec<(String, String)> {
    vec![
        ("Authorization".into(), format!("Bearer {api_key}")),
        ("Content-Type".into(), "application/json".into()),
        ("HTTP-Referer".into(), "https://github.com/driftwood".into()),
        ("X-Title".into(), "DriftWood".into()),
    ]
}

/// One LLM judgment for a candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmJudgment {
    pub id: String,
    pub tier: u8,
    pub confidence: f64,
    pub summary: String,
    pub reasoning: String,
}

/// Raw model + usage accounting for one call.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub content: String,
    pub cost_usd: f64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// Parse a chat-completions response body (pure; contract-tested against
/// recorded fixtures, including malformed JSON).
pub fn parse_chat_response(body: &str) -> Result<LlmResponse> {
    #[derive(Deserialize)]
    struct Usage {
        #[serde(default)]
        prompt_tokens: u64,
        #[serde(default)]
        completion_tokens: u64,
        #[serde(default)]
        cost: Option<f64>,
        #[serde(default)]
        total_cost: Option<f64>,
    }
    #[derive(Deserialize)]
    struct Choice {
        message: ChoiceMessage,
    }
    #[derive(Deserialize)]
    struct ChoiceMessage {
        content: Option<String>,
    }
    #[derive(Deserialize)]
    struct Envelope {
        choices: Vec<Choice>,
        #[serde(default)]
        usage: Option<Usage>,
        #[serde(default)]
        error: Option<serde_json::Value>,
    }

    let env: Envelope = serde_json::from_str(body).map_err(|e| {
        crate::DriftError::Reason(format!("malformed OpenRouter response: {e}"))
    })?;

    if let Some(err) = env.error {
        return Err(crate::DriftError::Reason(format!(
            "OpenRouter error: {err}"
        )));
    }

    let content = env
        .choices
        .first()
        .and_then(|c| c.message.content.clone())
        .ok_or_else(|| crate::DriftError::Reason("empty choices in response".into()))?;

    let (cost, prompt_tokens, completion_tokens) = env
        .usage
        .map(|u| {
            (
                u.cost.or(u.total_cost).unwrap_or(0.0),
                u.prompt_tokens,
                u.completion_tokens,
            )
        })
        .unwrap_or((0.0, 0, 0));

    Ok(LlmResponse {
        content,
        cost_usd: cost,
        prompt_tokens,
        completion_tokens,
    })
}

/// One parsed SSE `data:` payload.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SseChunk {
    /// `choices[0].delta.content` fragment, when present.
    pub delta_content: Option<String>,
    /// Usage object from the final chunk (or any chunk carrying one).
    pub usage: Option<StreamedResponse>,
    /// OpenRouter error object (mid-stream failures arrive this way before
    /// any content — handled as a batch failure, not a parse panic).
    pub error: Option<String>,
}

/// Parse the `data:` payload of one SSE line. Pure; contract-tested.
/// Returns `Ok(None)` for the `data: [DONE]` terminator.
pub fn parse_sse_data(data: &str) -> Result<Option<SseChunk>> {
    if data.trim() == "[DONE]" {
        return Ok(None);
    }
    let value: serde_json::Value = serde_json::from_str(data).map_err(|e| {
        let snippet = truncate_str(data, 200);
        crate::DriftError::Reason(format!("malformed SSE chunk: {e}: {snippet}"))
    })?;

    if let Some(err) = value.get("error") {
        return Ok(Some(SseChunk {
            error: Some(err.to_string()),
            ..SseChunk::default()
        }));
    }

    let delta_content = value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("delta"))
        .and_then(|d| d.get("content"))
        .and_then(|v| v.as_str())
        .map(String::from);

    let usage = value.get("usage").map(|u| {
        let prompt_tokens = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let completion_tokens = u
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cost = u
            .get("cost")
            .and_then(|v| v.as_f64())
            .or_else(|| u.get("total_cost").and_then(|v| v.as_f64()))
            .unwrap_or(0.0);
        StreamedResponse {
            cost_usd: cost,
            prompt_tokens,
            completion_tokens,
            ..StreamedResponse::default()
        }
    });

    if delta_content.is_none() && usage.is_none() {
        // Chunk with neither content nor usage (e.g. a role-only first
        // chunk) — nothing to accumulate.
        return Ok(Some(SseChunk::default()));
    }

    Ok(Some(SseChunk {
        delta_content,
        usage,
        error: None,
    }))
}

/// Extract `data:` payloads from a raw SSE buffer fragment. Pure; returns
/// complete `data:` lines only (the caller keeps the remainder of a split
/// line buffered). SSE comments (`:`), `event:`/`id:` fields are ignored.
pub fn extract_sse_data_lines(buffer: &str) -> (Vec<String>, String) {
    let mut out = Vec::new();
    let mut remainder_start = 0usize;
    let mut search_from = 0usize;
    while let Some(rel) = buffer[search_from..].find('\n') {
        let abs = search_from + rel;
        let line = &buffer[remainder_start..abs];
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(data) = line.strip_prefix("data: ") {
            out.push(data.to_string());
        } else if let Some(data) = line.strip_prefix("data:") {
            out.push(data.trim_start().to_string());
        }
        remainder_start = abs + 1;
        search_from = remainder_start;
    }
    (out, buffer[remainder_start..].to_string())
}

/// Attempt to repair a JSON array/object of judgments that a stream cut
/// off mid-write: truncate at the last complete top-level item and close
/// the open containers. Returns `None` when nothing complete survives.
/// Used only after a plain parse failed; candidates the salvage can't
/// recover still land in `fallback_ids` (never silently dropped).
pub fn salvage_truncated_json(content: &str) -> Option<String> {
    let trimmed = content.trim();
    let trimmed = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    let trimmed = trimmed
        .strip_suffix("```")
        .unwrap_or(trimmed)
        .trim();

    let first = trimmed.as_bytes().first().copied()?;
    if first != b'[' && first != b'{' {
        return None;
    }

    let bytes = trimmed.as_bytes();
    // Depth of a complete top-level judgment item: directly inside a bare
    // array (depth 1), or inside the items/results array of a wrapper
    // object (depth 2).
    let items_depth = if first == b'[' { 1 } else { 2 };
    let mut in_string = false;
    let mut escaped = false;
    let mut depth = 0usize;
    let mut boundary: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            b'{' | b'[' if !in_string => depth += 1,
            b'}' | b']' if !in_string => {
                depth = depth.saturating_sub(1);
                if depth == items_depth {
                    boundary = Some(i + 1);
                }
            }
            _ => {}
        }
    }

    let cut = boundary?;
    let head = &trimmed[..cut];
    Some(if first == b'[' {
        format!("{head}]")
    } else {
        format!("{head}]}}")
    })
}

fn truncate_str(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Does this JSON value look like one judgment object? Used to recognize
/// shapes the schema didn't ask for: a bare single judgment (common when
/// the batch holds exactly one item) and unknown wrapper keys.
fn looks_like_judgment(v: &serde_json::Value) -> bool {
    v.as_object()
        .map(|o| {
            o.get("id").map(|i| i.is_string()).unwrap_or(false)
                && o.get("tier").is_some()
        })
        .unwrap_or(false)
}

/// Read the tier off one judgment object. The schema says an integer
/// 1..=4, but models occasionally answer `"tier": "3"` or `"tier": 3.0` —
/// accept those; anything outside 1..=4 stays unusable.
fn tier_of(item: &serde_json::Value) -> Option<u8> {
    let t = item.get("tier")?;
    let n = t
        .as_u64()
        .or_else(|| t.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as u64))
        .or_else(|| t.as_str().and_then(|s| s.trim().parse::<u64>().ok()))?;
    if (1..=4).contains(&n) {
        Some(n as u8)
    } else {
        None
    }
}

/// Parse the model's content into judgments. Tolerates the model wrapping
/// the array in an object ("items"/"results"), a bare single judgment
/// object, an unknown wrapper key holding a judgment-shaped array, or code
/// fences. Unknown ids, invalid tiers, or non-object entries are skipped by
/// the caller via `Err`-per-item semantics: this returns whatever parsed
/// cleanly plus the ids it saw.
pub fn parse_judgments(content: &str) -> Result<Vec<LlmJudgment>> {
    let trimmed = content.trim();
    let trimmed = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    let trimmed = trimmed
        .strip_suffix("```")
        .unwrap_or(trimmed)
        .trim();

    let value: serde_json::Value = serde_json::from_str(trimmed).map_err(|e| {
        crate::DriftError::Reason(format!("model output is not valid JSON: {e}"))
    })?;

    let items: Vec<&serde_json::Value> = match &value {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(obj) => {
            let known = obj
                .get("items")
                .or_else(|| obj.get("results"))
                .or_else(|| obj.get("judgments"))
                .and_then(|v| v.as_array())
                .map(|a| a.iter().collect::<Vec<_>>());
            if let Some(items) = known {
                items
            } else if looks_like_judgment(&value) {
                // One-item batch: several models drop the requested array
                // and answer with the single judgment object itself.
                vec![&value]
            } else {
                // Unrecognized wrapper ("verdicts", "output", …): accept the
                // first top-level array whose entries look like judgments.
                obj.values()
                    .find(|v| {
                        v.as_array().is_some_and(|a| {
                            !a.is_empty() && a.iter().any(looks_like_judgment)
                        })
                    })
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().collect())
                    .unwrap_or_default()
            }
        }
        _ => vec![],
    };

    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(id) = item.get("id").and_then(|v| v.as_str()).map(String::from) else {
            continue;
        };
        let Some(tier) = tier_of(item) else {
            continue;
        };
        let confidence = item
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.5)
            .clamp(0.0, 1.0);
        let summary = item
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let reasoning = item
            .get("reasoning")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(LlmJudgment {
            id,
            tier,
            confidence,
            summary,
            reasoning,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_body_enforces_zdr_by_default() {
        let body = build_request_body("openai/gpt-4o-mini", "sys", "user", true);
        let provider = &body["provider"];
        assert_eq!(provider["zdr"], serde_json::json!(true));
        assert_eq!(provider["data_collection"], serde_json::json!("deny"));
        assert_eq!(body["model"], serde_json::json!("openai/gpt-4o-mini"));
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn request_body_streams_with_usage() {
        let body = build_request_body("openai/gpt-4o-mini", "sys", "user", true);
        assert_eq!(body["stream"], serde_json::json!(true));
        assert_eq!(
            body["stream_options"]["include_usage"],
            serde_json::json!(true)
        );
        // Streaming must not quietly drop the privacy routing.
        assert_eq!(body["provider"]["zdr"], serde_json::json!(true));
    }

    #[test]
    fn request_body_without_zdr_omits_provider_block() {
        let body = build_request_body("openai/gpt-4o-mini", "sys", "user", false);
        assert!(body.get("provider").is_none());
        // Everything else is unchanged — only the routing posture relaxes.
        assert_eq!(body["model"], serde_json::json!("openai/gpt-4o-mini"));
        assert_eq!(body["response_format"]["type"], serde_json::json!("json_object"));
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn parse_chat_fixture() {
        let fixture = r#"{"id":"x","choices":[{"message":{"content":"[]"}}],"usage":{"prompt_tokens":100,"completion_tokens":50,"cost":0.00012}}"#;
        let r = parse_chat_response(fixture).unwrap();
        assert_eq!(r.content, "[]");
        assert!((r.cost_usd - 0.00012).abs() < 1e-9);
        assert_eq!(r.prompt_tokens, 100);
    }

    #[test]
    fn parse_chat_malformed() {
        assert!(parse_chat_response("not json at all").is_err());
        assert!(parse_chat_response(r#"{"choices":[]}"#).is_err());
        assert!(parse_chat_response(
            r#"{"error":{"message":"rate limited"}}"#
        )
        .is_err());
    }

    #[test]
    fn parse_judgments_happy() {
        let content = r#"[{"id":"c-1","tier":1,"confidence":0.9,"summary":"s","reasoning":"r"},
                            {"id":"c-2","tier":4,"confidence":0.5,"summary":"s2","reasoning":"r2"}]"#;
        let j = parse_judgments(content).unwrap();
        assert_eq!(j.len(), 2);
        assert_eq!(j[0].id, "c-1");
        assert_eq!(j[0].tier, 1);
    }

    #[test]
    fn parse_judgments_fenced_and_wrapped() {
        let fenced = "```json\n[{\"id\":\"a\",\"tier\":2,\"confidence\":0.5,\"summary\":\"\",\"reasoning\":\"\"}]\n```";
        assert_eq!(parse_judgments(fenced).unwrap().len(), 1);

        let wrapped = r#"{"items":[{"id":"a","tier":3,"confidence":0.5,"summary":"","reasoning":""}]}"#;
        assert_eq!(parse_judgments(wrapped).unwrap().len(), 1);
    }

    #[test]
    fn parse_judgments_malformed_and_invalid_items() {
        assert!(parse_judgments("[{]").is_err());
        // invalid tier and missing id are skipped, not fatal
        let mixed = r#"[{"id":"ok","tier":2,"confidence":0.5,"summary":"","reasoning":""},
                        {"id":"bad","tier":9,"confidence":0.5,"summary":"","reasoning":""},
                        {"tier":1,"confidence":0.5,"summary":"","reasoning":""}]"#;
        let j = parse_judgments(mixed).unwrap();
        assert_eq!(j.len(), 1);
        assert_eq!(j[0].id, "ok");
    }

    /// Issue #1 ("Ask the river not working"): one-item batches used to
    /// parse to ZERO judgments when the model dropped the requested array
    /// and answered with the bare judgment object — a clean parse, an
    /// empty persist file, and only "the river did not answer" as symptom.
    #[test]
    fn parse_judgments_bare_single_object() {
        let content =
            r#"{"id":"c-a1b2c3d4e5f60718","tier":2,"confidence":0.8,"summary":"s","reasoning":"r"}"#;
        let j = parse_judgments(content).unwrap();
        assert_eq!(j.len(), 1);
        assert_eq!(j[0].id, "c-a1b2c3d4e5f60718");
        assert_eq!(j[0].tier, 2);
    }

    /// Same failure family: unknown wrapper keys ("verdicts", "output"…).
    #[test]
    fn parse_judgments_unknown_wrapper_key() {
        let content = r#"{"verdicts":[{"id":"a","tier":3,"confidence":0.5,"summary":"","reasoning":""}]}"#;
        let j = parse_judgments(content).unwrap();
        assert_eq!(j.len(), 1);
        assert_eq!(j[0].tier, 3);
    }

    /// Models occasionally emit the tier as a string or float.
    #[test]
    fn parse_judgments_string_and_float_tiers() {
        let as_string = r#"[{"id":"a","tier":"3","confidence":0.5,"summary":"","reasoning":""}]"#;
        assert_eq!(parse_judgments(as_string).unwrap()[0].tier, 3);
        let as_float = r#"[{"id":"a","tier":4.0,"confidence":0.5,"summary":"","reasoning":""}]"#;
        assert_eq!(parse_judgments(as_float).unwrap()[0].tier, 4);
        // out of range stays unusable in every encoding
        let bad = r#"[{"id":"a","tier":"9","confidence":0.5,"summary":"","reasoning":""}]"#;
        assert!(parse_judgments(bad).unwrap().is_empty());
    }

    /// A bare single judgment wins over the unknown-wrapper scan (it IS a
    /// judgment), and a non-judgment object still parses to empty.
    #[test]
    fn parse_judgments_shape_precedence() {
        let single = r#"{"id":"a","tier":1,"confidence":0.5,"summary":"","reasoning":""}"#;
        assert_eq!(parse_judgments(single).unwrap().len(), 1);
        let noise = r#"{"hello":"world"}"#;
        assert!(parse_judgments(noise).unwrap().is_empty());
        // known keys keep precedence over a bare-object fallback
        let both =
            r#"{"items":[{"id":"a","tier":2,"confidence":0.5,"summary":"","reasoning":""}]}"#;
        assert_eq!(parse_judgments(both).unwrap()[0].id, "a");
    }

    #[test]
    fn sse_delta_chunks_parse() {
        let data = r#"{"id":"x","choices":[{"delta":{"content":"{\"id\":\"c-1\"}"}}]}"#;
        let c = parse_sse_data(data).unwrap().unwrap();
        assert_eq!(
            c.delta_content.as_deref(),
            Some("{\"id\":\"c-1\"}")
        );
        assert!(c.usage.is_none());
        assert!(c.error.is_none());
    }

    #[test]
    fn sse_usage_chunk_parses() {
        let data = r#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":50,"cost":0.00012}}"#;
        let c = parse_sse_data(data).unwrap().unwrap();
        let u = c.usage.unwrap();
        assert!((u.cost_usd - 0.00012).abs() < 1e-9);
        assert_eq!(u.prompt_tokens, 100);
        assert_eq!(u.completion_tokens, 50);
        assert!(c.delta_content.is_none());
    }

    #[test]
    fn sse_done_returns_none() {
        assert!(parse_sse_data("[DONE]").unwrap().is_none());
    }

    #[test]
    fn sse_midstream_error_is_a_chunk_not_a_panic() {
        let data = r#"{"error":{"message":"rate limited","code":429}}"#;
        let c = parse_sse_data(data).unwrap().unwrap();
        assert!(c.error.unwrap().contains("rate limited"));
    }

    #[test]
    fn sse_malformed_is_error() {
        assert!(parse_sse_data("not json").is_err());
    }

    #[test]
    fn extract_sse_lines_across_fragments() {
        // Two complete lines plus a split third — only complete lines out.
        let buf = "event: message\ndata: {\"a\":1}\n\ndata: [DONE]\ndata: {\"bro";
        let (lines, rest) = extract_sse_data_lines(buf);
        assert_eq!(lines, vec![r#"{"a":1}"#.to_string(), "[DONE]".to_string()]);
        assert_eq!(rest, "data: {\"bro");

        // bare `data:` without a space is tolerated
        let (lines, _) = extract_sse_data_lines("data:[DONE]\n");
        assert_eq!(lines, vec!["[DONE]".to_string()]);
    }

    #[test]
    fn salvage_repairs_truncated_bare_array() {
        let cut = r#"[{"id":"a","tier":1,"confidence":0.9,"summary":"ok","reasoning":"r"},{"id":"b","ti"#;
        let salvaged = salvage_truncated_json(cut).unwrap();
        assert_eq!(salvaged, r#"[{"id":"a","tier":1,"confidence":0.9,"summary":"ok","reasoning":"r"}]"#);
        assert_eq!(parse_judgments(&salvaged).unwrap().len(), 1);
    }

    #[test]
    fn salvage_repairs_truncated_wrapped_object() {
        let cut = r#"{"items":[{"id":"a","tier":2,"confidence":0.5,"summary":"","reasoning":""},{"id":"b""#;
        let salvaged = salvage_truncated_json(cut).unwrap();
        assert_eq!(parse_judgments(&salvaged).unwrap().len(), 1);
    }

    #[test]
    fn salvage_ignores_nested_close_and_strings() {
        // last '}' is nested; the boundary must be the complete item before it
        let cut = r#"[{"id":"a","tier":1,"confidence":0.9,"summary":"{not a bracket}","reasoning":"r"},{"id":"b","summ"#;
        let salvaged = salvage_truncated_json(cut).unwrap();
        let j = parse_judgments(&salvaged).unwrap();
        assert_eq!(j.len(), 1);
        assert_eq!(j[0].id, "a");
    }

    #[test]
    fn salvage_gives_up_without_complete_items() {
        assert!(salvage_truncated_json(r#"[{"id":"a"#).is_none());
        assert!(salvage_truncated_json("lorem ipsum").is_none());
    }
}
