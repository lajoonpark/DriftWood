# DriftWood — Implementation Plan

Source: `driftwood-project-notes.txt` (compiled 2026-09-25). Repo state: greenfield (`README.md`, `LICENSE` (MIT), notes only).

---

## 0. Decisions

| # | Decision | Status | Choice / default |
|---|----------|--------|------------------|
| 1 | **Tech stack** | **DECIDED (approved 2026-09-25)** | **Tauri v2 + Rust core + TypeScript web frontend (Svelte)**. Reasons: the river animation (feTurbulence/feDisplacementMap SVG filters) was already prototyped and validated in web tech — it drops straight into a webview; one Rust core crate gets wrapped by *both* the app and the MCP server (exactly the "two thin wrappers" requirement); rmcp (official Rust MCP SDK) is mature; small binary, MIT-friendly. Rejected alternative: SwiftUI + Swift core (more native feel and native `NSMetadataQuery`, but the SVG river animation must be re-created in Core Animation/Metal, and the Swift MCP SDK is less mature). |
| 2 | **Stage 2 default model** | Proposed default (vetoable) | Ship with a cheap-fast default (e.g. a Claude Haiku-class or GPT-4o-mini-class model via OpenRouter) and make the model a *setting*, not a constant. The prompt is model-agnostic; pick the final default during dogfooding. Alternative: hardcode one premium model — better reasoning, ~10× cost per scan. |
| 3 | **Cost guardrail** | Proposed default (vetoable) | Hard per-scan token/USD cap (default ~$0.50) with a themed "Snagged — ran out of river" state when hit, and always show estimated cost before a Standard/Deep scan. Alternative: no cap (bad for non-technical users). |
| 4 | **Band thresholds** | Proposed default (vetoable) | By *score quantiles over the candidate list* (top 15% of scores → Driftwood, bottom 15% → Source), with absolute fallbacks when the list is small (<20 candidates: use fixed score cutoffs, e.g. ≥75 / ≤25). Bytes-based banding distorts because one huge cache skews everything. |
| 5 | **Deep tier folder listing** | Proposed default (vetoable) | Top-level entry names + extensions only, depth 1, capped at 100 entries (then "+N more"). Names + sizes gets token-expensive and sizes rarely change the judgment. |

Section 13's remaining items (weight tuning, model selection, threshold tuning) are dogfooding concerns — Phase 2 builds the CLI harness specifically to make tuning cheap.

---

## 1. Architecture

```
driftwood/
├── crates/
│   ├── driftwood-core/        # ALL logic; no UI, no Tauri, no MCP deps
│   │   ├── scan/              # Stage 1: scope walking, Spotlight, hard rules
│   │   ├── score/             # drift score heuristics + banding
│   │   ├── reason/            # Stage 2: OpenRouter client, batching, prompts
│   │   ├── rules/             # local rule engine, memory folder, distiller
│   │   └── report/            # report model, grouping, serialization
│   ├── driftwood-mcp/         # thin binary: rmcp server over core
│   └── driftwood-cli/         # thin binary: headless scan for dogfooding/tuning
├── app/                       # Tauri app
│   ├── src-tauri/             # thin wrapper: Tauri commands → core, event streaming
│   └── src/                   # Svelte frontend: onboarding, scan, report, theme
└── docs/
```

Rules of the architecture:
- `driftwood-core` is the product. App and MCP server must contain **zero** scan/reasoning logic — only transport, presentation, and permission plumbing.
- The core exposes two async entry points: `run_scan(config, event_sink)` and `apply_correction(...)`. Both wrappers call only these.
- Everything the UI shows comes from a serialized `Report` model; the frontend never reimplements grouping/scoring.

---

## 2. Data model (core types)

**Candidate** (Stage 1 output): `{ id, path, kind: file|folder|app, size_bytes, kind_stats, last_used_date (Spotlight, optional), modified_date, orphan_status: orphaned|unknown|active, scope_category, score, score_components {size, age, cache_loc, orphan, depth, file_type, child_count}, band: high|middle|low }`

**ReportEntry** (final): `{ candidate, tier: 1..4, tier_source: auto_high|auto_low|rule|llm, summary (one sentence: what it is / why it exists), reasoning (full LLM rationale for the dropdown), confidence, llm_model, privacy_tier_used }`

**Rule** (rules.json): `{ id, ext?, folder_name?, parent_folder?, size_band?, orphan?, score_band?, tier, support_count, created_at }` — plain JSON, user-editable.

**preferences.jsonl** line: `{ timestamp, candidate_features {ext, parent_folder, size_band, orphan, score_band}, original_tier, corrected_tier }`

**ScanConfig**: scope category toggles (per Section 9 presets), privacy tier, model id, cost cap, rules snapshot.

---

## 3. Phases

### Phase 0 — Scaffolding (~half day)
- Cargo workspace + Tauri app scaffold (Svelte + Vite + TS), MIT headers, CI skeleton.
- Define the core types above and the JSON serialization in one place (`report` module) so both wrappers and tests consume identical shapes.
- `driftwood-cli` stub that prints a JSON report — this is the Phase 1–2 workhorse.

### Phase 1 — Stage 1 scanner (the biggest phase, ~1 week)
Order within the phase matters: walk → hard rules → orphans → score.

1. **Scope enumeration.** Resolve the three preset categories (Section 9) into concrete roots. Low: `~/Library/Caches`, `/tmp`, `~/Library/Logs`, orphaned `~/Library/Application Support`. Medium: `~/Downloads`, `~/Library/Containers`. High: `~/Documents`, `~/Pictures`, `~/Movies`, `~/Music`. Category is attached to every candidate — the report groups by it.
2. **Spotlight-first metadata.** Use `mdfind` per scope root with `kMDItemLastUsedDate`, `kMDItemFSSize` attributes (`mdfind -onlyin <root> ...` + `mdls` batch for gaps). Fall back to a bounded `walkdir` for Spotlight-blind paths. Record *whether* last-used came from Spotlight or was missing — this distinction drives the "still in the current" rule (Section 13 mitigation: the rule is a hard filter **only** on positive Spotlight recency; missing data never blocks candidacy).
3. **Hard rules.** Drop anything Spotlight says was used ≤ 21 days. Flag known junk roots (caches/logs/tmp) and compute orphan status.
4. **Orphan detection.** Enumerate installed apps from `/Applications` + `~/Applications` (bundle IDs + display names, including nested .app inside .app for runtime-shipped apps). A support/cache/preferences folder is *orphaned* when no installed app matches its name under normalization: reverse-DNS match (`com.vendor.app`), vendor-prefix match (`com.vendor.*` counts as *active* — protects shared vendor folders), and fuzzy name match. Explicit never-orphan list: `com.apple.*`, known shared dirs (`Containers` group containers, `MobileSync`, etc.). This list is data, not code, so dogfooding can extend it.
5. **Drift score.** Weighted 0–100 sum of the seven heuristics (Section 8). Starting weights (tunable via a TOML next to the CLI, not hardcoded):
   - size: 25 max, log₂-scaled
   - age: 25 max, days/365 capped
   - cache-location bool: 15
   - orphan status: 15
   - depth: 5 max
   - file-type histogram (`.cache/.tmp/.log` etc.): 10
   - child count (folders only): 5
6. **Banding** per Decision #4, and **app bundles as units**: score a `.app` and its support/caches as one candidate where sensible (deleting an app means its data is what matters).
7. Emit progress events throughout (files searched, bytes searched, candidates found, recoverable bytes — Section 11 counters) via a channel trait so app, CLI, and MCP all get the same feed.

**Deliverable:** `driftwood-cli scan --scope low` prints a scored candidate JSON. This alone is dogfoodable.

### Phase 2 — Tuning harness (~2–3 days, overlaps dogfooding)
- CLI flags: dump score components per candidate, override weights, force band cut-offs, diff two runs.
- A tiny "known answers" fixture set (paths you *know* are tier 1/4) to check score sanity.
- Exit criterion: on your own machine, top-band items are all things you'd happily delete and bottom-band has no embarrassing mistakes. This is where weights and thresholds get their first real values.

### Phase 3 — Stage 2 LLM reasoning (~4 days)
1. **OpenRouter client** in `reason/`: enforce `zdr: true` on every call (Section 9), plus provider routing preferences; streaming not needed (batch calls), but timeouts + 2 retries with backoff are.
2. **Batching:** sort middle band by score, chunk 20–30 contiguous (score-proximity batching). Reference candidates by opaque `id`, never by raw path in the *response* format — the model returns `{id, tier, confidence, summary, reasoning}` per item, which prevents path-mangling and simplifies parsing.
3. **Prompt sketch** (per privacy tier): system prompt explains the four river tiers, the still-in-the-current context, and the output schema; per-candidate payload is the structured metadata (Minimal strips paths/filenames and sends kind/size/dates/orphan/ext only; Standard sends full paths; Deep adds the depth-1 folder listing capped at 100 names). JSON-mode/structured output if the model supports it.
4. **Robustness:** partial-batch parse failure → retry once → degrade failed items to their heuristic band label with `tier_source: fallback` and a themed "Snagged" note, never silently. Cost accounting per batch against the cap from Decision #3. Resume: persist batch results to disk as they arrive so a crashed scan isn't lost.
5. **Report assembly:** group by scope category, per-group count + size totals, tier badges, sort within groups by score.

**Deliverable:** CLI end-to-end scan → JSON report with summaries and reasoning.

### Phase 4 — App shell + onboarding (~1 week)
- Onboarding: three preset category cards with the Section 9 defaults (low ON, medium/high OFF), per-folder toggles, cache-specific "disable all cache scanning" button, and the **Full Disk Access explanation screen** — this is the first trust hurdle (Section 13), so it gets on-theme copy: what FDA unlocks, that DriftWood is read-only and open source, that only metadata leaves the machine (and nothing at all on Minimal). Include a live "detect whether FDA is granted" check with a themed retry state.
- Scan screen: "Search the river" button over the static watercolor art; Section 11 counters ticking live; status label pipeline (below).
- **Status label system:** core emits phase events (`Enumerating → Filtering → Scoring → Wading in (metadata) → Traveling to the river (LLM) → Sorting the driftwood`) mapped to the themed labels from Section 6; each label has a disclosure arrow revealing the raw detail (LLM thinking, current query, error text). Error → "Snagged", empty → "Clear waters".
- Settings: privacy tier selector, model picker, cost cap, "manage memory" entry point (Phase 7).

### Phase 5 — Report UI: the notebook (~1 week)
- Journal/notebook presentation: findings as field-note entries on paper-textured pages, grouped by category with group headers showing item count + size. Page-flip feel via CSS scroll-snap/pagination, **not** the rejected wash-ashore animation.
- Per entry: path (truncated middle), tier badge (Driftwood / Message in a Bottle / Current / Source — the Section 4 names), one-line summary, reasoning dropdown, **Reveal in Finder** button (`open -R` via Tauri shell), tier override control (feeds Phase 7).
- Report persisted to `~/Library/Application Support/DriftWood/reports/` so the last report survives relaunch.

### Phase 6 — River animation + theme (~3–4 days, mostly polish)
- Static watercolor river asset (from the Canva illustration) as the hero; layer the three *kept* effects from Section 7 on top: animated feTurbulence+feDisplacementMap wobble, pulsing shimmer highlights, paper-grain overlay at low opacity. All effects respect `prefers-reduced-motion`.
- Reuse the paper-grain filter in the report notebook background for cohesion. Explicitly do not build the drifting-specks effect.

### Phase 7 — Preference learning (~4–5 days)
- Memory folder exactly per Section 10: `preferences.jsonl` (append-only corrections), `rules.json` (distilled), `session-log.jsonl` (audit). Written under `~/Library/Application Support/DriftWood/memory/`.
- Correction flow: user overrides a tier → extract features (ext, parent folder, size band, orphan, score band) → append → background distiller regenerates `rules.json` (frequency-based: features shared by ≥2 corrections with consistent tier become a rule; support count recorded). No LLM involved.
- Rule engine: exact/normalized feature match against candidates in Stage 1 — a matching rule **pins the tier and skips the LLM for that candidate**. Hot-reload on file change. Rules are plain JSON and user-editable.
- Few-shot injection (privacy-gated per Section 10): only middle-band candidates with no matching rule; retrieve the N most feature-similar past corrections from `preferences.jsonl`; Minimal=never, Standard/Deep=opt-in toggle.
- MCP resources prepared here (same files the server will expose).

### Phase 8 — MCP server (~3 days)
- `driftwood-mcp` binary over rmcp, stdio transport (for Claude Desktop / any client).
- Tools: `scan(scope?, privacy_tier?, dry_run?)`, `get_report()`, `reveal(path)`, `correct_tier(path, tier, note?)`.
- Resources (read-only): `memory://rules`, `memory://preferences`.
- Progress surfaced as MCP logging notifications. The server enforces the same read-only guarantee — no tool exists that can delete/move/write anything outside DriftWood's own memory folder.
- Documented `claude_desktop_config.json` snippet in the README.

### Phase 9 — Hardening + packaging (~3 days)
- Manual test checklist: FDA denied mid-scan, network drop during Stage 2, empty scopes, 100k+ file Downloads, symlink loops (walk depth cap), unreadable dirs (skip + count as "wading past"), apps running right now (exclude active apps' data via `lsof`-free heuristic: last-used recency already covers most).
- Code signing + notarization (Developer ID) since FDA prompts are scary enough unsigned — even for personal dogfooding, notarization removes one frightening dialog.
- README: privacy model, ZDR enforcement, build instructions.

---

## 4. Edge cases / failure modes to design for now

1. **Spotlight disabled or indexing off** → scanner must degrade to walkdir + mtime and *label the report's confidence accordingly* ("the river ran murky — recent-use data was incomplete").
2. **mtime unreliability** — never use mtime as "last used"; Spotlight date or nothing. Downloads folder is an exception (download date ≈ creation is meaningful).
3. **Permission errors on walk** → skip-and-count, never abort.
4. **Huge directories / child-count explosion** → cap per-dir enumeration, aggregate deeper levels into the parent candidate ("folder, 12k files, 3.2GB").
5. **.app bundles & sandbox containers** — treat as atomic units; a running app's container must never be flagged orphaned.
6. **LLM batch partially fails** → per-item fallback (Section 3.3.4), report marks `tier_source` honestly.
7. **Cost cap hit mid-scan** → finish with heuristic tiers only, themed warning, no silent truncation.
8. **Two DriftWood instances / scan cancellation** → single-flight lock file; cancel must be prompt (check the flag between batches, not mid-batch).
9. **Path ambiguity in LLM output** — solved by opaque-ID round-tripping (Phase 3.2).
10. **Never-flag list** — hardcode-banned: `~/Library/Keychains`, anything under an active profile, iOS device backups (`~/Library/Application Support/MobileSync`) always land tier 4 minimum regardless of score.

---

## 5. Testing strategy

- Unit: score components, orphan normalization (the fuzzy matcher is the riskiest pure function — table-driven tests), rule matching, banding quantiles, privacy-tier payload redaction (assert no path fragments leak in Minimal).
- Golden files: `mdfind`/`mdls` output parsing against recorded fixtures (can't hit Spotlight in CI deterministically).
- Contract: LLM client against recorded response fixtures incl. malformed-JSON cases.
- CLI integration on real $HOME is the actual dogfooding loop — Phase 2 exists to make it cheap.

---

## 6. Why this ordering

Stage 1 + tuning harness come before any UI because **the notes' biggest unknowns (weights, thresholds, orphan false positives) are all Stage 1 concerns**, and the CLI is the fastest way to iterate on them. The LLM stage lands before UI so the report content model is real when the notebook UI is built — no mock data passes through. Theme/animation (Phase 6) is deliberately after the report UI works: polish on top of functioning screens, never blocking them. MCP last because it's a thin wrap that benefits from the core API being stable.
