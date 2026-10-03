<script lang="ts">
  import { fade } from "svelte/transition";
  import Stamp from "../components/Stamp.svelte";
  import { reveal } from "../lib/motion";
  import { formatBytes, formatCount, lastUsedLabel, sizeLabel, truncateMiddle } from "../lib/format";
  import { scan, toast } from "../lib/stores";
  import { bridge } from "../lib/bridge";
  import {
    SCOPE_LABELS,
    TIER_BLURBS,
    TIER_NAMES,
    type HandoffItem,
    type HandoffPlan,
    type RevealError,
    type RevealSummary,
    type Report,
    type ReportEntry,
    type ScopeCategory,
    type Tier,
  } from "../lib/types";

  let { onBack }: { onBack: () => void } = $props();

  /* The report may not be in the store yet (deep link, fresh app): pull
   * the last persisted one through the bridge — the mock answers so the
   * screen stays fully reviewable in the browser. */
  const stored = $derived($scan.report);
  let loaded = $state<Report | null>(null);
  $effect(() => {
    if (!stored && !loaded) {
      void bridge.getLastReport().then((r) => {
        if (r) loaded = r;
      });
    }
  });

  const report = $derived(stored ?? loaded);

  /* ---------- read-only browser state ---------- */

  /** Virtual, not physical: the four tiers are sections in this screen.
   *  Nothing on disk is copied, linked, moved, or deleted — the only
   *  action is handing a selection to Finder, where the user deletes. */
  let selected = $state<Set<string>>(new Set());
  /** Personal-risk cross-cut filter. */
  let scopeFilter = $state<ScopeCategory | "all">("all");
  /** How many rows each section renders (windowed lists — reports can
   *  carry thousands of entries; naive full rendering would choke). */
  const PAGE = 60;
  let visible = $state<Record<number, number>>({ 1: PAGE, 2: PAGE, 3: PAGE, 4: PAGE });
  let reportId = $state<string | null>(null);

  $effect(() => {
    // A new report resets the browser's local state.
    const firstId = report?.entries[0]?.candidate.id ?? null;
    if (firstId !== reportId) {
      reportId = firstId;
      selected = new Set();
      scopeFilter = "all";
      visible = { 1: PAGE, 2: PAGE, 3: PAGE, 4: PAGE };
      showAllContainers = false;
    }
  });

  const sections = $derived.by(() => {
    if (!report) return [];
    const byTier = new Map<Tier, ReportEntry[]>();
    for (const e of report.entries) {
      let list = byTier.get(e.tier as Tier);
      if (!list) byTier.set(e.tier as Tier, (list = []));
      list.push(e);
    }
    return ([1, 2, 3, 4] as Tier[])
      .filter((t) => byTier.has(t))
      .map((t) => {
        const all = byTier.get(t)!.sort((a, b) => b.candidate.score - a.candidate.score);
        const shown = scopeFilter === "all" ? all : all.filter((e) => e.candidate.scope_category === scopeFilter);
        return {
          tier: t,
          all,
          shown,
          bytes: all.reduce((n, e) => n + e.candidate.size_bytes, 0),
        };
      });
  });

  const allShownIds = $derived(sections.flatMap((s) => s.shown.map((e) => e.candidate.id)));

  const selection = $derived.by(() => {
    if (!report) return { count: 0, bytes: 0, hasSource: false };
    const entries = report.entries.filter((e) => selected.has(e.candidate.id));
    return {
      count: entries.length,
      bytes: entries.reduce((n, e) => n + e.candidate.size_bytes, 0),
      hasSource: entries.some((e) => e.tier === 4),
    };
  });

  function toggle(id: string) {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    selected = next;
  }

  function toggleSection(tier: Tier) {
    const ids = sections.find((s) => s.tier === tier)!.shown.map((e) => e.candidate.id);
    const allOn = ids.every((id) => selected.has(id));
    const next = new Set(selected);
    if (allOn) for (const id of ids) next.delete(id);
    else for (const id of ids) next.add(id);
    selected = next;
  }

  function sectionAllSelected(tier: Tier): boolean {
    const ids = sections.find((s) => s.tier === tier)!.shown.map((e) => e.candidate.id);
    return ids.length > 0 && ids.every((id) => selected.has(id));
  }

  function showMore(tier: Tier) {
    visible = { ...visible, [tier]: (visible[tier] ?? PAGE) + PAGE };
  }

  /* ---------- the hand-off ---------- */

  /** The selection as hand-off items: path plus tier and personal-risk
   *  scope, so the container view can judge what a folder aggregates. */
  const selectedItems = $derived.by(() => {
    if (!report) return [] as HandoffItem[];
    return report.entries
      .filter((e) => selected.has(e.candidate.id))
      .map((e) => ({
        path: e.candidate.path,
        size_bytes: e.candidate.size_bytes,
        tier: e.tier,
        scope: e.candidate.scope_category,
      }));
  });

  /** Pre-commit plan from the shell: containers after grouping, fold, and
   *  guarded rollup, with findings-versus-total and vanished paths. */
  let plan = $state<HandoffPlan | null>(null);
  let planning = $state(false);
  let planToken = 0;
  $effect(() => {
    const items = selectedItems;
    // A changed selection invalidates any previous hand-off outcome.
    lastResult = null;
    handoffError = null;
    if (items.length === 0) {
      plan = null;
      planning = false;
      return;
    }
    planning = true;
    const token = ++planToken;
    const t = setTimeout(() => {
      bridge
        .planHandoff(items)
        .then((p) => {
          if (token === planToken) {
            plan = p;
            planning = false;
          }
        })
        .catch(() => {
          if (token === planToken) {
            plan = null;
            planning = false;
            handoffError = "DriftWood could not measure the folders — try selecting again.";
          }
        });
    }, 250);
    return () => clearTimeout(t);
  });

  /** How many container rows show before "show all" — bounded like every
   *  other list on this screen. */
  const CONTAINER_PAGE = 12;
  let showAllContainers = $state(false);

  /** Below this findings-to-total share, a folder gets the ⌘A warning. */
  const LOW_RATIO = 0.5;
  function ratioWarning(findings: number, total: number | null): string | null {
    if (total === null || total === 0 || findings >= total) return null;
    if (findings / total >= LOW_RATIO) return null;
    return `Only ${formatCount(findings)} of ${formatCount(total)} items in this folder ${findings === 1 ? "is" : "are"} driftwood — ⌘A or a stray select-all would reach far beyond your selection.`;
  }

  let handing = $state(false);
  let lastResult = $state<RevealSummary | null>(null);
  let handoffError = $state<string | null>(null);
  /** macOS refused the Finder consent (-1743): a calm explanation, not a
   *  dead button. Stays until a hand-off succeeds. */
  let automationDenied = $state(false);

  async function handOff(folders?: string[]) {
    if (!report || handing || selection.count === 0) return;
    handing = true;
    handoffError = null;
    try {
      const sum = await bridge.revealAll(selectedItems, folders);
      lastResult = sum;
      let msg = `Finder has ${formatCount(sum.items_selected)} of ${formatCount(sum.items_requested)} findings preselected across ${sum.windows} window${sum.windows === 1 ? "" : "s"} — press ⌘Delete there. DriftWood deleted nothing.`;
      if (sum.items_selected !== sum.items_requested) {
        msg += " Some selections didn't take — see the hand-off report.";
      }
      if (sum.skipped.length > 0) {
        msg += ` ${formatCount(sum.skipped.length)} path${sum.skipped.length === 1 ? " was" : "s were"} already gone.`;
      }
      toast(msg);
    } catch (e) {
      if (e && typeof e === "object" && (e as RevealError).kind === "automation_denied") {
        automationDenied = true;
      } else {
        handoffError =
          e && typeof e === "object" && "message" in e
            ? String((e as { message: unknown }).message)
            : "The hand-off to Finder did not go through.";
      }
    } finally {
      handing = false;
    }
  }
</script>

<div class="browser" in:fade={{ duration: 400 }}>
  {#if !report}
    <div class="inner empty">
      <p class="kicker rv" use:reveal>The Finder</p>
      <h1 class="rv" use:reveal style="--rv-delay:120ms">No report yet.</h1>
      <p class="serif-lead rv" use:reveal style="--rv-delay:260ms">
        Search the river first — everything it finds shows up here.
      </p>
      <div class="actions rv" use:reveal style="--rv-delay:380ms">
        <button class="btn btn-primary" onclick={onBack}>Back</button>
      </div>
    </div>
  {:else}
  <div class="inner">
    <header>
      <p class="kicker rv" use:reveal>The Finder</p>
      <h1 class="rv" use:reveal style="--rv-delay:120ms">Everything the river found, in one place.</h1>
      <p class="serif-lead rv" use:reveal style="--rv-delay:260ms">
        A read-only view of your report — nothing here is copied, linked, moved, or
        deleted. Select what you're done with, hand it to Finder, and press
        ⌘Delete there yourself.
      </p>

      <div class="stats rv" use:reveal style="--rv-delay:380ms">
        <div class="stat">
          <span class="kicker">Findings</span>
          <span class="mid num">{formatCount(report.entries.length)}</span>
        </div>
        <div class="rule-v"></div>
        <div class="stat">
          <span class="kicker">Selection</span>
          <span class="mid num">
            {formatCount(selection.count)} · {formatBytes(selection.bytes)}
          </span>
        </div>
        <div class="rule-v"></div>
        <div class="stat filters">
          <span class="kicker">Personal risk</span>
          <div class="seg">
            {#each ["all", "low", "medium", "high"] as sc (sc)}
              <button
                class="seg-btn"
                class:sel={scopeFilter === sc}
                onclick={() => (scopeFilter = sc as ScopeCategory | "all")}
              >
                {sc === "all" ? "All" : SCOPE_LABELS[sc as ScopeCategory].replace(" personal risk", "")}
              </button>
            {/each}
          </div>
        </div>
      </div>
    </header>

    <div class="pages">
      {#each sections as s, gi (s.tier)}
        <section class="group" class:source-group={s.tier === 4} in:fade={{ duration: 500, delay: 120 + gi * 90 }}>
          <div class="group-head">
            <div class="gh-left">
              <Stamp tier={s.tier} size={40} title={TIER_NAMES[s.tier]} />
              <div class="gh-text">
                <h2 style:color={`var(--t${s.tier})`}>{TIER_NAMES[s.tier]}</h2>
                <p class="blurb">{TIER_BLURBS[s.tier]}</p>
              </div>
            </div>
            <div class="gh-right">
              <span class="g-meta num">
                {formatCount(s.shown.length)}{s.shown.length !== s.all.length ? ` of ${formatCount(s.all.length)}` : ""} · {formatBytes(s.bytes)}
              </span>
              {#if s.tier !== 4}
                <!-- No select-all on Source: a blanket hand-off of tier 4
                     is exactly what this screen must not encourage. -->
                <label class="selall">
                  <input
                    type="checkbox"
                    checked={sectionAllSelected(s.tier)}
                    onchange={() => toggleSection(s.tier)}
                  />
                  <span>Select all shown</span>
                </label>
              {/if}
            </div>
          </div>

          {#if s.tier === 4}
            <p class="source-note">
              Source items are personal or irreplaceable. They're listed so you can see
              exactly what stayed dry — not as candidates.
            </p>
          {/if}

          <ul class="rows">
            {#each s.shown.slice(0, visible[s.tier] ?? PAGE) as e (e.candidate.id)}
              {@const c = e.candidate}
              <li
                class="row"
                class:picked={selected.has(c.id)}
                class:high-risk={c.scope_category === "high"}
              >
                <label class="pick">
                  <input type="checkbox" checked={selected.has(c.id)} onchange={() => toggle(c.id)} />
                </label>
                <span class="r-main">
                  <span class="r-name mono">{truncateMiddle(c.path, 72)}</span>
                  <span class="r-sub">
                    {SCOPE_LABELS[c.scope_category]} · {lastUsedLabel(c)} · {sizeLabel(c)}
                    {#if e.tier_source === "llm_propagated"}
                      · shared cluster judgment
                    {:else if e.tier_source === "fallback" || e.tier_source === "heuristic"}
                      · heuristic estimate
                    {/if}
                  </span>
                </span>
              </li>
            {/each}
          </ul>

          {#if s.shown.length > (visible[s.tier] ?? PAGE)}
            <button class="btn-quiet more" onclick={() => showMore(s.tier)}>
              Show {formatCount(Math.min(PAGE, s.shown.length - (visible[s.tier] ?? PAGE)))} more of {formatCount(s.shown.length)}
            </button>
          {/if}
        </section>
      {/each}
    </div>

    {#if selection.count > 0}
      <section class="handoff-panel" in:fade={{ duration: 400 }}>
        <div class="hp-head">
          <p class="kicker">The hand-off</p>
          <p class="serif-lead">
            {#if plan}
              Your {formatCount(selection.count)} findings live in
              {formatCount(plan.groups.length)} folder{plan.groups.length === 1 ? "" : "s"}.
              Each folder opens as one Finder window with exactly those findings
              preselected — press ⌘Delete there yourself.
            {:else if planning}
              Measuring the folders your selection lives in…
            {/if}
          </p>
        </div>

        {#if automationDenied}
          <div class="hp-denied">
            <p class="serif-lead">
              <strong>macOS needs a yes from you.</strong> DriftWood asks to control Finder —
              that is how it opens windows with your findings preselected. It uses that
              permission for nothing else, and it still deletes nothing itself.
              System Settings → Privacy &amp; Security → Automation → Finder → DriftWood.
            </p>
            <div class="hp-denied-actions">
              <button class="btn btn-ghost" onclick={() => bridge.openAutomationSettings()}>
                Open Automation Settings
              </button>
              <button class="btn-quiet" disabled={handing} onclick={() => handOff()}>
                Try the hand-off again
              </button>
            </div>
          </div>
        {:else if plan}
          {#if plan.skipped.length > 0}
            <p class="hpg-warn">
              {formatCount(plan.skipped.length)} finding{plan.skipped.length === 1 ? " has" : "s have"}
              vanished since the scan and will be skipped — caches go when their own apps clear them.
            </p>
          {/if}
          <ul class="hp-groups">
            {#each (showAllContainers ? plan.groups : plan.groups.slice(0, CONTAINER_PAGE)) as g (g.folder)}
              <li class="hp-group" class:tainted={g.tainted}>
                <div class="hpg-text">
                  <span class="hpg-folder mono" title={g.folder}>{truncateMiddle(g.folder, 60)}</span>
                  <span class="hpg-meta num">
                    {#if g.total_in_folder === null}
                      total unknown ·
                    {:else}
                      {formatCount(g.total_in_folder)} item{g.total_in_folder === 1 ? "" : "s"} in this folder ·
                    {/if}
                    {formatCount(g.findings)} {g.findings === 1 ? "is" : "are"} driftwood · {formatBytes(g.finding_bytes)} of findings
                  </span>
                  {#if ratioWarning(g.findings, g.total_in_folder)}
                    <p class="hpg-warn">{ratioWarning(g.findings, g.total_in_folder)}</p>
                  {/if}
                  {#if g.tainted}
                    <p class="hpg-warn">This folder holds Source items — {TIER_BLURBS[4]}</p>
                  {/if}
                </div>
                <button
                  class="btn-quiet hpg-btn"
                  disabled={handing || automationDenied}
                  onclick={() => handOff([g.folder])}
                >
                  Hand off this folder
                </button>
              </li>
            {/each}
          </ul>
          {#if plan.groups.length > CONTAINER_PAGE && !showAllContainers}
            <button class="btn-quiet more" onclick={() => (showAllContainers = true)}>
              Show all {formatCount(plan.groups.length)} folders
            </button>
          {/if}
        {/if}

        {#if handoffError}
          <p class="hpg-warn">{handoffError}</p>
        {/if}

        {#if lastResult}
          <div class="hp-result">
            <p class="kicker">What Finder reports</p>
            {#each lastResult.groups as g (g.folder)}
              <p class="hp-line">
                <span class="mono">{truncateMiddle(g.folder, 48)}</span> —
                {#if g.ok}
                  {formatCount(g.selected)} of {formatCount(g.requested)} preselected.
                {:else}
                  <span class="warn-text">
                    {g.error ??
                      `Finder selected ${formatCount(g.selected)} of ${formatCount(g.requested)} — a mismatch.`}
                  </span>
                {/if}
              </p>
            {/each}
            {#each lastResult.skipped as s (s.path)}
              <p class="hp-line">
                <span class="mono">{truncateMiddle(s.path, 48)}</span> — skipped: {s.reason}
              </p>
            {/each}
          </div>
        {/if}
      </section>
    {/if}

    <footer class="rv" use:reveal>
      <button class="btn-quiet" onclick={onBack}>Back to the report</button>
      <div class="handoff">
        {#if selection.count > 0 && selection.hasSource}
          <p class="warn-note">Your selection includes Source items — double-check each one.</p>
        {/if}
        <button
          class="btn btn-primary"
          disabled={selection.count === 0 || handing || automationDenied}
          onclick={() => handOff()}
        >
          {handing
            ? "Opening Finder…"
            : selection.count === 0
              ? "Show these in Finder"
              : plan && plan.groups.length > 0
                ? `Open ${formatCount(plan.groups.length)} folders in Finder (${formatCount(selection.count)} findings)`
                : `Show ${formatCount(selection.count)} in Finder (${formatBytes(selection.bytes)})`}
        </button>
        <p class="foot-note">
          A handoff, not a deletion — DriftWood opens your folders in Finder with exactly its
          findings preselected, and you press ⌘Delete there. DriftWood deleted nothing. It never
          does.
        </p>
      </div>
    </footer>
  </div>
  {/if}
</div>

<style>
  .browser {
    height: 100%;
    overflow-y: auto;
  }

  .inner {
    max-width: 860px;
    margin: 0 auto;
    padding: 9vh 40px 10vh;
  }

  .empty {
    text-align: center;
  }

  .empty h1 {
    font-size: clamp(34px, 4.2vw, 52px);
    margin: 12px 0 18px;
  }

  .empty .actions {
    margin-top: 30px;
  }

  h1 {
    font-size: clamp(34px, 4.2vw, 52px);
    margin: 12px 0 18px;
  }

  .serif-lead {
    font-size: 15px;
    max-width: 640px;
    color: var(--ink-soft);
  }

  .stats {
    display: flex;
    align-items: center;
    gap: clamp(20px, 3.4vw, 40px);
    padding: 20px 0 4px;
    flex-wrap: wrap;
  }

  .stat {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .mid {
    font-family: var(--font-display);
    font-size: 22px;
    color: var(--ink);
    line-height: 1;
  }

  .rule-v {
    width: 1px;
    height: 40px;
    background: var(--hairline);
  }

  .seg {
    display: inline-flex;
    border: 1px solid var(--hairline);
    border-radius: 999px;
    padding: 2px;
    gap: 1px;
    background: rgba(255, 255, 255, 0.3);
  }

  .seg-btn {
    padding: 6px 14px;
    border-radius: 999px;
    font-size: 12px;
    font-weight: 600;
    color: var(--ink-soft);
    transition: background-color 0.25s, color 0.25s;
  }

  .seg-btn:hover {
    color: var(--ink);
  }

  .seg-btn.sel {
    color: #f5f1e6;
    background: linear-gradient(160deg, #4873b4 0%, var(--river-deep) 100%);
  }

  .pages {
    margin-top: 22px;
    display: flex;
    flex-direction: column;
    gap: 44px;
  }

  .group-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    padding-bottom: 12px;
    border-bottom: 1px solid var(--hairline);
  }

  .gh-left {
    display: flex;
    align-items: center;
    gap: 14px;
  }

  .gh-text h2 {
    font-size: 20px;
    font-weight: 460;
  }

  .blurb {
    font-size: 12.5px;
    color: var(--ink-faint);
    margin-top: 2px;
  }

  .gh-right {
    display: flex;
    align-items: center;
    gap: 18px;
  }

  .g-meta {
    font-size: 12.5px;
    color: var(--ink-faint);
  }

  .selall {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-size: 12.5px;
    color: var(--ink-soft);
    cursor: pointer;
    white-space: nowrap;
  }

  .source-note {
    font-size: 12.5px;
    color: var(--warn);
    margin-top: 10px;
  }

  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 8px;
    border-bottom: 1px solid var(--hairline-soft);
    border-radius: 8px;
    transition: background-color 0.2s;
  }

  .row.picked {
    background: rgba(63, 102, 168, 0.1);
  }

  .row.high-risk .r-sub {
    color: var(--warn);
  }

  .pick {
    flex: none;
    display: flex;
    align-items: center;
  }

  .pick input {
    accent-color: var(--river-deep);
    width: 15px;
    height: 15px;
  }

  .r-main {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .r-name {
    font-size: 12.5px;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .r-sub {
    font-size: 11.5px;
    color: var(--ink-faint);
  }

  .more {
    margin-top: 12px;
    font-size: 13px;
  }

  footer {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 28px;
    margin-top: 54px;
  }

  /* ---------- the hand-off panel ---------- */

  .handoff-panel {
    margin-top: 54px;
    border-top: 1px solid var(--hairline);
    padding-top: 26px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .hp-head .serif-lead {
    margin-top: 6px;
    max-width: 640px;
  }

  .hp-groups {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .hp-group {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 18px;
    padding: 13px 8px;
    border-bottom: 1px solid var(--hairline-soft);
    border-radius: 8px;
    transition: background-color 0.2s;
  }

  .hp-group.tainted {
    background: rgba(168, 85, 47, 0.07);
  }

  .hpg-text {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .hpg-folder {
    font-size: 12.5px;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .hpg-meta {
    font-size: 11.5px;
    color: var(--ink-faint);
  }

  .hpg-warn {
    font-size: 12px;
    color: var(--warn);
    margin-top: 2px;
  }

  .hpg-btn {
    flex: none;
  }

  .hp-denied {
    border: 1px solid var(--hairline);
    border-left: 3px solid var(--warn);
    border-radius: 10px;
    padding: 16px 18px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    max-width: 640px;
  }

  .hp-denied-actions {
    display: flex;
    align-items: center;
    gap: 16px;
  }

  .hp-result {
    border-top: 1px dashed var(--hairline);
    padding-top: 14px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .hp-line {
    font-size: 12px;
    color: var(--ink-soft);
  }

  .warn-text {
    color: var(--warn);
  }

  .handoff {
    max-width: 460px;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 10px;
    text-align: right;
  }

  .warn-note {
    font-size: 12.5px;
    color: var(--warn);
  }

  .foot-note {
    font-size: 12.5px;
    color: var(--ink-faint);
  }

  footer .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
</style>
