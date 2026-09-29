# DriftWood: reasoning speed, live progress/cost meters, honest cancel, and the in-app Finder

## Goal

DriftWood's Stage 2 (LLM reasoning) is unusably slow — a 807-item middle band takes ~5 minutes
because batches run strictly serially — and the UI gives the user zero feedback during that time:
no progress bar, no ETA, no live cost, no speed readout. Meanwhile the "Reveal N Driftwood in
Finder" button fires one `open -R` per parent directory, so a large report pops 40 Finder windows
in sequence and reads as a compromise. And the user has no in-app way to see their findings as an
organized list.

Ship, as one coherent release:

1. A fast reasoning pipeline (concurrent batches, larger batches, cluster dedup).
2. Streaming responses feeding a live progress bar, ETA, items/sec "speed" readout, and live cost
   meter during Stage 2.
3. Cancellation that actually aborts the in-flight request and returns the partial report instead
   of discarding the scan.
4. A single-window reveal that never mass-opens Finder.
5. A built-in, in-app "Finder" — a virtual, read-only browser over the report organized by tier
   and cross-cut by personal risk — from which the user selects items and hands them off to the
   real Finder to delete themselves.

## Context

Monorepo: Rust workspace (`crates/driftwood-core` is the engine, also consumed by
`crates/driftwood-cli` and `crates/driftwood-mcp`) + a Tauri v2 / Svelte 5 frontend in `app/`
(`app/src`, Rust shell at `app/src-tauri/src/lib.rs`).

### The trust moat — non-negotiable, and it survives this release

The product promise is that **DriftWood never deletes anything**. It is stated in
`app/src/screens/Welcome.svelte` ("Read-only, always. It never deletes a thing"), the Full Disk
Access trust screen, and `app/src/screens/Report.svelte:189` ("DriftWood deleted nothing. It never
does."). `driftwood-mcp` is designed around the same guarantee — it exposes no tool that can
delete, move, or write anything outside DriftWood's own memory folder.

The in-app Finder built here is a **read-only browser**. It does not delete, move, or trash
anything. It hands the user's selection off to macOS Finder, where the user performs the deletion
themselves with Cmd+Delete. **Do not add any code path that unlinks, trashes, moves, or modifies
a scanned path.** If in doubt, do not build it. The copy may be *reframed* to say DriftWood shows
you exactly what to clear, but the literal claim "it never deletes" must remain true.

### Current pipeline (read these before changing them)

- `crates/driftwood-core/src/engine.rs::run_scan` — the single async entry point. Phases:
  Enumerating → Wading (Spotlight/metadata) → Filtering (recency + orphans) → Sizing (concurrent
  `spawn_blocking`, max 8) → Scoring + banding → Stage 2 → assemble entries.
- `crates/driftwood-core/src/reason/mod.rs::reason_middle_band_with` — Stage 2. Batches the middle
  band by score proximity, `batch_size` from config, and loops
  `for chunk_start in (0..sorted.len()).step_by(batch_size)` **serially**, awaiting each batch
  before starting the next. Cancellation is checked only at the top of that loop. The cost cap is
  checked only at the top of that loop against `total_cost` (actual spend so far).
- `crates/driftwood-core/src/reason/client.rs` — the `Transport` trait
  (`post_json(url, headers, body) -> Result<(u16, String)>`), `build_request_body`, and
  `parse_chat_response` (reads `usage.cost` / `usage.total_cost`, `prompt_tokens`,
  `completion_tokens`). Cost and token counts are only knowable **after** a call returns today.
- `reason/mod.rs::ReqwestTransport` implements `Transport` with
  `tokio::task::block_in_place` + `Handle::block_on` around a **blocking** `reqwest` call. This
  makes the in-flight request impossible to abort and serializes the runtime.
- `crates/driftwood-core/src/config.rs` — `Reasoning` defaults: `batch_size: 25`,
  `cost_cap_usd: 0.50`, `timeout_secs: 120`, `retries: 2`, `few_shot: false`, `few_shot_count: 5`.
  `DriftTuning` is the single knob bag and is `from_toml_str`-loadable.
- `crates/driftwood-core/src/engine.rs` already implements the "folder-type auto-High" rule
  (`is_definitionally_driftwood`, line ~712) which keeps cache/log roots out of Stage 2 entirely.
- `crates/driftwood-core/src/events.rs` — `Phase` enum and `ScanEvent` enum
  (`Phase`/`FilesSearched`/`BytesSearched`/`CandidatesFound`/`RecoverableBytes`/`Notice`/`Warn`/
  `Error`), and the `EventSink` trait. This is the progress channel to both wrappers.
- `crates/driftwood-core/src/types.rs` — `Tier` (1 Driftwood, 2 Message in a Bottle, 3 Current,
  4 Source), `TierSource`, `ScopeCategory` (low/medium/high), `Candidate`, `PrivacyTier`.
- `app/src/lib/types.ts` is a hand-maintained mirror of `types.rs` — the wire contract. Field
  names must stay identical. `TIER_NAMES` and `TIER_BLURBS` already exist.
- `app/src/lib/bridge.ts` — the single seam between frontend and world. `Bridge` interface with
  `TauriBridge` and `MockBridge` implementations. Every new command needs **both**, or `npm run dev`
  breaks and the screens become unreviewable in the browser.
- `app/src/lib/stores.ts` — `scan` store with `applyEvent` switch, `ScanState` shape.
- `app/src-tauri/src/lib.rs` — `TauriSink` bridges core `EventSink` to the `scan-event` window
  event; `reveal_path` / `reveal_paths` (line 44-72) run `open -R` once per parent group.
- `app/src/lib/mock.ts` — scripted event timeline for the mock bridge. It must be updated to
  exercise the new events or the new UI can't be reviewed in the browser.

### Verified diagnosis of the slowness

807 middle-band items ÷ `batch_size: 25` = **33 strictly serial round trips**. The model choice is a
minor contributor. Three compounding fixes are specified below.

Note: `newideas.md` in the repo root is a scratchpad that lists these same ideas with older
numbers (banding defaults have since been widened to 0.25/0.25, and the auto-High rule has landed).
Treat it as context, not as a spec.

## Requirements

### 1. Reasoning pipeline speed

**a. Make the transport async and streaming.** Replace the `block_in_place` + blocking-reqwest
`ReqwestTransport` with a genuinely async `reqwest` client. Evolve the `Transport` trait so a
streamed call can report progress and, on request, be aborted mid-flight. The trait exists so
contract tests can replay recorded fixtures — keep that property, and keep the fixture-based tests
in `client.rs` and `reason/mod.rs` passing. Do not break the `OkTransport` / `FailTransport` test
doubles; extend them for streaming rather than deleting them.

**b. Stream from OpenRouter.** Send `"stream": true` and
`"stream_options": {"include_usage": true}` in `build_request_body`. Parse SSE `data:` lines,
accumulating `choices[0].delta.content`, and take the final usage chunk for `cost` / token counts.
The accumulated content must still be fed to the existing tolerant `parse_judgments` (it handles
bare arrays, `{"items": [...]}` wrappers, and code fences — keep all of that, real models
intermittently break the schema and retrying costs more than salvaging).

**c. Run batches concurrently.** Replace the serial `step_by` loop with a bounded-concurrency
fan-out over batches (4–6 in flight; put it in `Reasoning` config as a tunable, e.g.
`max_concurrent_batches`). Results must be collected per batch and merged into the same
`judgments` / `fallback_ids` maps. Keep the per-batch retry-and-backoff behavior (currently
`cfg.retries` with 500ms × 2ⁿ backoff, and 4xx-except-429 breaks immediately) — it moves inside
each concurrent task. Use `tokio::task::JoinSet` or `futures::stream::buffer_unordered`.

**d. Raise `batch_size`.** Default `Reasoning::batch_size` from 25 to 60–100 (pick a value and
state the reasoning in a comment). The middle band is structured JSON classification with no
required reasoning trace; the old 20–30 sizing was tuned for expensive frontier models. Clamp
bounds stay in place (currently `clamp(1, 100)`).

**e. Cluster dedup.** Before batching, group the middle band by
`(parent_dir, orphan_status, cache_like_ratio bucket)` and send **one representative** per cluster
to the model; propagate the returned judgment to its siblings. (This is idea D from
`newideas.md`, never implemented. `~/Library/Logs/DiagnosticReports/*` alone is ~120 items and
`~/Library/Caches/com.google.Chrome/*` ~47.) Batching then operates on representatives.

**Honesty requirement for (e):** propagated judgments must NOT be labeled `TierSource::Llm`. Add
a distinct `TierSource` variant (e.g. `LlmPropagated`) that the report surfaces as its own
category, so 120 folders sharing one judgment are never presented as 120 independent judgments.
Mirror the new variant in `app/src/lib/types.ts`'s `TierSource` union and give it a distinct
summary string. Same discipline the existing `fallback` tier source already follows.

**f. Keep ZDR enforcement as-is, but surface the tradeoff.** `client.rs` currently either enforces
`provider.zdr = true` + `data_collection: deny` or omits the block entirely — "no middle ground by
design." Preserve that. Note that the fast models the user is drawn to (e.g. `gpt-oss-safeguard-20b`)
are typically **not** ZDR, so using them requires `allow_non_zdr: true`, which on Standard privacy
tier means real file paths and folder names can be processed by providers that retain them. Make
that consequence visible in the Settings UI when a non-ZDR model is selected, rather than leaving
it as a silent consequence of a model dropdown.

### 2. Progress bar, ETA, and speed readout

Add new `ScanEvent` variants in `crates/driftwood-core/src/events.rs` and mirror them in
`app/src/lib/types.ts` + the `scan.applyEvent` switch in `stores.ts`. At minimum, a Stage 2
progress event carrying `{ judged, total, cost_usd, prompt_tokens, completion_tokens }`, plus a
"batch started / batch finished" signal so the UI can distinguish work in flight from work done.

Rules for honesty — a fake global progress bar is worse than none:

- **Only Stage 2 has a knowable denominator.** `middle.len()` is known before reasoning starts, so
  a true percentage and a real ETA are valid during `Phase::Reasoning` and nowhere else.
- The walk/sizing phases have no denominator (you cannot know how many files are under
  `~/Library/Caches` until you finish walking it). Those phases show an indeterminate indicator
  with the existing live counts (`FilesSearched`, `BytesSearched`, `CandidatesFound`) — do not
  render a percentage for them.
- ETA uses an EMA (not a naive mean) of observed items/sec, and should visibly settle rather than
  swing wildly on the first two batches. Once dedup is in play the ETA denominator is the
  representative count, not the raw middle-band count.

**Speed readout:** the headline number is **items/sec**, labeled as a plain-language "speed" (the
user explicitly asked for a label non-technical people can read; TPS is not that). Raw
tokens/sec goes into the existing disclosure feed (`Notice` events) under a technical label, not
into the headline counter. Place it alongside the existing `Counter` row in
`app/src/screens/Scan.svelte` (`Files searched` / `River walked` / `Recoverable so far`).

### 3. Live cost meter

- Display **actual cost so far** plus a **projected total** (`cost_so_far / batches_done *
  batches_remaining`).
- Change the cost cap enforcement from checking actual spend at the top of the loop
  (`total_cost >= cost_cap_usd`) to checking the **projected** cost before dispatching each batch.
  Today a single expensive batch can overshoot the cap arbitrarily; the user asked for the ability
  to "pull the plug if it gets too expensive" and that requires the cap to be a real ceiling, not a
  suggestion.
- Show the cap as a visible ceiling in the UI so the user can see headroom, not just a running
  number that might be about to breach.
- `format_cost` (`reason/mod.rs`) already exists for report notices — reuse it.

### 4. Honest cancellation

Two defects to fix together:

- **Cancellation cannot abort in-flight work.** With the new async transport, select on the cancel
  flag inside the streaming loop and drop the request future, so "Pull ashore" takes effect
  immediately instead of after the current batch. Keep the existing between-batch check as a
  backstop.
- **Cancelling discards the whole report.** `run_scan` currently returns
  `Err(DriftError::Cancelled)` and the entire result is lost — ten minutes of work produces nothing
  and the user must start over. Change this so cancel produces the **partial report**, with every
  unjudged middle-band item marked `TierSource::Fallback` — the same shape the cost-cap path
  already produces at `engine.rs` (~line 655). The frontend (`Scan.svelte`'s `start()`) currently
  special-cases the `"cancelled"` error string and shows a toast; that path must be updated to
  render the partial report instead. A completed-with-partial-result scan should be visually
  distinguishable from a full one, with copy explaining the scan stopped early.
- UX: the button should acknowledge immediately (disabled/"stopping…" state) rather than appearing
  dead while a batch finishes.

### 5. Single-window reveal

`reveal_paths` in `app/src-tauri/src/lib.rs` (lines 53–72) groups by parent and runs one
`open -R` per group, so a large report opens dozens of Finder windows in sequence. Change it to:

- Group by parent directory, cap the number of Finder windows opened (choose a cap; ~6 is
  reasonable), and open the remaining groups in prioritized order (largest group first).
- **Tell the user** how many windows will open and how many items were included, in a toast —
  never silently open 40 windows.
- Never invoke `open` more than once per reveal action; batch it.

### 6. In-app Finder (read-only browser over the report)

Build a new screen (suggest `app/src/screens/Browser.svelte`, wired through `App.svelte`'s view
switch and `View` union in `stores.ts`) that presents the report as an organized, browsable list.

Requirements:

- **Virtual, not physical.** Do **not** create four folders on disk, do **not** copy the scanned
  files into them, and do **not** create symlinks. The user floated all three; copying 170 GB to
  build an index is unacceptable, and Finder follows symlinks into the real files anyway. The four
  tiers exist only as sections in the app.
- **Organization:** sections by tier (Driftwood / Message in a Bottle / Current / Source, using
  the existing `TIER_NAMES`), cross-cut by personal risk (`ScopeCategory` low / medium / high, using
  the existing `SCOPE_LABELS` / `SCOPE_HINTS`). Every `Candidate` already carries both
  `tier` and `scope_category`, so no core change is needed for the data itself.
- **Selection:** multi-select with a select-all-per-section affordance, showing a live count and
  total size of the current selection.
- **Handoff:** a "Show these in Finder" action that calls the (fixed) `revealAll` with the selected
  paths. The user then presses Cmd+Delete themselves. Copy must make the hand-off explicit and
  calm — this is a handoff, not a deletion.
- **Safety framing:** `Source` (tier 4) and high-personal-risk items should be visually distinct
  and carry the existing blurbs (`TIER_BLURBS`). DriftWood still performs no deletion, so no
  confirmation gate is strictly required, but the UI should not encourage a blanket hand-off of
  tier 4.
- The existing `Report.svelte` bulk "Reveal N Driftwood in Finder" footer button should be
  re-pointed at (or replaced by) an entry into this browser, since the browser supersedes it.

### 7. Copy and mock

- Reframe, do not weaken, the promise. "DriftWood deleted nothing. It never does." stays literally
  true. Where copy describes the Finder hand-off, it should say DriftWood shows you exactly what to
  clear and hands it to Finder — not that it clears for you.
- Update `app/src/lib/mock.ts` so the scripted event timeline emits the new Stage 2 progress events
  and the mock bridge supports any new commands. Both `TauriBridge` and `MockBridge` in
  `app/src/lib/bridge.ts` must implement every new `Bridge` method, so `npm run dev` in a plain
  browser remains a full review environment for the new screens.

## Edge cases and considerations

- **Cancel during a stream** must not leave a half-written `batches-<scan_id>.jsonl` resume file
  with unparseable lines. `persist_batch` appends one JSON object per line; a cancelled stream
  should persist nothing for that batch, or the resume loader
  (`reason/mod.rs`, the `PersistedJudgment` loop) must tolerate and skip a truncated trailing line.
- **Cost is only finalized at the end of a streamed call.** The live meter must therefore be
  labeled as actual-so-far, not as a running total that will only grow. Don't imply precision that
  doesn't exist yet.
- **A cancelled or partial scan must not report a cost-cap hit**, and vice versa. The
  `cost_cap_hit` flag and the new "stopped early" state are distinct conditions; the report should
  distinguish them.
- **OpenRouter returns 200 with an `error` object** in the body (already handled in
  `parse_chat_response`). A streamed error can also arrive mid-stream as an `error` chunk before
  any content — handle it as a batch failure with the same retry semantics, not a parse panic.
- **Streaming can be interrupted mid-JSON.** Accumulated content that fails `parse_judgments` after
  retries should fall the whole batch to `fallback`, same as today. Consider one salvage attempt
  that repairs a truncated trailing object, but never let a partial parse silently drop candidates —
  anything unanswered must land in `fallback_ids`, which is the existing safety net.
- **ZDR + streaming:** confirm the `provider.zdr` block is still honored on streamed calls; don't
  let the streaming change quietly drop the privacy routing.
- **Cluster propagation must not cross safety boundaries.** A cluster is only propagated within an
  identical `(parent, orphan_status, cache_like_ratio bucket)` group, and never to
  `never_flagged` or rule-pinned candidates (both are resolved before Stage 2 and are excluded from
  `middle` already — keep it that way).
- **Large reports.** A report with thousands of entries needs the browser's list to be virtualized
  or windowed; naive `{#each}` over thousands of rows will be unusable. `Report.svelte` currently
  renders every entry, so this is a real constraint, not hypothetical.
- **Window cap on reveal** must be surfaced to the user in the toast, and the overflow must be
  predictable (largest groups first) rather than arbitrary.
- Preserve the existing single-flight `ScanLock` behavior and the `Phase::Assembling` phase, which
  `run_scan` currently never emits.

## Ask

Implement the goal. Work through it in this order, verifying as you go:

1. **Async streaming transport** — refactor `ReqwestTransport` off `block_in_place`/blocking
   reqwest, extend the `Transport` trait for streaming + abort, add `stream` /
   `stream_options` to `build_request_body`, implement SSE parsing that accumulates content and
   harvests final usage. Keep the fixture-based transport tests working.
2. **Concurrent batches + new `batch_size` default** — bounded fan-out with per-batch retries,
   merged into the existing judgment/fallback maps.
3. **Cluster dedup + the new `TierSource` variant** — including the `types.ts` mirror and distinct
   report copy, so propagated judgments are never presented as independent LLM judgments.
4. **New `ScanEvent` variants** (core + `types.ts` mirror + `stores.ts` reducer) carrying judged /
   total / cost / tokens, plus a batch-in-flight signal.
5. **Cancel: abortable, and returns the partial report** — including the `Scan.svelte` error path
   and the distinct partial-result presentation.
6. **Projected-cost cap enforcement.**
7. **Scan screen UI** — Stage 2-only determinate progress bar with EMA-based ETA, headline
   items/sec speed readout, and the live cost meter with visible cap headroom. Indeterminate
   indicator elsewhere.
8. **Reveal fix** — single invocation, window cap, prioritized grouping, user-visible count.
9. **In-app Finder screen** — virtual tier sections cross-cut by personal risk, multi-select with
   live count/size, Finder hand-off. Wired into `App.svelte` / `View` and superseding the bulk
   reveal button in `Report.svelte`.
10. **Settings** — surface the ZDR consequence when a non-ZDR model is selected.
11. **Mocks and copy** — `mock.ts` event script, `MockBridge` parity, reframed (not weakened) trust
    copy.

Run `cargo test` for the Rust workspace and the frontend's typecheck/build. The existing test
suites in `reason/mod.rs` and `client.rs` (happy path, failure→fallback, cost cap stops batches,
partial ids fall back, cancellation, tier mapping, request-body ZDR shape, tolerant judgment
parsing) must continue to pass — extend them for streaming and concurrency rather than weakening or
removing them. If a design decision is genuinely ambiguous, make the conservative choice that
preserves the never-deletes guarantee and say so explicitly in the summary.
