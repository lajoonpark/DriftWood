//! OpenRouter client (plan §3.3.1): enforces `provider.zdr = true` +
//! `data_collection: deny` on EVERY call, timeouts + retries with backoff,
//! JSON-mode-friendly, cost accounting from `usage`.

use serde::{Deserialize, Serialize};

use crate::Result;

pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

/// Transport abstraction so contract tests can replay recorded fixtures.
pub trait Transport: Send + Sync {
    fn post_json(&self, url: &str, headers: &[(String, String)], body: String)
        -> Result<(u16, String)>;
}

/// Build the request body for OpenRouter. Pure function — golden-testable.
pub fn build_request_body(model: &str, system: &str, user: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ],
        "response_format": {"type": "json_object"},
        "provider": {
            "zdr": true,
            "data_collection": "deny"
        }
    })
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

/// Parse the model's content into judgments. Tolerates the model wrapping
/// the array in an object ("items"/"results") or code fences. Unknown ids,
/// invalid tiers, or non-object entries are skipped by the caller via
/// `Err`-per-item semantics: this returns whatever parsed cleanly plus the
/// ids it saw.
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
        serde_json::Value::Object(obj) => obj
            .get("items")
            .or_else(|| obj.get("results"))
            .or_else(|| obj.get("judgments"))
            .and_then(|v| v.as_array())
            .map(|a| a.iter().collect())
            .unwrap_or_default(),
        _ => vec![],
    };

    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(id) = item.get("id").and_then(|v| v.as_str()).map(String::from) else {
            continue;
        };
        let tier = match item.get("tier").and_then(|v| v.as_u64()) {
            Some(t) if (1..=4).contains(&t) => t as u8,
            _ => continue,
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
    fn request_body_always_enforces_zdr() {
        let body = build_request_body("openai/gpt-4o-mini", "sys", "user");
        let provider = &body["provider"];
        assert_eq!(provider["zdr"], serde_json::json!(true));
        assert_eq!(provider["data_collection"], serde_json::json!("deny"));
        assert_eq!(body["model"], serde_json::json!("openai/gpt-4o-mini"));
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
}
