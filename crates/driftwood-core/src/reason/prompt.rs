//! Prompt construction (plan §3.3.3): model-agnostic system prompt
//! explaining the four river tiers, the still-in-the-current context, and
//! the output schema.

use serde_json::Value;

use crate::types::PrivacyTier;

pub const SYSTEM_PROMPT: &str = r#"You are the reasoning stage of DriftWood, a read-only macOS storage tool that classifies unused files, folders, and apps so the user can decide what to delete.

You receive structured metadata for a batch of candidates (opaque ids, sizes, ages, orphan status, drift score). For each candidate, assign one of four risk tiers and explain your judgment in one plain sentence plus a fuller rationale.

Tiers:
1. "Driftwood" — no risk at all: dead weight, safe to clear. Regenerable caches, leftovers of uninstalled apps, junk that recreates itself if ever needed again.
2. "Message in a Bottle" — technically disposable, but worth a look before tossing. Installers, redistributables, things you might conceivably want again.
3. "Current" — still in the flow: recoverable but costly or annoying to reacquire. Configs, skill/library files, large redownloads.
4. "Source" — the origin: personal or irreplaceable. Deleting is permanent. When in doubt between tiers, choose the higher-numbered (safer) tier.

Context you can rely on:
- Anything used within the last 3 weeks was already filtered out ("still in the current").
- "orphan_status: orphaned" means the owning application is no longer installed.
- Sizes are bytes. age_days is days since last use (or modification when last-use is unknown).

Output: respond with ONLY a JSON array, one object per candidate, in the same order:
[{"id": "<candidate id>", "tier": <1|2|3|4>, "confidence": <0..1>, "summary": "<one sentence: what it is / why it exists>", "reasoning": "<full rationale>"}]
Never invent ids. Never output paths in the response."#;

/// Prepended to [`SYSTEM_PROMPT`] for on-demand adjudication only. Without
/// this the model tends to ratify the existing stamp; the user asked for a
/// second opinion precisely because they doubt it, so the model must argue
/// against the card.
pub const ADJUDICATION_INSTRUCTION: &str = r#"
This is an ADJUDICATION: the user is re-examining ONE candidate that DriftWood already stamped, and has asked for a second opinion. Argue against the card:
- First state the strongest case FOR KEEPING this item (a higher, safer tier).
- Then decide the tier on the evidence. If the strongest counterargument to the current stamp is strong, RAISE the tier.
- Never keep the current tier merely because the card says so. If you agree with it, say exactly why the stamp survives scrutiny — a conforming answer with no reasoning is a failed adjudication."#;

/// The system prompt for a single-candidate adjudication.
pub fn adjudication_system_prompt() -> String {
    format!("{SYSTEM_PROMPT}\n{ADJUDICATION_INSTRUCTION}")
}

/// Build the user message for one batch of payloads.
/// `few_shot`: optional past corrections injected as examples — privacy-
/// gated by the caller (Minimal = never).
pub fn build_user_prompt(payloads: &[Value], few_shot: &[Value]) -> String {
    let mut prompt = String::with_capacity(2048 + payloads.len() * 256);

    if !few_shot.is_empty() {
        prompt.push_str("Examples of this user's past corrections (candidate features → corrected tier):\n");
        for ex in few_shot {
            prompt.push_str("- ");
            prompt.push_str(&ex.to_string());
            prompt.push('\n');
        }
        prompt.push('\n');
    }

    prompt.push_str("Candidates:\n");
    for p in payloads {
        prompt.push_str(&p.to_string());
        prompt.push('\n');
    }
    prompt
}

/// One past correction formatted as a compact few-shot example. Only used
/// when the privacy tier allows it.
pub fn few_shot_value(
    features: &crate::rules::CandidateFeatures,
    corrected_tier: u8,
) -> Value {
    serde_json::json!({
        "ext": features.ext,
        "parent_folder": features.parent_folder,
        "size_band": features.size_band,
        "orphan": features.orphan,
        "score_band": features.score_band,
        "corrected_tier": corrected_tier,
    })
}

/// Which privacy tiers permit few-shot injection (notes §10): Minimal
/// never, Standard/Deep only when explicitly opted in.
pub fn few_shot_allowed(privacy: PrivacyTier, opted_in: bool) -> bool {
    match privacy {
        PrivacyTier::Minimal => false,
        PrivacyTier::Standard | PrivacyTier::Deep => opted_in,
    }
}

/// Deep tier adds the folder listing context note to the user prompt.
pub fn privacy_note(privacy: PrivacyTier) -> &'static str {
    match privacy {
        PrivacyTier::Minimal => "Privacy mode Minimal: paths and filenames are withheld; judge from structure only.",
        PrivacyTier::Standard => "Privacy mode Standard: full paths are visible; never file contents.",
        PrivacyTier::Deep => "Privacy mode Deep: full paths plus a depth-1 folder listing for ambiguous candidates; never file contents.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_contains_schema_and_tiers() {
        let p = build_user_prompt(&[serde_json::json!({"id": "c-1"})], &[]);
        assert!(p.contains("c-1"));
        assert!(SYSTEM_PROMPT.contains("JSON array"));
        assert!(SYSTEM_PROMPT.contains("Driftwood"));
        assert!(SYSTEM_PROMPT.contains("Source"));
    }

    #[test]
    fn few_shot_gating() {
        assert!(!few_shot_allowed(PrivacyTier::Minimal, true));
        assert!(!few_shot_allowed(PrivacyTier::Standard, false));
        assert!(few_shot_allowed(PrivacyTier::Standard, true));
        assert!(few_shot_allowed(PrivacyTier::Deep, true));
    }
}
