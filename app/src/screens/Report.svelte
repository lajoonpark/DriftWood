<script lang="ts">
  import { fly, fade } from "svelte/transition";
  import Entry from "../components/Entry.svelte";
  import Stamp from "../components/Stamp.svelte";
  import { reveal } from "../lib/motion";
  import { formatBytes, formatCount } from "../lib/format";
  import { scan } from "../lib/stores";
  import {
    SCOPE_LABELS,
    TIER_NAMES,
    type Report,
    type ScopeCategory,
    type Tier,
  } from "../lib/types";

  let { onRescan }: { onRescan: () => void } = $props();

  const report = $derived($scan.report as Report);

  /* Effective tiers: overrides win over the report's assignment. */
  let overrides = $state<Record<string, Tier>>({});
  let reportId = $state<string | null>(null);

  $effect(() => {
    if (report.entries[0]?.candidate.id !== reportId) {
      reportId = report.entries[0]?.candidate.id ?? null;
      overrides = {};
    }
  });

  const effTier = (id: string, base: Tier): Tier => overrides[id] ?? base;

  const groups = $derived(
    [...report.groups]
      .filter((g) => g.count > 0)
      .sort((a, b) => (a.category === "low" ? -1 : b.category === "low" ? 1 : 0)),
  );

  const entriesByCategory = $derived.by(() => {
    const map = new Map<ScopeCategory, typeof report.entries>();
    for (const cat of ["low", "medium", "high"] as ScopeCategory[]) {
      const items = report.entries
        .filter((e) => e.candidate.scope_category === cat)
        .sort((a, b) => b.candidate.score - a.candidate.score);
      if (items.length) map.set(cat, items);
    }
    return map;
  });

  const totalBytes = $derived(report.groups.reduce((n, g) => n + g.bytes, 0));

  const tierCounts = $derived.by(() => {
    const counts: Record<Tier, number> = { 1: 0, 2: 0, 3: 0, 4: 0 };
    for (const e of report.entries) counts[effTier(e.candidate.id, e.tier)]++;
    return counts;
  });

  function onOverride(id: string, tier: Tier) {
    overrides = { ...overrides, [id]: tier };
  }

  function rescan() {
    onRescan();
  }
</script>

<div class="report" in:fade={{ duration: 400 }}>
  <div class="inner">
    <header>
      <p class="kicker rv" use:reveal>Field notes</p>
      <h1 class="rv" use:reveal style="--rv-delay:120ms">What the river gave back.</h1>

      <div class="stats rv" use:reveal style="--rv-delay:320ms">
        <div class="stat main">
          <span class="kicker">Could be freed</span>
          <span class="big">{formatBytes(totalBytes)}</span>
        </div>
        <div class="rule-v"></div>
        <div class="stamps">
          {#each [1, 2, 3, 4] as t (t)}
            <div class="stat">
              <Stamp tier={t as Tier} size={34} title={TIER_NAMES[t as Tier]} />
              <span class="count num">{tierCounts[t as Tier]}</span>
            </div>
          {/each}
        </div>
        <div class="rule-v"></div>
        <div class="stat">
          <span class="kicker">Findings</span>
          <span class="mid num">{formatCount(report.entries.length)}</span>
        </div>
      </div>

      {#if report.warnings?.length}
        <div class="warnings rv" use:reveal style="--rv-delay:460ms">
          {#each report.warnings as w (w)}
            <p class="serif-lead">{w}</p>
          {/each}
        </div>
      {/if}
    </header>

    <div class="pages">
      {#each groups as g, gi (g.category)}
        <section class="group" in:fly={{ y: 18, duration: 600, delay: 200 + gi * 120 }}>
          <div class="group-head">
            <h2>{SCOPE_LABELS[g.category]}</h2>
            <span class="g-meta">
              <span class="num">{formatCount(g.count)}</span> items ·
              <span class="num">{formatBytes(g.bytes)}</span>
            </span>
          </div>
          {#each entriesByCategory.get(g.category) ?? [] as e, i (e.candidate.id)}
            <div use:reveal style={`--rv-delay:${Math.min(i * 60, 420)}ms`}>
              <Entry
                entry={e}
                tier={effTier(e.candidate.id, e.tier)}
                onOverride={onOverride}
              />
            </div>
          {/each}
        </section>
      {/each}
    </div>

    <footer class="rv" use:reveal>
      <button class="btn btn-primary" onclick={rescan}>Search the river again</button>
      <p class="foot-note">DriftWood deleted nothing. It never does.</p>
    </footer>
  </div>
</div>

<style>
  .report {
    height: 100%;
    overflow-y: auto;
  }

  .inner {
    max-width: 860px;
    margin: 0 auto;
    padding: 9vh 40px 10vh;
  }

  h1 {
    font-size: clamp(36px, 4.4vw, 56px);
    margin: 12px 0 30px;
  }

  .stats {
    display: flex;
    align-items: center;
    gap: clamp(24px, 4vw, 48px);
    padding: 18px 0;
  }

  .stat {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }

  .big {
    font-family: var(--font-display);
    font-variation-settings: "opsz" 40, "SOFT" 40, "WONK" 0;
    font-size: 40px;
    font-weight: 420;
    color: var(--river-deep);
    line-height: 1;
  }

  .mid {
    font-family: var(--font-display);
    font-size: 24px;
    color: var(--ink);
    line-height: 1;
  }

  .rule-v {
    width: 1px;
    height: 44px;
    background: var(--hairline);
  }

  .stamps {
    display: flex;
    gap: 20px;
  }

  .stamps .stat {
    align-items: center;
  }

  .count {
    font-size: 14px;
    color: var(--ink-soft);
  }

  .warnings p {
    font-size: 14px;
    margin-top: 8px;
    color: var(--warn);
  }

  .pages {
    margin-top: 26px;
    display: flex;
    flex-direction: column;
    gap: 52px;
  }

  .group-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 16px;
    padding-bottom: 10px;
    border-bottom: 1px solid var(--hairline);
  }

  h2 {
    font-size: 21px;
    font-weight: 440;
  }

  .g-meta {
    font-size: 12.5px;
    color: var(--ink-faint);
  }

  footer {
    display: flex;
    align-items: center;
    gap: 28px;
    margin-top: 60px;
  }

  .foot-note {
    font-size: 13px;
    color: var(--ink-faint);
  }
</style>
