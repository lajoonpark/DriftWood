# DriftWood: honest tiers — argued auto-high, a vendor floor, on-demand adjudication, and a Deep read mode

## Goal

DriftWood currently tells the user something is **"no risk at all — safe to clear"** about folders it
never reasoned about, and gives them no way in the app to check. On this machine
`~/Library/Caches/com.apple.HomeKit` came back as Tier 1 at confidence `1.0` — which
`confidenceWord()` renders as the word **"certain"** — with an empty reasoning string and therefore
no "Why the river says so" dropdown at all. The tier was arguably fine; the *assertion* was not, and
it was unfalsifiable from inside the app.

Ship four changes, in this order, that together make every tier the user sees either **argued** or
**visibly labeled as a heuristic guess**:

1. **On-demand adjudication** — "ask the river about this one," for any entry, at any tier.
2. **Argued auto-high** — auto-high carries a real, deterministic explanation and stops rendering a
   confidence word.
3. **A system/vendor floor** — OS-owned paths are never auto-labeled Driftwood.
4. **A Deep read mode** — a third scan mode in which bands are advisory and every surviving
   candidate is argued.

Items 3 and 4 are explicitly downstream of 1 and 2. Deep read before argued auto-high is a strictly
larger bill for the same unargued assertions, and a floor that lives anywhere other than the
assembly loop is a floor Deep read can bypass. Do not reorder.

## The trust moat — unchanged, and the reason this release exists

DriftWood never deletes, moves, or modifies anything. That claim is stated in
`app/src/screens/Welcome.svelte`, the Full Disk Access screen, and `app/src/screens/Report.svelte`
("DriftWood deleted nothing. It never does."). Nothing in this document adds a code path that
unlinks, trashes, or writes a scanned path. Adjudication calls the LLM; it does not touch the disk.

What this release changes is the *other* promise — that a tier stamp means what it says. Today, for
roughly a third of Tier 1, it does not.

## Root cause, with locations

`~/Library/Caches/com.apple.HomeKit` reached Tier 1 like this:

1. `crates/driftwood-core/src/scan/scope.rs:36-49` — `resolve_roots` puts `~/Library/Caches` in the
   Low category with `cache_like: true`.
2. `crates/driftwood-core/src/scan/orphan.rs:39` — `DEFAULT_NEVER_ORPHAN` contains `com.apple.*`, so
   `classify_orphan` returns `OrphanStatus::Active` for it (`orphan.rs:198-200`). The engine *knows*
   this is a live, OS-managed folder.
3. `crates/driftwood-core/src/engine.rs:740-753` — `is_definitionally_driftwood` returns `true` for
   **any** path under `~/Library/Caches`, `~/Library/Logs`, `/tmp`, `/private/tmp`. It consults
   `orphan_status` only on the Application Support branch (`engine.rs:751-752`); the cache-root
   branch at `engine.rs:747-749` ignores it entirely. An `Active` folder is promoted regardless.
4. `crates/driftwood-core/src/engine.rs:479-484` — that promotion sets `band = Band::High`.
5. `crates/driftwood-core/src/engine.rs:607-615` — the `Band::High` arm emits:

   ```rust
   Tier::Driftwood, TierSource::AutoHigh,
   "High drift score — clear driftwood, safe to clear.",   // summary
   String::new(),                                           // reasoning
   1.0,                                                     // confidence
   ```

Three separate defects compound, and all three are in the presentation layer as much as the engine:

- **The claim.** "safe to clear" and "no risk at all" (`TIER_BLURBS[1]`) are safety assertions. A
  quantile threshold and a path-prefix test do not support them.
- **The confidence.** `types.rs:195-196` documents `confidence` as *"0..1 from the LLM; 1.0 for
  deterministic assignments."* `1.0` here means "certain the rule fired," not "certain this is
  safe" — but `app/src/lib/format.ts:42-48` maps `1.0` to the word "certain," and
  `app/src/components/Entry.svelte:92` renders it next to the tier. The schema has no way to say
  "I know how I decided; I don't know whether the decision is right."
- **The invisibility.** `app/src/components/Entry.svelte:70-84` gates the entire "Why the river says
  so" affordance on `{#if entry.reasoning}`. Auto-high has empty reasoning, so the button does not
  exist. The provenance phrase *is* shown — `auto_high: "scored straight to driftwood"`
  (`Entry.svelte:22`) — but at 12.5px `var(--ink-faint)`, visually subordinate to a 14.5px
  `var(--ink)` summary and a 17.5px tier name. The hedge is the quietest text on the card.

Note the existing tension: the auto-high rule was added to keep ~800 obvious cache folders out of
Stage 2 and cut the bill. It solved cost by making Tier 1 unargued. This release gives the argument
back without giving back the cost.

## The principle

**The burden of proof should scale with how destructive the claim is.**

An unargued "keep this" is harmless. An unargued "safe to delete" is the only thing this app can do
that can actually hurt someone. So the fix is *asymmetric* — and that asymmetry is why this does not
need the LLM on everything:

- `auto_low` → Source: stays cheap, stays unargued, keeps `confidence: 1.0`. Fine.
- `auto_high` → Driftwood: must carry a real explanation, or must not make a safety claim.

`never_flag` and `rule` keep `confidence: 1.0` — those are deterministic *pins*, not judgments, and
1.0 is accurate for them. The field's semantics do not need redefining; only one source is
dishonest.

## Decisions already made

Do not re-litigate these; they were settled in design review.

| Decision | Choice |
|---|---|
| Vendor floor strength | Hard floor at Tier 3. A user rule can still override it. |
| Adjudication result | Shown as a second opinion. The user re-stamps; nothing is auto-applied. |
| Mode representation | Three-way enum replacing the `stage2: bool`. |
| Floor scope | Apple + Microsoft + other OS vendors. Not merely "installed apps." |
| Third mode name | "Deep read." ("Full Current" collides with Tier 3 "Current".) |

### Consequences of those choices, stated plainly

- **The floor costs real headline bytes.** Measured on the dev machine: ~829 MB of `com.apple.*`
  under `~/Library/Caches` (`helpd` 64M, `python` 60M, `Music` 37M, `e5rt` 28M, …) plus ~49 MB of
  `com.microsoft.*`. Much of it is genuinely junk — `com.apple.python` is `__pycache__` — and all of
  it leaves the "recoverable" figure once floored. This is the accepted cost.
- **The escape hatch is the mechanism the user already chose.** A user re-stamping
  `com.apple.python` to Driftwood writes to `preferences.jsonl`; `rules::distill` turns it into a
  rule; the rule beats the floor. No new machinery is needed for exceptions. This is why "hard floor,
  user rules still win" was the right answer and not a compromise.
- **A floored item must not be sent to the LLM.** It is pinned at 3; the model can only confirm 3 or
  push to 4. Paying for a foregone conclusion across 880 MB of vendor caches is waste. Floored items
  resolve before Stage 2, exactly like `never_flagged`.

## Requirements

### 1. On-demand adjudication

The cheapest and highest-trust item, and the one that most directly answers "I don't trust this
one." Ship it first.

- New core entry point alongside `run_scan` / `apply_correction`. Signature should take an opaque
  candidate id and resolve the candidate from the persisted report, mirroring how
  `app/src-tauri/src/lib.rs:255` `correct_tier` already does it — the frontend sends an id, never a
  path.
- Build a single-candidate payload with `reason::payload::build_payload` under the scan's privacy
  tier, and run one batch through the existing `reason::reason_middle_band_with` machinery
  (`reason/mod.rs:322`) with `batch_size` forced to 1. Do not write a second HTTP path; the ZDR
  enforcement, streaming, retry, and cost-accounting logic in `client.rs` should be reused verbatim.
- **The system prompt must be told to argue against the card.** `reason/prompt.rs:9` currently has no
  adversarial instruction. For adjudication specifically, prepend an instruction that the model must
  state the strongest case *for keeping* the item, and that it should raise its tier when the
  strongest counterargument is strong. Without this the model tends to ratify the existing stamp.
- New Tauri command + both `Bridge` implementations (`app/src/lib/bridge.ts` — `TauriBridge` **and**
  `MockBridge`; a new method on one without the other breaks `npm run dev`).
- UI: a quiet control on `Entry.svelte`, available on **every** entry regardless of tier or
  `tier_source` — including `auto_high`, `rule`, and `never_flag`, which today have no reasoning and
  therefore no dropdown to hang it on.
- **Result presentation is the whole point.** It must be visibly a *second opinion that disagrees*,
  not a silent overwrite:
  - The card's tier and stamp do not change.
  - The reasoning renders in the existing dropdown area, under its own heading, with a distinct
    `TierSource` (see §2) and a plain statement of the disagreement ("The river says Current. This
    card says Driftwood.").
  - **If the LLM's tier is safer than the card's, say so explicitly.** A card that says Driftwood
    while the second opinion says Source is the exact failure this release exists to prevent, and
    the user must not have to infer it from two paragraphs of prose.
  - Adjudication cost counts toward the report's total. A report that says "$0.12 spent" when the
    true figure including adjudications is "$0.13" is exactly the dishonesty being fixed.
- **Never write to the memory folder.** Only an explicit user re-stamp writes to
  `preferences.jsonl`, via the existing `apply_correction`. Accepting the model's opinion is a
  conversation, not a user decision.

### 2. Argued auto-high

Zero API cost, and it fixes the presentation defects above.

- **Record why the auto-high fired.** `is_definitionally_driftwood` is currently evaluated at
  `engine.rs:480` and its result is consumed immediately to set `band`; the reason is then lost. Add
  a field to `Candidate` (`types.rs:141`) recording the promotion basis — cache root, orphaned
  Application Support, or quantile band — and the data the explanation needs. Without this, §2's
  explanation has nothing true to say.
- **Populate `reasoning` for `auto_high` from that recorded data**, not from an LLM. It must name
  what actually happened: which rule fired, the score components that put it in the top band, the
  orphan status, and the root it sits under. This is a *true* explanation, and it is what makes the
  existing `{#if entry.reasoning}` affordance appear for free.
- **Strip the safety claim from the summary.** `"High drift score — clear driftwood, safe to clear."`
  is not supportable by a path-prefix test. Say what is true: the location is a cache root, the score
  is high, and the app did not reason about it. The card should read as *unverified*, not *cleared*.
- **Stop rendering `confidenceWord` for `auto_high`.** `Entry.svelte:92` should show that word only
  for `tier_source` values where it means something (`llm`, `llm_propagated`). Keep the numeric
  field; fix the display. Do not redefine the field.
- Add a distinct `TierSource::ArguedAutoHigh` (or keep `AutoHigh` and change the UI) — whichever is
  chosen, a Tier 1 card must be distinguishable at a glance from an LLM-judged one. Mirror the
  variant in `app/src/lib/types.ts` and give it its own phrase in `SOURCE_PHRASES`
  (`Entry.svelte:21-29`).
- Consider raising the visual weight of the provenance phrase for unargued sources, or dimming the
  tier stamp, so a heuristic verdict never outranks its own caveat. Design's call, but the asymmetry
  in §"The principle" must remain legible at a glance.

### 3. System/vendor floor

- **One list, two consumers.** `orphan.rs:38-56` `DEFAULT_NEVER_ORPHAN` already encodes
  "system-owned" and is already documented as *"Data, not code: the memory folder may extend this
  via `never-orphan.json`"* (`orphan.rs:162-170`). Extract the vendor/system-owner entries into a
  single shared list consumed by **both** never-orphan detection and the new floor. Two parallel
  lists will drift: someone adds a vendor to `never-orphan.json`, orphan detection respects it, the
  floor does not, and the confident-wrong-answer class is back.
- **Document *why* the list is what it is**, in the code comment and in a header on the memory-folder
  file. The criterion is not "installed" and not "big vendor" — it is **"the vendor owns the OS or
  the whole suite, so this folder may be load-bearing for things outside that one application."**
  That sentence is what lets the next person to touch the file distinguish system-owned from merely
  installed. Microsoft qualifies (Office/Teams/OneDrive share state); an ordinary App Store app does
  not.
- **The floor resolves in the assembly loop, not at banding time.** It must beat auto-high *and* the
  LLM, so it belongs in `engine.rs:586-674` alongside `never_flagged` and `pinned`. **Precedence:
  user rule > floor > auto-high / LLM / fallback.** Do not put the primary check in
  `is_definitionally_driftwood` (a banding-time function Deep read can route around) — that fix is
  insufficient on its own, though landing it too is harmless and cheap.
- New `TierSource::SystemFloor`, mirrored in `types.ts`, with its own `SOURCE_PHRASES` entry.
- **Non-empty, honest `reasoning`**, or this reproduces the exact bug it fixes. It must say what it
  is:

  > System-managed folder (`com.apple.*`). DriftWood won't call this safe to clear.

  It must **not** say the item is "still in the flow, costly to reacquire." `TIER_BLURBS[3]` is
  about *cost to reacquire*, and 880 MB of vendor caches dumped into Tier 3 will read as "expensive
  to get back" when the truth is "we are declining to opine." Different claim; the copy carries it.
- Add the floored-item count and bytes to the scan notices (`ScanEvent::Notice`) so the drop in the
  recoverable figure is explained during the scan, not discovered afterward.

### 4. Deep read mode

- Replace `ScanConfig::stage2: bool` (`config.rs:168-169`) with a three-value enum —
  `Express` / `Standard` / `DeepRead` — plus a helper (`runs_llm()`, `is_deep()`) so the call sites
  at `engine.rs:499`, `engine.rs:699`, and `engine.rs:714` stay readable. Mirror in
  `app/src/lib/types.ts:143-154`.
- **Mapping for existing callers.** `app/src/screens/Scan.svelte:31` computes
  `stage2: !$appSettings.expressScan` from the persisted `expressScan` boolean
  (`app/src/lib/stores.ts:69-71`, `stores.ts:51-56`). Migrate that setting to the enum:
  `expressScan: true` → `Express`, `false` → `Standard`. Keep the `localStorage` key
  (`SETTINGS_KEY`, `stores.ts:60`) and the `{...DEFAULT_SETTINGS, ...}` merge shape at `stores.ts:81`
  working, or old installs lose their settings silently. **A default-settings change that
   de-prioritizes the old `false` case into a third mode is a silent behavior change for every
   existing user** — decide and state explicitly which mode they land in.
- Check `driftwood-cli` and `driftwood-mcp` for `stage2` construction and update both.
- `Express`: no Stage 2, current behavior, `fallback` labels.
- `Standard`: current behavior, middle band only.
- `DeepRead`: bands are advisory. Every surviving candidate that is not `never_flagged`, floored, or
  rule-pinned goes to Stage 2. Bands still inform **batching order** and the progress denominator;
  they must not determine a tier.
- **`DeepRead` does not bypass the floor.** Precedence is unchanged: user rule > floor > LLM. This
  is the one thing that must hold, because Deep read is exactly where a user would expect the floor
  to matter most.
- Surface the cost implication before the user commits. On a large scan Deep read is still ~2000
  candidates; it is only affordable because `cluster_middle_band` (`reason/mod.rs:243`) collapses
  large sibling groups. Warn with a real estimate, not a generic caveat.
- On Minimal privacy, Deep read sends every candidate path with no filenames — the tier
  `payload.rs:52-54` already handles. Verify the estimate copy reflects what each privacy tier
  actually transmits.

### 5. Report honesty

- Bump `SCHEMA_VERSION` (`report/mod.rs:14`) if any serialized field changes shape. Add
  `#[serde(default)]` to any new field so previously persisted reports still load.
- Keep `tier_source` the single honest provenance channel. Every new source introduced above must
  appear in `app/src/lib/types.ts` and in `Entry.svelte`'s `SOURCE_PHRASES` with distinct copy — the
  existing `fallback` variant (`Entry.svelte:27`) already sets the precedent of flagging
  non-reasoned sources with `warn: true`.

## Edge cases

- **`DeepRead` + cost cap + cancel.** All three can truncate judgment. A floored item is resolved
  before Stage 2 and is never a `fallback` candidate; do not let a partial Deep read demote or
  re-label floored items on the way out.
- **Adjudication with no report loaded.** `correct_tier` already returns a typed error
  (`lib.rs:257-264`); adjudication must do the same rather than inventing a candidate.
- **Adjudication with no API key.** Mirror the `stage2_no_key` warning
  (`engine.rs:572-577`): the button reports why it cannot run instead of failing silently.
- **Adjudication cost must respect the per-scan cap**, or it is a hole in the cap. Decide and
  document whether a single adjudication may exceed a nearly-exhausted cap (defensible: the cap
  governs bulk scanning, not an explicit one-item user request) — and make the report total honest
  either way.
- **Adjudicating a `never_flagged` item.** Allowed — a user may want to know *why* MobileSync is
  protected. It must never be able to *change* the never-flag outcome.
- **Deep tier folder listings in Deep read.** `payload.rs:56-70` attaches a depth-1 listing at
  `PrivacyTier::Deep`. Applied to ~2000 candidates instead of ~200, that is a token and cost blowup.
  Cap it, gate it, or restrict listings to the middle band even in Deep read — and say which.
- **The floor and symlinks / non-standard locations.** The floor matches on path shape. A vendor
  folder reached by an unusual path may miss it. Accept that; do not add path canonicalization to
  chase it (the scanner already refuses to follow symlinks at the unit level, `scope.rs:121-127`).
- **`is_definitionally_driftwood` still ignores orphan status on the cache branch.** Land the cheap
  fix there too, so the *band* is also honest even before the floor runs. The band is user-visible in
  the score breakdown.
- **`com.apple.*` is already protected from orphan-classification but not from tier assignment.** If
  any other code path promotes a candidate to Tier 1, the floor must cover it. Grep for
  `Tier::Driftwood` and confirm every construction site routes through the assembly loop.

## Counter-consideration, stated so it isn't lost

This release makes DriftWood **more honest and more inspectable, and gets the tier right** — it does
not make clearing `~/Library/Caches/com.apple.HomeKit` safe. That folder is a real system folder; the
app will re-download HomeKit assets and may re-trigger pairing flows. "Better reasoned" is not
"safe," and no copy change in this document should drift toward implying otherwise. The user's
original ask was an all-files mode; the accepted design is deliberately narrower, and that narrowing
is the point.

## Ask

Implement the goal in the order given, verifying as you go. Do not start item 3 or 4 before 1 and 2
land and have been seen in a real report.

1. **Argued auto-high** — record the promotion basis on `Candidate`, populate `reasoning` from it,
   strip the safety claim from the summary, stop rendering `confidenceWord` for it, add the distinct
   `TierSource` + `types.ts` + `SOURCE_PHRASES` entry.
2. **On-demand adjudication** — core entry point resolving from the persisted report, reusing
   `reason_middle_band_with` at batch size 1, adversarial prompt, new Tauri command, both `Bridge`
   implementations, UI control on every entry, disagreement made explicit, cost counted, nothing
   written to the memory folder.
3. **Vendor floor** — extract the shared system-owner list, document the criterion, resolve in the
   assembly loop with precedence **user rule > floor > auto-high/LLM**, new `TierSource::SystemFloor`
   with honest non-empty reasoning, notice with count and bytes. Plus the cheap
   `is_definitionally_driftwood` orphan-status fix.
4. **Three-way mode enum** — `Express` / `Standard` / `DeepRead` in core and `types.ts`, settings
   migration preserving existing users' mode, CLI and MCP updated, Deep read advisory-bands with
   clustering still doing the cost work, cost warning, floor still winning.
5. **Report honesty pass** — `SCHEMA_VERSION` bump if needed, `#[serde(default)]` on new fields,
   every new `TierSource` mirrored and given distinct copy.

Run `cargo test` for the workspace and the frontend typecheck/build. Existing suites in
`reason/mod.rs` and `client.rs` (happy path, failure→fallback, cost cap, partial ids, cancellation,
tier mapping, ZDR request shape, tolerant parsing) and the `OkTransport` / `FailTransport` doubles
must continue to pass. `app/src-tauri/src/lib.rs:356` asserts the report shape matches the frontend
contract — extend it for the new `TierSource` variants rather than loosening it.

If a design decision is genuinely ambiguous, make the conservative choice that preserves the
never-deletes guarantee and the never-argue-without-saying-so guarantee, and say so in the summary.
