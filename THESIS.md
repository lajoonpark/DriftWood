# THESIS — autonomous exploration run

Repo state at start: v1.3.0 (`afecd4d`) plus ~3,900 lines of uncommitted work implementing
`docs/handoff-honest-tiers.md` (argued auto-high, system/vendor floor, `ScanMode` enum,
on-demand adjudication). The tree was already green: 89 Rust tests pass, `svelte-check`
0 errors, `vite build` clean. That uncommitted work is sound and I am building on top of it.

---

## Entry 0 — Ground truth: what the product actually is (read-only survey)

**Observed.** Core (`driftwood-core`) is genuinely the product: scan/score/reason/rules/report
all live there; the Tauri shell, CLI, and MCP server are thin. The moat is enforced in three
layers: (1) no code path names a mutation API — `handoff_source_contains_no_mutation_apis`
greps the production half of `app/src-tauri/src/lib.rs` for `remove_file`/`remove_dir`/
`fs::rename`/`fs::write`/`trash`; (2) `handoff_pipeline_leaves_files_untouched` proves
byte-identity through the whole pure hand-off pipeline; (3) provenance flows through a single
`tier_source` channel with a pass-through contract test and mirrored `SOURCE_PHRASES`.

**Judged.** The architecture is unusually honest. My job is to find where it lies or freezes.

---

## Entry 1 — DOGFOOD: the scan hangs forever on the real filesystem, twice, in two different phases

**Observed.** First real run, `./target/debug/driftwood scan --scope all --quiet`:

- After ~2 minutes the process had used **0.02s of CPU** and emitted nothing. `sample` showed
  the main thread blocked in `open$NOCANCEL` ← `__opendir2` ← `std::fs::read_dir` ←
  `scope::enumerate_units` — the scan was stuck opening one of the top-level scope roots.
- `kill -INT` did terminate that first hang (see below for why the second one is worse).
- A clean `--scope low` run then **completed** (575 KB report, 345 entries, 3.1M files
  measured, 330 GB walked) — but only after minutes of debug-build walking, and a mid-run
  sample caught it blocked in `open()` again, this time inside `measure_folder` →
  `walkdir::IntoIter::handle_entry` (a subdirectory open during sizing).
- A clean `--scope medium` run reproduced it deterministically: **hung after 40s**, zero CPU,
  one `spawn_blocking` measure task blocked in `opendir` of a directory under a Medium root.
  The other 7 sizing workers were idle; the whole scan waited on one unresponsive directory.

**Root cause (class, not instance).** Every Stage-1 filesystem touch is a bare blocking
syscall with no timeout and no cancel path:

- `scope::enumerate_units` — synchronous, called on the async runtime thread; one unresponsive
  directory (TCC-pending access, dead network mount, stale disk image, unresponsive FUSE)
  blocks the whole future. The exact trigger on this machine is environment-dependent
  (sandboxed process tree, TCC for Downloads/Documents on an unsigned binary); the defect is
  not the trigger but the *absence of a bound*: the notes' own edge case §4.3 says
  "permission errors are skip-and-count, never abort" — a hang defeats that by never
  returning at all.
- `walk::measure_folder` — same, inside `spawn_blocking`; the sizing drain loop
  (`join.join_next().await`) waits on it forever. **Cancel cannot reach it**: the cancel flag
  is only checked before dispatching the next unit, so a user who sees a frozen scan and hits
  Ctrl-C (or "Pull ashore" in the app) during sizing gets nothing — the flag is never
  observed. "Honest cancel" (v1.3.0) only covers Stage 2.
- `spotlight::run` — `mdutil`/`mdfind`/`mdls` subprocesses with `kill_on_drop` but **no
  timeout**; `mdfind -onlyin <root>` on a hung mount hangs the same way.
- `orphan::enumerate_installed_apps` — synchronous walkdir of `/Applications` directly on an
  async worker thread (also blocks the runtime; also unbounded).

**Also measured:** debug-build sizing walks 3.1M files in minutes of pure CPU; dogfooding
must use `--release` (noted for every future run).

**Built.** See Entry 2.

**What I would do next:** feed real-world trigger inventory (TCC states per root) into the
trust screen so the app can say "this riverbed never answered" per root, not just globally.

---

## Entry 2 — Build #1: bounded riverbed — every Stage-1 filesystem wait gets a timeout, a skip, and a warning

(This entry is written after the build; the design constraints were fixed before coding.)

**Why this and not something else first.** A cleanup tool that silently freezes is worse than
one that reports wrong-but-labeled heuristics: the freeze destroys the trust loop entirely
(no report, no provenance, no cancel). It was found by a real run, not by reading code —
exactly what the handoffs said would happen.

**Design decisions (made autonomously, rationale inline):**

1. **Timeouts live in `WalkCaps`** (`spotlight_timeout_secs: 15`, `root_timeout_secs: 30`,
   `measure_timeout_secs: 120`, all tunable via TOML) — dogfooding can retune without
   rebuilds, consistent with the existing knob-bag philosophy.
2. **Skip-and-warn, never abort** — the scan continues without the unresponsive subtree and
   says so, in themed copy ("wading past …"), consistent with edge case §4.3. A degraded
   report that arrives beats a perfect report that never does.
3. **Sizing give-up counter**: a leaked blocked thread per timed-out `measure_folder` is
   unavoidable (threads can't be killed); to bound the worst case the engine stops
   dispatching new measurement tasks after `max_measure_timeouts` (8) and marks the
   remainder honestly (truncated stats + warning), instead of leaking unboundedly.
4. **Cancel through sizing**: the drain loop now selects on a cancel poll so Ctrl-C /
   "Pull ashore" lands during sizing too. (Early-phase cancel still discards the scan —
   there is nothing worth reporting before scoring; that contract is unchanged.)
5. **Core-only** — all of it lives in `driftwood-core`; wrappers get the fix for free,
   consistent with constraint 4.

**Measured.**
- Unit: 4 new tests (93 → 96 total, all green). `run_times_out_a_hung_helper` proves the
  Spotlight bound against a real `sleep 5` subprocess; `measure_wrapper_times_out_a_slow_task`
  pins the spawn_blocking timeout primitive; `unmeasurable_riverbed_degrades_with_warnings`
  walks the full pipeline through the give-up path (every folder reported, zero size claimed,
  `truncated: true` in the data model, `measure_timeouts` warning in the report); three
  lock-integrity tests cover the stale-lock fix below.
- Real FS: the previously-hung `--scope medium` run now completes in **2.1 s** (614 entries,
  17.3 GB measured) and `--scope low` in **2.9 s** (345 entries, 3.1M files, 330 GB — same
  result as the pre-change debug run, in release).
- **Honest caveat:** the original trigger did not reproduce after the fix landed. The two
  zero-CPU hangs were real (sampled in `open$NOCANCEL`, twice, different phases) but transient.
  So this build is not "fixed a reproduced hang" — it is "removed the unbounded-wait class";
  any future trigger (TCC-pending, dead mount, wedged FUSE) now degrades to a labeled warning
  within `measure_timeout_secs` instead of a silent freeze. The tests, not the environment,
  carry the proof.
- **Bonus dogfood find, fixed in the same entry:** a SIGKILLed scan leaves `scan.lock` behind
  and the old staleness check only consulted mtime age (6 h) — the lock records its owner PID
  but nothing ever checked it. A crashed scan could block every scan for 6 hours (I hit this
  in verification). `ScanLock::acquire` now steals the lock when the owner PID is gone
  (`ps -p` probe; unambiguous where `kill -0` is not), age stays as the recycled-PID backstop.

**Judged.** Strictly more honest than before, negligible cost. One wart: a timed-out folder
reports size 0 — we claim nothing — but a user reading "0 B" without the warning could
misread it as "empty". The `truncated` flag in `kind_stats` says the walk was incomplete, and
the report carries `measure_timeouts` warnings; if the UI ever wants a fourth figure
("size unknown"), the data model already supports it.

**Would do next:** per-root TCC probing (`check_full_disk_access` already exists in the
shell) surfaced as a pre-scan warning rather than a mid-scan timeout.

---

## Entry 3 — Build #2: an Express scan must not wear failure copy (`TierSource::Heuristic`)

**Observed.** My real Express report labeled 15 middle-band items
`tier_source: "fallback"` with the summary **"Snagged — the river ran rough here; heuristic
estimate only."** Nothing snagged: Express never asks the river. The user chose a no-AI scan;
per-card copy claiming a failure is a provenance lie of exactly the kind constraint 2 forbids —
it makes "the LLM tried and failed" indistinguishable from "the LLM was never part of this
scan". Deep read with no API key had the same defect through the other fallback arm.

**Built.** A distinct `TierSource::Heuristic`: *the scan mode never asked the river — the tier
is a heuristic estimate by design*. `Fallback` keeps its meaning: tried and failed. The engine
now decides with `llm_attempted = mode.runs_llm() && api_key present`, and both fallback arms
split on it. Heuristic cards get non-empty reasoning ("No AI reasoning was spent on this item…")
so the "Why the river says so" dropdown appears with an honest story instead of silence.
Mirrored everywhere the constraint requires: `types.ts` union, `SOURCE_PHRASES`
("heuristic by design — the river was never asked", warn), `Browser.svelte` row note,
`MockBridge.adjudicate`'s unargued set, the shell's `tier_sources_pass_through_untouched`
contract test, and a mock entry so the browser-review path exercises the new phrase.

**Measured.** New pipeline test `express_middle_band_is_heuristic_not_snagged` (self-contained
fixture: a top-level aged file under Application Support lands middle without touching a cache
root or an installed-app list); the floor test's deep-read-no-key arm now asserts `Heuristic`
and no "Snagged" copy. Real scan after the change: `tier_sources: {system_floor: 145,
argued_auto_high: 143, heuristic: 15, auto_low: 41, never_flag: 3}` — the 15 formerly-"snagged"
items now read "Heuristic estimate — this scan never asked the river about this item."
97 Rust tests green, svelte-check clean.

**Judged.** Strict improvement, no cost. One design note: `fallback_tier` scoring stays shared
between the two sources (score ≥ 60 → Bottle, else Current) — reasonable for both stories, but
if heuristic-by-design ever wants its own mapping (Express users see a LOT of Current), that is
now expressible at the assembly site.

**Would do next:** the Scan-mode picker could show a live estimate of how many cards will be
argued per mode, so Express users know exactly what "no AI" buys and costs.

---

## Entry 4 — Build #3: "Could be freed" was counting the untouchable

**Observed.** On my real low-scope report the headline read **23.53 GB**. Breaking it down by
tier: T1 11.01 GB, T3 5.76 GB, **T4 6.75 GB**. So 12.5 GB — 53% of the headline — was Tier 3
("recoverable, but costly to reacquire") and Tier 4 ("personal or irreplaceable — don't touch").
`Report.svelte`'s `totalBytes` summed *every* entry's bytes, and core's live "Recoverable so
far" counter stored the everything-sum *before any tier decision existed* — including
never-flagged personal locations. For a non-technical audience (the product's stated target) a
"could be freed" number that includes the mail archive is the most dangerous kind of
dishonesty: technically a sum of findings, practically a promise.

**Built.** Two numbers, both labeled, never blurred:
- **Headline "Could be freed" = tiers 1–2 only** (the tiers whose own blurbs say disposable),
  honoring user re-stamps via `effTier` — a re-stamp moves bytes between figures immediately.
  A sub-line under the headline states "tiers 1–2 only · all findings: X" so the neutral
  figure stays visible and can never masquerade as the actionable one (the easy-deletion
  handoff's rule: never display the two figures with the same label).
- **Core's recoverable counter is now conservative-then-exact.** Mid-scan it counts only
  *definitely* disposable candidates (auto-high'd, not floored, not never-flagged, not
  pinned above tier 2; nothing at all in Deep read where every band is advisory). After
  assembly it re-emits the exact tiers-1–2 total, which is also what
  `counters.recoverable_bytes` persists in the report. The old banding-time everything-sum
  emission is gone.
- `stores.ts`'s reducer now **takes the latest** recoverable event instead of `Math.max` —
  the max would have pinned the stale mid-scan figure even after the exact (smaller)
  correction arrived. Corrections downward are information.

**Measured.** Floor-test assertions pin the invariant (`report.counters.recoverable_bytes ==
T1+T2 sum`, strictly below the everything-sum whenever tier 3/4 data exists). Real scan after:
`recoverable_bytes = 11.01 GB == T1+T2 sum` exactly; previously 23.53 GB. Mock timeline updated
to model the conservative→exact sequence. 97 Rust tests green, svelte-check and build clean.

**Judged.** This is the change I would defend hardest from this run. It does not alter a single
tier decision — only what the app claims about them — and it removes a 2.1× overclaim that
every previous report carried. Cost: the headline number drops, which "feels" worse in a demo;
that is the point of honest numbers.

**Would do next:** the same lens on the group headers ("N items · X GB per category") — they
are informational today, but could offer a per-category tiers-1–2 split so users triage
personal-risk areas by what is actually actionable there.

---

## Entry 5 — Run summary, honest accounting

**What this run changed** (all verified green: 97 core + 23 shell tests, svelte-check clean,
moat tests untouched and passing):

1. **Bounded riverbed** (`engine.rs`, `spotlight.rs`, `config.rs`) — every Stage-1 filesystem
   wait (root enumeration, folder sizing, installed-app snapshot, mdutil/mdfind/mdls
   subprocesses) has a timeout, a themed skip-and-warn, a bounded give-up counter, and cancel
   that lands during sizing. Plus the stale-lock PID check so a SIGKILLed scan cannot block
   scans for 6 hours. Dogfood-found; the trigger proved transient, the class is gone.
2. **`TierSource::Heuristic`** (`types.rs`, `engine.rs`, + all mirrors per constraint 2) —
   Express scans and key-less Deep reads no longer wear "Snagged" failure copy; "the river was
   never asked" is a different provenance from "the river failed".
3. **Honest recoverable bytes** (`engine.rs`, `Report.svelte`, `stores.ts`) — the headline
   counts tiers 1–2 only (11.01 GB vs the old 23.53 GB on a real scan); all findings stay
   visible, labeled; the live counter is conservative mid-scan and exact after assembly; the
   reducer takes corrections downward instead of pinning a stale max.

**What I considered and cut, with reasons:**

- *Spotlight-provided folder sizes to skip the 3.1M-file walk* — the release build walks
  330 GB in ~3 s, so the win is negligible and kMDItemFSSize staleness would make the
  "recoverable" figure (this run's other headline fix) quietly wrong. Honesty-per-second
  lost the trade.
- *Keyboard triage / export-script ideas from `newideas.md`* — the container hand-off
  (already shipped) covers the need without new deletion-adjacent surface area.
- *Per-root TCC pre-probing in the shell* — right idea (Entry 1's "would do next"), but
  mid-scan degradation now makes it a UX nicety rather than a correctness need; ran out of
  conviction before running out of budget.

**Self-judgment, severity-ordered:**

- The honest-recoverable change alters what a demo looks like (smaller headline). If the
  maintainer disagrees with counting only tiers 1–2, the *fallback* position is not the old
  everything-sum; it is relabeling the headline "All findings" and keeping tiers 1–2 as the
  actionable figure. The conflation itself was the defect.
- The bounded-wait warnings can be noisy on a genuinely degraded machine (per-folder warn
  lines). A single consolidated summary line would read better; the report warnings are
  already consolidated.
- Entry 2's timeouts are generous-by-default (30 s root, 120 s folder). A user with a truly
  wedged mount waits 2 minutes per folder × 8 before the give-up trips. Tunable, documented,
  but the first real stuck machine will teach the right defaults.

**State left behind:** the pre-existing uncommitted honest-tiers work (argued auto-high,
system floor, ScanMode, adjudication) is still uncommitted — I built on top of it and its
tests guard everything I touched. THESIS.md sits at the repo root; nothing in
`driftwood-core` writes outside its base dir; the moat greps and byte-identity tests pass;
`driftwood-mcp` gained no tools.

---

## Entry 6 — The easy-deletion session: verifying a hand-off nobody logged, and the relaunch hole

**The goal I was given:** "make deleting the found files in the report as easy as possible
to delete, without the app deleting them itself" — deliberately ambiguous, a test.

**Observed first.** The working tree already contains a complete, unlogged implementation of
`docs/handoff-easy-deletion-handoff.md` (container grouping, guarded rollup, AppleScript
hand-off with Finder readback verification, `-1743` consent handling, container view with
findings-vs-total, MockBridge parity, moat tests, even the §5–6 doc revisions) — none of it in
THESIS.md, which still ended at Entry 5. I chose verification over rewriting: every prior
defect in this project was found by a real run, and this code had never survived one.

**Real runs, before and during code reading:**

- Fresh `--release` scan, then the requirement-1 measurement on it: 347 findings → **4
  distinct parents** (App Support 19.1 GB/131 items, Caches 4.3 GB/189, Logs 0.09 GB,
  /tmp 0.04 GB); tiers 1–2 land in the same 4; **2 of the 4 containers are tainted** with
  Source-tier findings — the taint warning is load-bearing, not decorative. Rollup stays
  implemented but is structurally idle (scan candidates are already top-level children of
  ~8 roots).
- The shipped `open -R` primitive, reproduced on a /tmp fixture: one call with 5 paths →
  **7 windows, every one `selcount=0`**, 4 of them opened at the machine root. The handoff
  doc's complaint is real and understated: nothing is selected at all, so ⌘Delete has no
  target.
- The replacement primitive, probed before trusting it: `set selection of window id` fails
  (-10006, the known modern-macOS breakage); the working recipe is window-by-`id` +
  z-order raise + app-level `select` + app-level readback — verified with spaces, quotes
  and emoji paths, a 100-alias single select (2.2 s, no chunking needed at real scales),
  and with Finder fully backgrounded (TextEdit frontmost) so assembly never steals focus.
  System Events turned out to be a TCC trap (-1712 hang); the design correctly avoids it.
- A new permanent opt-in end-to-end test (`real_finder_handoff_end_to_end`) runs the real
  `plan_groups → build_handoff_script → run_osascript` pipeline against real Finder: 99+2+3
  verified readbacks across three windows, one path vanished mid-flight misses **alone**
  (per-item try), fixture windows closed by resolved-path matching afterward.

**Defects found and fixed this session (all by real runs, none by reading):**

1. **The generated script selected nothing at all** — introduced by my own refactor of the
   alias construction to per-item tries: one line used `push_str` where it needed
   `format!`, leaving a literal `s{i}` in the AppleScript. Every alias step errored into
   the per-item catch (101 MISS lines, every group `OK:0`) while still *compiling* — the
   failure mode is silent by design, which is exactly why the string-assertion unit tests
   sailed past it. Hardened the tests (`set end of s0` must appear; a literal `{i}` must
   not) and the e2e test now pins the real behavior. Honest note: the prior session's
   chunk-literal form was correct; the bug is mine, and the real run caught it within one
   execution — the owner's philosophy, demonstrated on myself.
2. **The report was unreachable after a relaunch** (found via GUI review). Nothing ever
   loaded the persisted report into the store at startup, so `hasReport` stayed false and
   the Report / The Finder nav links vanished — yesterday's findings were one forced rescan
   away from being handed off, and the rescan changes what it finds. Fixed at the right
   level: `scan.loadPersisted()` (never overwrites a store that already has a report, so it
   cannot race a fresh scan), App-level restore effect, and Report.svelte now mirrors
   Browser.svelte's cold-load + honest "No report yet." empty state. The deep-linked Report
   screen had been rendering *blank* (crash on the cold store) — same fix. Verified in the
   browser: report renders cold, nav links appear.
3. Two build warnings (`ParentStat`/`parent_stats` dead in production) — measurement
   machinery moved into the test module where its only callers live.

**Measured (verification state):** 97 core + 23 shell tests green, plus the two ignored
real-machine tests (measurement, e2e) green when run; `svelte-check` 0 errors 0 warnings;
`vite build` clean; both moat tests untouched and passing; the app shell builds. The tree
gains: per-item MISS resilience in the hand-off script, the permanent e2e test, the
relaunch restore, and the Report empty state.

**GUI review, honestly scoped:** rendering verified by screenshots for Welcome, The Finder
(tier sections, no select-all on Source, honest footer copy), Trust (the new Finder hand-off
consent block), and Report. **The interactive flows — select → plan panel → hand-off click →
honest result report, and the `?reveal-snag` / `?reveal-denied` paths — could not be
click-driven this session**: the in-app-browser runtime delivered no synthesized input at
all (three mechanisms failed even on the trivially-correct Welcome screen), so I judged them
by unit tests, the mock machinery, and code review rather than pretend to a click-through.
A human pass over the real Tauri window remains worthwhile; the `-1743` consent flow was
never triggered live here (Finder automation was already granted on this machine).

**What "as easy as possible" still leaves on the table** (considered, not built): a
hand-off entry directly on the Report screen (today: Report → The Finder → select → hand
off); preselecting tiers 1–2 would be one step easier but nudges a blanket delete — against
the moat's spirit, rejected. The relaunch fix removes the biggest real friction I could
find: the findings now survive an app restart.

**Judged.** The prior session's implementation was genuinely good — the container design,
the honest readback contract, and the guards all held up under adversarial probing. What it
lacked was a run log and any real-run evidence; this session's contribution is the proof
(one silent script-killer, one relaunch hole), the two fixes, and the permanent test that
makes the Finder hand-off lie-detected from now on.

**Would do next:** the human click-through of the hand-off panel in the real app window;
and if the owner wants one-step-further ease, a "Hand off to Finder" affordance on the
Report screen itself that pre-selects nothing but jumps into The Finder with the plan open.
