# DriftWood Backend — Status & Context for the App Team

Status: **built and verified** (2026-09-26). All logic lives in `crates/driftwood-core`;
the app shell (Tauri) and MCP server are transport-only wrappers.

## Repo layout

```
crates/
├── driftwood-core/   # ALL logic — no UI, no Tauri, no MCP deps
│   └── src/
│       ├── lib.rs        # re-exports + DriftError
│       ├── types.rs      # Candidate, ReportEntry, Rule, tiers, bands…
│       ├── config.rs     # DriftTuning (TOML), ScanConfig, default model
│       ├── events.rs     # ScanEvent enum + EventSink trait
│       ├── paths.rs      # base dir: ~/Library/Application Support/DriftWood
│       ├── engine.rs     # run_scan + apply_correction (the ONLY entry points)
│       ├── scan/         # scope, spotlight (mdfind/mdls), walk, orphan, hard_rules
│       ├── score/        # 7 heuristics + banding
│       ├── reason/       # OpenRouter client, payloads, prompts, batching
│       ├── rules/        # memory folder, rule engine, distiller
│       └── report/       # Report model, grouping, persistence
├── driftwood-cli/        # headless binary (dogfooding/tuning workhorse)
└── driftwood-mcp/        # rmcp stdio server (tools + read-only resources)
```

## The two entry points the app must call

```rust
run_scan(config: ScanConfig, sink: Arc<dyn EventSink>, handle: ScanHandle)
    -> Result<Report, DriftError>

apply_correction(request: CorrectionRequest, memory_dir_override: Option<PathBuf>)
    -> Result<Vec<Rule>, DriftError>
```

- `ScanConfig`: `scopes` (Low/Medium/High categories),
  `privacy_tier` (Minimal/Standard/Deep), `stage2` (bool, LLM on/off),
  `model` (setting, never a constant), `api_key` (from env — core never
  persists it), `rules` (snapshot from the memory folder),
  `tuning` (`DriftTuning`), `persist` (bool).
- `ScanHandle::cancel()` — cancellation is checked between phases and
  between LLM batches (never mid-batch). Fine to wire to a Cancel button.
- Core enforces a single-flight lock (`scan.lock`, stale after 6 h) so two
  scans can't overlap.

## Event feed (themed status pipeline)

`EventSink` receives `ScanEvent`s: phase changes, running counters
(files searched, bytes searched, candidates, recoverable bytes), notices,
and warnings. Phases map to the themed labels from the notes:

| Phase        | Theme                    |
|--------------|--------------------------|
| Enumerating  | "Searching for driftwood" |
| Wading       | "Wading in"              |
| Filtering    | "Following the current"  |
| Scoring      | "Scoring"                |
| Reasoning    | "Traveling to the river" |
| Assembling   | "Sorting the driftwood"  |

The CLI renders these to stderr; the app should map them to the Section 6
labels with disclosure arrows (notices carry the raw detail).

## Report model (everything the UI shows)

`Report` → `groups: [GroupSummary]` (per category: item count + total
bytes) → `entries: [ReportEntry]`. Each `ReportEntry`:

```json
{
  "candidate": { "id", "path", "kind", "size_bytes", "kind_stats",
                 "last_used_date", "last_used_from_spotlight",
                 "modified_date", "created_date", "orphan_status",
                 "scope_category", "score", "score_components",
                 "band" },
  "tier": 1..4,
  "tier_source": "auto_high|auto_low|rule|llm|fallback|never_flag",
  "summary": "one sentence",
  "reasoning": "full LLM rationale (dropdown content)",
  "confidence": 0..1,
  "llm_model": "...",
  "privacy_tier_used": "...",
  "rule_id": "..."          // when tier_source == rule
}
```

Tier names (already in the model): 1 Driftwood, 2 Message in a Bottle,
3 Current, 4 Source. `tier_source: "fallback"` must be surfaced honestly
(themed: "Snagged") — never hide it. Reports persist to
`~/Library/Application Support/DriftWood/reports/` (timestamped +
`last-report.json`).

## Behaviors the UI must honor

- **Read-only**: no scan path can delete/move/write anything outside the
  DriftWood base dir. "Reveal in Finder" runs `open -R <path>`.
- **Still in the current**: anything used ≤ 21 days is filtered out,
  INCLUDING folders whose contents were touched (Spotlight-positive
  recency only; missing data never blocks candidacy).
- **Never-flag list**: Keychains, MobileSync, Mail/Messages data, group
  containers, etc. always land tier 4 minimum with `tier_source:
  never_flag`.
- **Tier override** → calls `apply_correction`, which appends to
  `preferences.jsonl`, re-distills `rules.json` locally (no LLM), and logs
  to `session-log.jsonl`. Rules pin tiers on the next scan and skip the
  LLM for matched candidates.
- **Privacy tiers**: Minimal sends no paths/filenames to the LLM
  (enforced by tests); Standard sends full paths, never content; Deep adds
  a depth-1 folder listing capped at 100 names. Show the user the model +
  tier used per entry; show cost before Standard/Deep scans.
- **Cost cap**: default $0.50/scan (Decision #3); when hit, remaining
  items degrade to heuristic tiers and the report carries a
  `cost_cap` warning ("Snagged — ran out of river").

## Verification done

- 61 core tests + workspace clean build, 0 clippy warnings.
- Live dogfood scan on this machine (low scope, no LLM): 351 candidates,
  3.1 M files measured, 23.7 GiB recoverable; top band = orphaned caches
  (Epic Games Launcher, Kagi — genuinely uninstalled, correctly detected),
  bottom band = Apple/system data only.
- MCP server verified over stdio: tools `scan`, `get_report`, `reveal`,
  `correct_tier`; resources `memory://rules`, `memory://preferences`.

## What still needs the design agent

- Tauri app shell (`app/src-tauri`): wire Tauri commands to
  `run_scan`/`apply_correction`, stream `ScanEvent`s to the frontend,
  run `open -R` via the shell plugin.
- Svelte frontend: onboarding (scope presets, FDA explanation),
  scan screen (live counters + themed status), notebook report UI,
  theme/animation (Phases 4–6 of the implementation plan).
- No scan/reasoning logic belongs in the wrapper — the core is the product.