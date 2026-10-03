# Changelog

All notable changes to DriftWood are recorded in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the app follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

DriftWood reports on files that could be deleted; it never deletes them itself.
The Finder hand-off opens Finder with a selection preselected and the user
removes files there. That guarantee is unchanged in every release below.

> v1.0.0 through v1.4.1 were backfilled on 2026-10-03 from the repo's tags,
> commit history, and published GitHub release notes. Entries from the next
> release onward are written at release time; see `.kilo/agent/release.md`.

## [Unreleased]

### Added

- Release workflow (`.github/workflows/release.yml`): pushing a `v*` tag builds
  the arm64 macOS DMG, verifies the app version files against the tag, and opens
  a **draft** GitHub release with the DMG, its SHA-256, and curated notes.
- Full Disk Access probe at scan start. The scan can now say which
  TCC-protected folders it could not list, instead of reporting them as empty.
- One editable system-owned path list (`crates/driftwood-core/src/system_paths.rs`):
  protected home locations (keychains, mail, messages, MobileSync device
  backups, call history), Apple daemon/service directory names, and `com.apple.*`
  owner patterns. A match goes straight to Source before any model call and
  costs no tokens.

### Changed

- Metadata reads are tri-state: `known`, `unavailable`, or `error` with a
  reason. A failed size, child count, or date read is never rendered or reasoned
  about as a zero/empty value.
- Unreadable items are held at Source in code and never sent to the model.
- Folder sizing distinguishes a failed read, a partial read, and a timeout. A
  failed read is never reported as 0 B.
- Orphan detection now requires positive evidence: only a reverse-DNS folder
  name with no matching installed app counts as orphaned. A plain folder name
  with no match is `unknown`, and OS-owned folders are never orphaned.
- `last used` is only emitted when Spotlight actually recorded a use; filesystem
  timestamps are labeled as timestamps, not as usage.

## [1.4.1] - 2026-10-03

### Fixed

- **Scans start again, in all three modes.** v1.4.0 always failed with
  `invalid args 'config' for command 'start_scan': missing field 'stage2'`.
  `mode` (`express` | `standard` | `deep_read`) is now the field the command
  reads and it defaults. `stage2` is still accepted as an optional legacy
  fallback (`true` → Standard, `false` → Express); neither field present →
  Standard. The other config fields (`model`, `api_key`, `cost_cap_usd`,
  `allow_non_zdr`) are now genuinely optional.
- **"Ask the river" gives a second opinion again.** One-item batches answered as
  a bare judgment object, under an unknown wrapper key (`verdicts`, `output`),
  or with the tier as a string or integral float parsed to zero judgments and
  were silently discarded. The parser now accepts those shapes, with the tier
  still rejected outside 1–4.
- An empty-but-clean parse is now treated as a snag: it retries, and if it still
  cannot be used the model's raw answer is carried into the error.
- `adjudicate_candidate` names the failure (HTTP, parse, or an unusable answer)
  instead of emitting a bare refusal.
- A one-item batch returning a single judgment under a slightly mangled id is
  kept, with a scan notice, rather than reporting silence.

## [1.4.0] - 2026-10-02

### Added

- **"Ask the river" on every card.** Request a second opinion on any finding at
  any tier. The model is told to argue *against* the card; the result appears as
  "Second opinion — argued on request, not applied." The card's tier changes only
  on an explicit re-stamp. The cost is added to the report total.
- **Deep read mode.** A third scan mode where quantile bands set batching order
  only and cannot produce a tier; every remaining eligible finding is argued. The
  mode picker warns with an estimate, and the cost cap still applies. CLI gained
  `--deep-read`; the MCP server gained a `deep_read` flag.
- **Hand-off previews before commit.** The Finder screen shows how many folders a
  selection lives in, and each container's findings count, total items, and
  reclaimable bytes. Per-folder hand-offs are supported.
- **Finder Automation consent step.** Onboarding now asks for the macOS
  Automation permission with a deep link to System Settings, instead of a dead
  control and a raw osascript error.

### Changed

- **The Finder hand-off groups by folder and verifies what Finder did.** The
  selection is collapsed to its containing folders (rolled up to ≤12 containers
  when many, never into a root, `$HOME`, a finding, or across a tainted
  container), one Finder window per container, and the selection count is read
  back out of Finder. The reported number is Finder's readback, and a mismatch is
  shown as a mismatch. On the dev machine's last real report: 347 findings → 4
  windows.
- **The UI states when ⌘A would reach past your findings.** Every container shows
  findings-versus-total, and folders holding any Source-tier item are marked and
  never merged by the rollup.
- **Auto-high argues itself and stops claiming safety.** It records which rule
  fired (cache root, orphaned Application Support, or top quantile) and emits a
  true explanation. The "safe to clear" claim is gone, and the confidence word is
  no longer rendered for heuristic stamps.
- **Three scan modes replace the Express/Standard toggle.** Existing settings
  migrate on the same key (`true` → Express, `false` → Standard). Report schema is
  now version 2; old reports still load.
- **"Could be freed" counts tiers 1–2 only.** The all-findings total is still
  shown, on its own line and explicitly labeled.
- **Unargued items are no longer dressed as failure.** Express and no-API-key
  items are labeled "the river was never asked" rather than "snagged".
- **System/vendor floor.** `com.apple.*` and `com.microsoft.*` folders are held
  at Current. Measured on the dev machine this removes ~880 MB from the
  could-be-freed headline. Precedence everywhere: your rule > the floor >
  auto-high / the AI / the heuristic.

### Fixed

- An active app's cache folder could be called safe to clear; the
  definitionally-driftwood cache branch now respects orphan status.
- Root enumeration, folder sizing, and every Spotlight helper (`mdutil`,
  `mdfind`, `mdls`) now run under timeouts. Unresponsive subtrees degrade to a
  warning and a skip; unmeasurable folders are marked truncated, never 0, and the
  scan stops dispatching after a bounded number of timeouts.
- A failed or timed-out `mdfind` now returns "no data" instead of an empty set,
  which used to read as a positive "nothing recent" signal.
- The report screen could render blank on a cold start; the last persisted report
  is now loaded into the store at launch, without clobbering a fresh scan.
- The recoverable-bytes meter takes the latest reading instead of `Math.max`, so
  corrections can go down.
- A crashed scan could block every scan for six hours; the lock now detects a
  dead owner by PID.
- Vanished files no longer sink a hand-off; every missing path is reported with a
  reason.
- Untracked `.DS_Store` and `gui-test-screenshots/`.

## [1.3.0] - 2026-09-29

### Added

- **Live reasoning meters.** Stage 2 has a determinate progress bar, an ETA, an
  items/sec readout, and a live cost meter against your cap.
- **In-app Finder ("The Finder").** A read-only screen that lays out findings by
  tier and cross-cuts them by personal risk, with multi-select and a live
  count/size, then hands the selection to Finder.
- **Partial reports on cancel.** "Pull ashore" aborts in-flight requests and
  returns everything judged so far, clearly marked as partial; unjudged items are
  labeled as heuristic fallbacks.

### Changed

- **Faster reasoning.** Batches run concurrently (5 in flight), batch size grew
  from 25 to 80 candidates, and folders in the same directory with the same
  profile share one judgment, labeled "shared verdict" in the report.
- **Smarter cost cap.** The projected spend is checked before each batch is
  dispatched, so one expensive call cannot blow past the ceiling silently.
- **Calmer reveals.** A bulk hand-off opens at most 6 Finder windows in one
  `open` invocation, largest folders first, with a summary of what opened and
  what was left out.
- **ZDR consequences surfaced.** Settings shows, under the selected model, what
  the zero-data-retention posture means for it. ZDR-only stays the default.

## [1.2.0] - 2026-09-28

### Added

- **Express Scan.** A "Skip AI reasoning" toggle: the scan finishes instantly,
  costs nothing, and ranks by heuristics alone. Items the AI would have judged
  are labeled as fallbacks, not verdicts.
- **Filter the report by tier.** The four stamps in the report header are
  buttons; group counts and byte totals follow the filter.
- **Reveal Driftwood in Finder.** One button opens every Driftwood-tier item
  preselected, one Finder window per folder, for the user to confirm with
  ⌘Delete.
- **Cache and log folders skip the AI.** `~/Library/Caches`, `~/Library/Logs`,
  `/tmp`, and orphaned Application Support folders band High without the LLM;
  user rules still get the last word.

### Changed

- **Danger zone in Settings.** "Allow providers that keep your data" turns off
  zero-data-retention routing. It takes a two-step confirmation, and every scan
  warns while it is enabled. The default stays protected.
- **Wider bands.** The high/low quantiles moved from 15% to 25%, so more folders
  reach the AI.

## [1.1.1] - 2026-09-27

### Fixed

- **"Command start_scan not found."** The Tauri backend now registers
  `start_scan`, `cancel_scan`, `last_report`, `correct_tier`, and `reveal_path`,
  bridged to the core scan engine with live `scan-event` progress and
  cancellation.
- **Stage 2 now uses the OpenRouter key** saved in Settings.
- **Report shape mismatches.** Groups, warnings, the cost-cap flag, and score
  components are translated to the shape the report screen expects, so persisted
  reports display after a relaunch.

## [1.1.0] - 2026-09-27

### Added

- **Settings "Where the river may look."** The same presets and per-folder
  toggles as first setup, so folder access is no longer locked in after
  onboarding.
- **OpenRouter setup in Settings.** Enter an API key (with a Test key button)
  and pick a text-generation model from OpenRouter's live catalog via search;
  the choice and cost cap are saved and passed to every scan.

### Fixed

- **Dead UI after a snag.** A scan failure could leave the interface
  unresponsive; page transitions now always swap immediately and recover.
- **White flash during the river transition.** The river art now melts into the
  paper while pages slide in and out.

## [1.0.0] - 2026-09-26

### Added

- Initial public release, shipped as an arm64 (Apple Silicon) macOS DMG.
- The Rust workspace: the `driftwood-core` scan/score/reason/report engine, plus
  the `driftwood-cli` and `driftwood-mcp` wrappers, and the app UI.

[Unreleased]: https://github.com/lajoonpark/DriftWood/compare/v1.4.1...HEAD
[1.4.1]: https://github.com/lajoonpark/DriftWood/compare/v1.4.0...v1.4.1
[1.4.0]: https://github.com/lajoonpark/DriftWood/compare/v1.3.0...v1.4.0
[1.3.0]: https://github.com/lajoonpark/DriftWood/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/lajoonpark/DriftWood/compare/v1.1.1...v1.2.0
[1.1.1]: https://github.com/lajoonpark/DriftWood/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/lajoonpark/DriftWood/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/lajoonpark/DriftWood/releases/tag/v1.0.0
