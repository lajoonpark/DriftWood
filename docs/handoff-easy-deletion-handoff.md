# DriftWood: make deleting the findings actually easy, without ever writing to disk

## Goal

DriftWood's last mile is broken. The user reaches the in-app Finder
(`app/src/screens/Browser.svelte`), selects their findings, presses "Show N in Finder", and
`reveal_paths` fires a single `open -R` with hundreds of paths. The result is a stack of Finder
windows that keep arriving after the command returns, each with a single item selected, and the
user still has to walk window-to-window pressing Cmd+Delete by hand. The promised experience —
"select, hand off, press ⌘Delete" — does not happen, and it reads as a worse experience than no
button at all.

The product promise stays exactly as it is: **DriftWood never deletes, moves, trashes, or modifies
anything it scanned.** This is settled and non-negotiable. The fix is entirely in the handoff:
make the number of Finder windows tiny, make each window's selection correct and *verifiable*, and
be honest about what opened.

The key insight driving the design: **the number of windows is the number of deletion round-trips.**
Finder's Cmd+Delete acts on the active window's selection, so N windows always means N trips
regardless of how well each one is preselected. `open -R` is also a *reveal* verb, not a
*select-many* verb — it does not reliably leave a whole group selected, and it returns before
Finder has finished opening windows. It is the wrong primitive for this job. The right primitive is
AppleScript against Finder, which can set a window's selection to an exact alias list and read the
resulting selection count back.

The second insight: candidates are already the right unit. `resolve_roots`
(`crates/driftwood-core/src/scan/scope.rs:30-75`) defines six-ish scan roots
(`~/Library/Caches`, `~/Library/Logs`, `~/Library/Application Support`, `~/Library/Containers`,
`/tmp`, `~/Downloads`, plus the high-risk personal folders), and the walk makes each candidate one
*top-level child* of a root with deeper levels aggregated. So a 200-item Caches finding set is not
200 things to delete — it is one folder. Collapsing the handoff to distinct parent directories
turns hundreds of leaves into roughly 6-12 round trips, which is what makes deletion actually
easy while staying read-only.

## Context

Monorepo: Rust workspace (`crates/driftwood-core` is the engine, consumed by
`crates/driftwood-cli` and `crates/driftwood-mcp`) + a Tauri v2 / Svelte 5 frontend in `app/`
(shell at `app/src-tauri/src/lib.rs`, frontend at `app/src`).

### The trust moat

Stated in `app/src/screens/Welcome.svelte` ("Read-only, always. It never deletes a thing"), the
Full Disk Access trust screen, and `app/src/screens/Report.svelte` ("DriftWood deleted nothing. It
never does."). `crates/driftwood-mcp/src/main.rs` exposes no tool that can delete, move, or write
anything outside DriftWood's own memory folder. `docs/handoff-reasoning-speed-and-finder.md`
mandates the same for the in-app Finder.

**Do not add any code path that unlinks, trashes, moves, renames, or writes to a scanned path.**
Writing to DriftWood's own temp dir for an AppleScript payload is fine; touching a scanned path is
not. If a design idea requires DriftWood to act on a file, cut it. Earlier options that were
considered and rejected: a staging folder on the Desktop, one-click Trash via AppleScript
`delete`, and generating a user-runnable cleanup script. All three require DriftWood to modify the
filesystem or the user to run opaque code; none are in scope.

### Relevant existing code

- `app/src-tauri/src/lib.rs:68-134` — `reveal_paths`, the broken bulk handoff. Groups by parent,
  sorts largest-group-first, caps at `MAX_WINDOWS: usize = 6`, and issues ONE `Command::new("open")
  .arg("-R")` with every path. `Command::status()` returns as soon as `open` exits, which is well
  before Finder has finished opening the windows. This is the root cause of the reported symptom.
- `app/src-tauri/src/lib.rs:44-46` — `reveal_path`, the single-item reveal used by entry cards.
  This one is fine and should keep working as-is.
- `app/src-tauri/src/lib.rs:50-60` — the `RevealSummary` serde struct. Its shape changes (below).
- `app/src/lib/types.ts` — `RevealSummary` mirror; the Rust struct and this must stay in lockstep.
- `app/src/lib/bridge.ts` — the only seam between frontend and world. `Bridge` interface with
  `TauriBridge` and `MockBridge`. **Every new/changed method needs both**, or `npm run dev` in a
  plain browser breaks and the screen becomes unreviewable. `MockBridge.revealAll` currently
  simulates the old grouping and 3-window cap and must be updated.
- `app/src/screens/Browser.svelte` — the read-only browser. Per-leaf checkboxes, per-tier
  sections, `scopeFilter` cross-cut, windowed rendering at `PAGE = 60` per section,
  `handOff()` at line 123, the footer button at line 273, and the Source/tier-4 guard at
  line 210 (no select-all on tier 4, plus a `hasSource` warning in the footer).
- `app/src/screens/Report.svelte:111` — the "Could be freed" headline, which sums *leaf* bytes.
- `crates/driftwood-core/src/scan/scope.rs:30-75` — `resolve_roots`, the scan roots.
- `app/src-tauri/src/lib.rs:414-482` — the existing `#[cfg(test)] mod tests` in the shell, with a
  `report_shape_matches_frontend_contract` shape test and a `tier_sources_pass_through_untouched`
  provenance test. Add to this module.
- `docs/handoff-reasoning-speed-and-finder.md` §5-6 specified the original single-window reveal and
  the read-only browser. Update those sections to record the new container-level design decision and
  the reason for it, so the doc does not contradict the code.

## Requirements

### 1. Measure first, before choosing the rollup depth

The design assumes a real report has a small number of distinct parent directories. Do not assume
it — measure it.

- Add a way to compute, for a given path list, the number of distinct parents and the distribution
  of items/bytes per parent. Expose it as a pure function with unit tests.
- Run it against the user's actual persisted report (`Report::load_last()`) and against
  `MOCK_REPORT` in `app/src/lib/mock.ts`.
- Report the numbers in the implementation summary. If distinct parents is small (roughly ≤ 15),
  the design below stands as written. If it is large, the rollup in requirement 3 is required, not
  optional — see the fallback note there.

### 2. Replace the bulk `open -R` with a verified AppleScript handoff

Remove the `Command::new("open").arg("-R")` mass-invocation from `reveal_paths` and replace it with
`osascript` driving Finder directly. Requirements:

- **Per parent group:** one Finder window per parent directory. Open the window at that directory
  (via a new Finder window whose target is set to the parent, or `open -R <parent>`), then set its
  selection to the exact list of aliases for that group's findings.
- **Verify and report the truth:** after setting the selection, read back
  `count of selection of window N` from Finder and put the verified number in the result. The UI
  must state what Finder actually has selected, not what we asked for. A mismatch is surfaced, not
  hidden.
- **Build all windows first, set all selections, then activate only the first window at the end.**
  Do not activate each window as it is created — the user should not watch focus jump repeatedly
  while windows assemble. The first window should be frontmost when the handoff finishes.
- **Do not depend on window indices being stable.** Resolve each window by the object returned from
  creating/opening it (a `Finder window` object reference), not by counting on a global index.
- **No silent drops.** Any group that could not be opened, or any item that was missing on disk
  between scan and hand-off, must appear in the result with a reason. The current behavior of
  dropping overflow groups behind a cap and mentioning it in a toast is what makes this feel
  dishonest — with container grouping the overflow should be small, and each skipped item must be
  individually accounted for.
- **Keep `reveal_path` (single item) on `open -R`.** It is a one-item, one-window operation and
  works correctly. Do not route it through the new machinery.
- `crates/driftwood-mcp/src/main.rs`'s single-path `reveal` tool is unaffected.

### 3. Group the handoff by container, with rollup when there are many

The handoff unit is the containing directory, not the leaf.

- Collapse the selected paths to their distinct parent directories.
- **Drop a parent that is a descendant of another parent in the set** (if both
  `Library/Application Support/Chrome` and `Library/Application Support/Chrome/Cache` are present,
  keep the ancestor; the window already contains the descendant). Handle path comparison on
  component boundaries, not string prefixes, so `/foo/barbaz` is not treated as inside `/foo/bar`.
- **Rollup, if requirement 1 shows the distinct-parent count is too large.** Repeatedly merge the
  set of parents up to a common ancestor, choosing at each step the candidate ancestor that covers
  the most reclaimable bytes, until the count is at or below a target (roughly 12). Hard guards on
  rollup:
  - Never roll up to a filesystem root, to `$HOME` itself, or to any ancestor that is not a
    directory the user actually scanned.
  - Never roll up across a tier-4 / `ScopeCategory::High` boundary. A container that contains any
    tier-4 or high-personal-risk finding must not be merged with, or promoted by, a rollup that
    would make those findings disappear into a broader "clear this" action.
  - Never roll up a container into something that is itself a finding.
- Order the resulting containers by reclaimable bytes descending, so the first few cover the
  majority of the recoverable space and the user can stop early.

### 4. Guard the real hazard: Cmd+A reaches past the findings

Container-level hand-off introduces a way for a user to delete far more than DriftWood found. This
is the one thing in this design that could actually hurt someone, so it must be handled explicitly.

`~/Library/Application Support` might hold 40 driftwood findings inside 4,000 files of live
application data. Handing that window over as "40 items" and watching the user Cmd+A is how an app
loses someone's mail archive.

- Every container row in the UI must show findings-versus-total: e.g. `412 items in this folder ·
  40 are driftwood · 4.2 GB of findings`.
- The findings count must be the preselected selection (requirement 2), so a plain Cmd+Delete
  touches exactly the findings. Cmd+A is the user's deliberate escalation beyond that.
- Any container with a low findings-to-total ratio gets an explicit, visible warning in the UI.
- A container containing any tier-4 or high-personal-risk finding must be visually distinct and
  must carry the existing `TIER_BLURBS` framing. The current per-row tier-4 guard in
  `Browser.svelte:210` is insufficient — it operates on leaves and cannot see that a container
  aggregates something dangerous. This guard must move up to the container level.

### 5. The counts of a folder

To render the findings-versus-total ratio, the shell needs the entry count of each container
directory.

- Count with a cheap `read_dir`, and make the count **nullable** — a container whose count cannot
  be obtained (permissions, or a directory too large to walk cheaply) must show "total unknown"
  rather than a wrong number or a blocking scan.
- Do not walk a large directory exhaustively just to render a label. Cap the work (e.g. stop
  counting past a few tens of thousands of entries and report the count as unknown/at-least), and
  prefer `metadata` where it is cheaper.
- Returning an honest `null` must never be treated as a ratio of zero, which would wrongly flag
  every container as dangerous.

### 6. Two byte figures that must not be conflated

- **Findings bytes** — the sum of the `size_bytes` of the selected findings. This is what DriftWood
  claims it found.
- **Container bytes** — the size of the containing folder, which is larger and includes
  non-findings.

`app/src/screens/Report.svelte:111`'s "Could be freed" is a findings-bytes figure and is correct as
a *findings* number. Do not silently switch it to a container figure, and do not display the two
with the same label. Any container-level view must label which number it is showing.

### 7. New `RevealSummary` shape

Replace the current `{windows, items, skipped_groups, skipped_items}` with something that can
express verification, per-container detail, and per-item skip reasons. Proposed:

```ts
export interface RevealGroup {
  folder: string;              // the parent directory opened
  findings: number;            // driftwood findings in it
  finding_bytes: number;       // reclaimable bytes of those findings
  total_in_folder: number | null;  // directory entry count, null when unknown
  requested: number;           // aliases we asked Finder to select
  selected: number;            // what Finder reported as selected — may differ
  tainted: boolean;            // contains a tier-4 / high-personal-risk finding
  ok: boolean;
  error?: string;
}

export interface RevealSummary {
  groups: RevealGroup[];
  skipped: { path: string; reason: string }[];  // every path not handed off, and why
  windows: number;
  items_requested: number;
  items_selected: number;      // Finder-verified total
}
```

`succeeded` must never be implied by the mere absence of an error — it comes from Finder's
readback. Update the Rust struct and `app/src/lib/types.ts` together.

### 8. Frontend changes in `Browser.svelte`

- Keep the existing per-leaf browsing: the tier sections (`TIER_NAMES`), the `scopeFilter`
  cross-cut, `PAGE`-windowed rendering, and the read-only framing. Browsing is not the problem and
  should not regress.
- Add a derived **container view** built from the current selection: distinct parent directories,
  each with findings count, finding bytes, folder total (or unknown), and taint state, sorted by
  bytes descending.
- Present the hand-off as a set of deliberate per-container actions with counts and warnings
  visible *before* the user commits, plus a bulk action for the top containers that states
  exactly how many windows it will open and how many findings that covers. A single unlabeled
  "show everything" button is what produced the current complaint.
- On completion, report Finder's verified numbers, and surface any group that failed or any path
  that was skipped and why. No silent partial success.
- Update the hand-off copy to stay accurate under the new mechanics. "DriftWood deleted nothing.
  It never does." remains literally true. Where copy describes the hand-off, it should say
  DriftWood shows exactly what it found, opens it in Finder, and the user presses ⌘Delete there.

### 9. Bridge parity and Automation permission

- Implement every changed method in **both** `TauriBridge` and `MockBridge` in
  `app/src/lib/bridge.ts`. `MockBridge` must simulate the new grouping, the rollup, the window
  ordering, the taint flag, and a verification count — including a way to simulate a failure or a
  count mismatch so the UI's honest-reporting path is reviewable in the browser. Use a
  `URLSearchParams` flag in the style of the existing `?snag` flag in `MockBridge`'s constructor.
- **macOS Automation permission is a new user-facing requirement.** Driving Finder via `osascript`
  triggers a TCC prompt the first time. The app already has a trust/onboarding flow
  (`app/src/screens/Trust.svelte`, `check_full_disk_access`, `open_system_settings`) — extend it to
  handle Automation denial gracefully: detect the `-1743` ("not allowed to assist") error, show a
  calm explanation of why Finder control is being requested, and offer a deep link to System
  Settings → Privacy & Security → Automation. Do not let a raw osascript error surface as a dead
  button.

## Edge cases and considerations

- **AppleScript argument length and path quoting.** Do not pass hundreds of paths as `open`
  argv. Feed the script to `osascript` on stdin (`osascript -`) to avoid argv limits. Paths must be
  embedded with correct AppleScript string escaping — single quotes and backslashes must be
  escaped, and a path containing a newline or a quote must not corrupt the script. Use
  `POSIX file "<path>" as alias` per item, and note that `as alias` throws if the file does not
  exist.
- **Files that vanished between scan and hand-off** are expected, not exceptional — caches get
  cleared by the very apps that own them. Check existence before building the alias list, and
  report each miss in `skipped` with a reason. A hand-off that hard-fails because one of 400 paths
  disappeared is a bug.
- **Selecting a large group in one window** — 200 aliases in a single `set selection` — should be
  verified as actually working at that size. If Finder is slow or drops items at high counts, chunk
  the selection and verify the final count.
- **Cross-volume and symlinked paths.** `~/Library/Containers` and `/tmp` can be symlinks; the
  resolved path may differ from the scanned path. Group by the path as scanned, but be aware
  Finder may display the resolved target.
- **Rollup must be deterministic and inspectable.** Sort candidate ancestors by a stable key
  (bytes descending, then path) so the same report always produces the same containers. The user
  should be able to see *why* a group was formed — if a container aggregates 40 findings across
  12 parents, say so.
- **Preserving the existing guarantee against a regression.** Add a Rust test asserting the new
  handoff code path contains no filesystem mutation — no `remove_file`, `remove_dir`,
  `rename`, `fs::write`, or `trash` call on anything derived from a caller-supplied scanned path.
  This is a moat test, not a formality.
- **Unit-test the pure logic**, not the osascript call: grouping by parent, descendant-parent
  dropping with component-boundary comparison, the rollup merge order and its guards (never to a
  root, never across a tier-4 boundary), byte-descending ordering, and the nullable folder count.
  Put these in the existing `#[cfg(test)] mod tests` in `app/src-tauri/src/lib.rs`. Keep
  `tier_sources_pass_through_untouched` and `report_shape_matches_frontend_contract` passing.
- **Non-ASCII and emoji paths** must survive the round trip through AppleScript intact.
- **Do not regress the windowed rendering.** Reports can carry thousands of entries; naive
  `{#each}` over them is already a known constraint (`docs/handoff-reasoning-speed-and-finder.md`
  flags it). The new container view must also be bounded or windowed.
- **Keep `driftwood-mcp`'s read-only guarantee** — the MCP server's tool surface is unchanged; do
  not add a bulk-reveal tool there.

## Ask

Implement the goal, in this order, verifying as you go:

1. The pure grouping/rollup/taint logic in `app/src-tauri/src/lib.rs` as testable functions, with
   the unit tests from the "Edge cases" section, plus the measurement from requirement 1. Run it
   against the real persisted report and report the numbers before going further.
2. The new `RevealSummary` shape in Rust and its `app/src/lib/types.ts` mirror, together.
3. The AppleScript handoff replacing the mass `open -R`, with Finder readback verification,
   window objects resolved rather than indices, all windows built before any is activated, and
   per-path skip reporting.
4. The Automation-permission handling and the deep link, integrated into the existing trust flow.
5. The nullable folder-entry count, with the unknown case handled honestly.
6. The container view in `Browser.svelte` — derived groups, findings-vs-total display, taint
   warnings, per-container actions, the honest completion report, and updated copy. Keep the
   existing per-leaf browsing and windowed rendering intact.
7. `MockBridge` parity, including a failure/mismatch simulation path, and the `?`-flag wiring.
8. The moat regression test, the copy update, and the `docs/handoff-reasoning-speed-and-finder.md`
   §5-6 revision.

Run `cargo test` for the Rust workspace, plus the frontend typecheck and build. Existing tests in
`reason/mod.rs` and `client.rs` and the two contract tests in `app/src-tauri/src/lib.rs` must
continue to pass — extend, do not weaken or delete. If a design decision is genuinely ambiguous,
make the conservative choice that preserves the never-deletes guarantee and say so explicitly in
the summary. Report the measurement numbers from requirement 1 in that summary too, since the
rollup depth depends on them.
