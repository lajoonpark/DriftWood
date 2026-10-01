# New Ideas — Bulk Deletion UX + Middle Band Token Problem

*Created: 2026-09-27 | Disposable scratchpad*

---

## Context from First Real Run

- **170 GB Tier 1 (Driftwood)** — mostly folders from logs/caches, correctly auto-labeled
- **807 Tier 3 (Current)** — middle-band folders, LLM reasoning failed (bug)
- Rest Tier 4 (Source)

---

## Problem 1: "I have 1,000+ findings to manually delete"

### Decision: **No auto-delete**. Ever.
Breaks the core trust model: "DriftWood deleted nothing. It never does." (footer in Report.svelte)

### Solutions That Keep Read-Only Promise

| Idea | Effort | User Value |
|------|--------|------------|
| **Reveal All Tier 1 in Finder** (one click, `open -R` multi-path) | Low | Select all → Cmd+Delete = done in 2 sec |
| **Keyboard triage in Report** — Space=QuickLook, Cmd+Delete=Trash, J/K=nav, 1-4=re-stamp | Medium | Power users clear 1000 items in minutes |
| **Export cleanup script** (`driftwood-cleanup.sh`) — plain text, user audits/runs | Low | Transparent, editable, no trust violation |
| **Handoff to DaisyDisk / GrandPerspective / AppCleaner** — "Open in ___" button | Low | DriftWood = analyzer, not deleter |

**Recommended**: #1 + #2 + #3. Covers both "just nuke it" and "I want to review" users.

---

## Problem 2: 800+ folders in middle band = too many LLM calls

### Root Cause
- Banding defaults: 15% High / 70% Middle / 15% Low
- Cache/log folders in Low scope *should* score High but don't (scoring weights don't favor known cache roots enough)
- Result: hundreds of obvious Tier 1 folders sent to LLM

### Fixes (Combine Freely)

#### A. Widen Auto Bands (Immediate Config)
```toml
# ~/.config/driftwood/driftwood.toml
[banding]
high_quantile = 0.25   # was 0.15
low_quantile = 0.25    # was 0.15
# → 25% auto-High, 50% middle, 25% auto-Low
```

#### B. Scope-Aware Banding (Code Fix)
Low scope (caches/logs) → higher `high_quantile` (e.g., 0.30). These folders *are* the definition of Driftwood.

#### C. Folder-Type Heuristic Auto-High (Code Fix)
```rust
// In engine.rs, before banding:
if unit.category == ScopeCategory::Low && is_known_cache_root(&unit.path) {
    candidate.band = Band::High;  // Skip LLM entirely
}
```
Known cache roots: `~/Library/Caches`, `/tmp`, `~/Library/Logs`, orphaned `Application Support`.

#### D. Cluster & Judge Representatives (v2 Smart Batching)
Group middle-band by `(parent_dir, orphan_status, cache_like_ratio_bucket)`:
- `~/Library/Caches/com.google.Chrome/*` (47 folders) → judge 1, propagate to 47
- `~/Library/Logs/DiagnosticReports/*` (120) → judge 1
- Cuts 800 → ~20-30 LLM calls

#### E. "Express Scan" Mode (Product Toggle)
UI toggle: **"Skip AI reasoning (fast, free)"**
- High band → Tier 1 (Driftwood)
- Middle band → Tier 3 (Current) via `fallback_tier(score)`
- Low band → Tier 4 (Source)
- Cost: $0, instant, honest about heuristic-only

---

## Recommended Priority

1. **Express Scan toggle** (UI + engine flag) — solves cost + speed for repeat scans
2. **Folder-type auto-High** (engine.rs) — fixes the heuristic bug causing 800 middle items
3. **Reveal All Tier 1 in Finder** (bridge + UI button) — solves bulk delete UX
4. **Widen banding defaults** (config.rs) — quick win while #2 lands
5. **Cluster deduplication** — v2 if middle band still >200 after #1-4
6. **Keyboard triage / export script** — polish for power users

---

## Token Math (for reference)

| Model | 800 items, 25/batch | Input Cost | Output Cost | Total |
|-------|---------------------|------------|-------------|-------|
| gpt-4o-mini | 32 batches | ~$0.045 | ~$0.05 | ~$0.10 |
| Claude 3.5 Sonnet | 32 batches | ~$0.90 | ~$1.20 | ~$2.10 |
| Deep tier (+ listings) | 32 batches | 3-5× | 3-5× | $$$ |

With current $0.50 cap: gpt-4o-mini fits, anything else hits cap → fallbacks.

---

## Files to Touch

| Change | File |
|--------|------|
| Express Scan config + UI | `app/src/lib/types.ts` (ScanConfig), `app/src/screens/Scan.svelte` |
| Folder-type auto-High | `crates/driftwood-core/src/engine.rs` (before banding) |
| Reveal All in Finder | `app/src/lib/bridge.ts` (add `revealAll`), `app/src/screens/Report.svelte` (button) |
| Widen banding defaults | `crates/driftwood-core/src/config.rs` (Banding::default) |
| Cluster deduplication | `crates/driftwood-core/src/reason/mod.rs` (before batching) |

---

## Notes

- The LLM reasoning bug (failed on 807 items) is separate — fix that first, then apply above.
- All fixes preserve "DriftWood never deletes" — the trust moat.
- Config-driven where possible so dogfooding can tune without rebuilds.