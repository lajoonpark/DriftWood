<script lang="ts">
  import { slide } from "svelte/transition";
  import Stamp from "../components/Stamp.svelte";
  import { bridge } from "../lib/bridge";
  import { confidenceWord, formatBytes, relDate, truncateMiddle } from "../lib/format";
  import { toast } from "../lib/stores";
  import { TIER_NAMES, type ReportEntry, type Tier } from "../lib/types";

  let {
    entry,
    tier,
    onOverride,
  }: {
    entry: ReportEntry;
    tier: Tier;
    onOverride: (id: string, tier: Tier) => void;
  } = $props();

  let expanded = $state(false);

  const SOURCE_PHRASES: Record<string, { text: string; warn?: boolean }> = {
    auto_high: { text: "scored straight to driftwood" },
    auto_low: { text: "kept safely upstream" },
    rule: { text: "pinned by your own rule" },
    llm: { text: "judged by the river" },
    fallback: { text: "snagged — heuristic guess", warn: true },
    never_flag: { text: "protected — never flagged" },
  };

  const ORPHAN_WORDS = {
    orphaned: "orphaned",
    active: "app still around",
    unknown: "unknown lineage",
  } as const;

  const phrase = $derived(SOURCE_PHRASES[entry.tier_source]);
  const c = $derived(entry.candidate);

  async function reveal() {
    bridge.reveal(c.path);
  }

  async function restamp(t: Tier) {
    if (t === tier) return;
    onOverride(c.id, t);
    await bridge.correctTier(c.id, t);
    toast("The river will remember that.");
  }
</script>

<article class="entry">
  <Stamp {tier} size={52} press title={TIER_NAMES[tier]} />

  <div class="body">
    <div class="topline">
      <span class="tier-name" style:color={`var(--t${tier})`}>{TIER_NAMES[tier]}</span>
      <span class="src" class:warn={phrase.warn}>· {phrase.text}</span>
    </div>

    <p class="path mono" title={c.path}>{truncateMiddle(c.path, 64)}</p>
    <p class="summary">{entry.summary}</p>

    <div class="meta">
      <span>{formatBytes(c.size_bytes)}</span>
      <span class="sep">·</span>
      <span>last used {relDate(c.last_used_date)}</span>
      <span class="sep">·</span>
      <span>{ORPHAN_WORDS[c.orphan_status]}</span>
      {#if entry.reasoning}
        <button class="why" onclick={() => (expanded = !expanded)}>
          <svg
            class="chev"
            class:open={expanded}
            width="11"
            height="7"
            viewBox="0 0 14 9"
            aria-hidden="true"
          >
            <path d="M1 1l6 6 6-6" fill="none" stroke="currentColor" stroke-width="1.8" />
          </svg>
          Why the river says so
        </button>
      {/if}
    </div>

    {#if expanded && entry.reasoning}
      <div class="reasoning" transition:slide={{ duration: 340 }}>
        <p>{entry.reasoning}</p>
        <p class="r-meta kicker">
          {#if entry.llm_model}{entry.llm_model} · {/if}{entry.privacy_tier_used}
          privacy · {confidenceWord(entry.confidence)}
          {#if entry.rule_id}· rule {entry.rule_id}{/if}
        </p>
      </div>
    {/if}

    <div class="row">
      <button class="btn-quiet" onclick={reveal}>Reveal in Finder</button>
      <div class="restamp">
        <span class="kicker">Re-stamp</span>
        {#each [1, 2, 3, 4] as t (t)}
          {@const stampImg =
            t === 1 ? "driftwood" : t === 2 ? "bottle" : t === 3 ? "current" : "source"}
          <button
            class="mini"
            class:cur={t === tier}
            title={TIER_NAMES[t as Tier]}
            aria-label={`Re-stamp as ${TIER_NAMES[t as Tier]}`}
            onclick={() => restamp(t as Tier)}
          >
            <img src={`/assets/stamp-${stampImg}.png`} alt="" />
          </button>
        {/each}
      </div>
    </div>
  </div>

  <div class="size num">{formatBytes(c.size_bytes)}</div>
</article>

<style>
  .entry {
    display: flex;
    gap: 20px;
    padding: 22px 4px;
    border-top: 1px solid var(--hairline-soft);
  }

  .body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 7px;
  }

  .topline {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }

  .tier-name {
    font-family: var(--font-display);
    font-variation-settings: "opsz" 24, "SOFT" 50, "WONK" 0;
    font-size: 17.5px;
    font-weight: 480;
  }

  .src {
    font-size: 12px;
    color: var(--ink-faint);
    letter-spacing: 0.04em;
  }

  .src.warn {
    color: var(--warn);
  }

  .path {
    font-size: 12.5px;
    color: var(--ink-soft);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .summary {
    font-size: 14.5px;
    color: var(--ink);
    line-height: 1.55;
    max-width: 560px;
  }

  .meta {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12.5px;
    color: var(--ink-faint);
    flex-wrap: wrap;
  }

  .sep {
    opacity: 0.5;
  }

  .why {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-left: 10px;
    font-size: 12.5px;
    color: var(--river);
    transition: color 0.25s;
  }

  .why:hover {
    color: var(--river-deep);
  }

  .chev {
    transition: transform 0.3s var(--ease-out);
  }

  .chev.open {
    transform: rotate(180deg);
  }

  .reasoning {
    margin: 6px 0 2px;
    padding: 14px 18px;
    background: rgba(255, 255, 255, 0.4);
    border: 1px solid var(--hairline-soft);
    border-left: 3px solid var(--river-mid);
    border-radius: 10px;
    max-width: 560px;
  }

  .reasoning p:first-child {
    font-size: 13.5px;
    line-height: 1.65;
    color: var(--ink-soft);
  }

  .r-meta {
    margin-top: 10px;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-top: 4px;
  }

  .restamp {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .mini {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    opacity: 0.38;
    transition:
      opacity 0.3s,
      transform 0.3s var(--ease-out),
      box-shadow 0.3s;
  }

  .mini:hover {
    opacity: 0.8;
    transform: translateY(-2px);
  }

  .mini.cur {
    opacity: 1;
    box-shadow:
      0 0 0 2px var(--paper-raised),
      0 0 0 3.5px var(--ink-faint);
  }

  .mini img {
    width: 100%;
    height: 100%;
    border-radius: 50%;
    display: block;
  }

  .size {
    color: var(--ink-faint);
    font-size: 13px;
    padding-top: 4px;
    flex: none;
  }
</style>
