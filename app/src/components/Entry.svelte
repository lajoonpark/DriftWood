<script lang="ts">
  import { slide } from "svelte/transition";
  import Stamp from "../components/Stamp.svelte";
  import { bridge } from "../lib/bridge";
  import { confidenceWord, lastUsedLabel, modifiedLabel, sizeLabel, truncateMiddle } from "../lib/format";
  import { toast, appSettings, scan } from "../lib/stores";
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
    // Legacy stamp from a pre-honest-tiers scan: no argument existed.
    auto_high: { text: "scored straight to driftwood — unargued (older scan)", warn: true },
    // Heuristic verdict with a deterministic explanation — still unargued
    // by AI, so the caveat must outrank the stamp.
    argued_auto_high: {
      text: "scored straight to driftwood — heuristic, unargued",
      warn: true,
    },
    auto_low: { text: "kept safely upstream" },
    rule: { text: "pinned by your own rule" },
    llm: { text: "judged by the river" },
    llm_propagated: { text: "shared verdict — its whole cluster was judged once" },
    fallback: { text: "snagged — heuristic guess", warn: true },
    // By-design unargued: the scan mode never crossed the river (Express /
    // no key). Not a snag — a failure copy would lie about what happened.
    heuristic: {
      text: "heuristic by design — the river was never asked",
      warn: true,
    },
    never_flag: { text: "protected — never flagged" },
    system_floor: { text: "system floor — DriftWood declines to opine", warn: true },
    adjudication: { text: "second opinion — argued on request, not applied" },
    not_inspected: { text: "not inspected — kept for safety", warn: true },
  };

  const ORPHAN_WORDS = {
    orphaned: "orphaned",
    active: "app still around",
    unknown: "unknown lineage",
  } as const;

  const phrase = $derived(SOURCE_PHRASES[entry.tier_source]);
  const c = $derived(entry.candidate);
  // The confidence word only means something for sources where a 0..1
  // figure is an actual judgment. For deterministic pins it means "the
  // rule fired", which is not a certainty about safety — never shown.
  const showConfidence = $derived(
    entry.tier_source === "llm" || entry.tier_source === "llm_propagated",
  );

  /* ---------- on-demand adjudication ---------- */

  type AdjudicationResult = Awaited<ReturnType<typeof bridge.adjudicate>>;
  let adj = $state<AdjudicationResult | null>(null);
  let adjBusy = $state(false);
  let adjError = $state<string | null>(null);

  async function askRiver() {
    if (adjBusy) return;
    adjBusy = true;
    adjError = null;
    adj = null;
    try {
      const key = $appSettings.apiKey.trim() || undefined;
      const result = await bridge.adjudicate(c.id, {
        model: $appSettings.model,
        apiKey: key,
        allowNonZdr: $appSettings.allowNonZdr,
      });
      adj = result;
      // Keep the report total honest in-session; the persisted report is
      // updated on disk by the backend.
      scan.addAdjudicationCost(result.cost_usd);
    } catch (err) {
      adjError = err instanceof Error ? err.message : String(err);
    } finally {
      adjBusy = false;
    }
  }

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
      <span>{sizeLabel(c)}</span>
      <span class="sep">·</span>
      <span>{lastUsedLabel(c)}</span>
      {#if modifiedLabel(c)}
        <span class="sep">·</span>
        <span>{modifiedLabel(c)}</span>
      {/if}
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
      <!-- Adjudication is available on EVERY entry, regardless of tier or
           tier_source — including auto_high, rule, and never_flag, which
           have no reasoning dropdown to hang it on. Unreadable items are
           the exception: they are never sent to the model. -->
      <button
        class="ask"
        onclick={askRiver}
        disabled={adjBusy || c.readable === false}
        title={c.readable === false
          ? "This item could not be read, so it is never sent to the model."
          : ""}
      >
        {c.readable === false
          ? "not inspectable"
          : adjBusy
            ? "asking the river…"
            : adj
              ? "ask again"
              : "Ask the river"}
      </button>
    </div>

    {#if adjError}
      <div class="adj-error" transition:slide={{ duration: 200 }}>
        <p>The river could not answer: {adjError}</p>
      </div>
    {/if}

    {#if expanded && entry.reasoning}
      <div class="reasoning" transition:slide={{ duration: 340 }}>
        <p>{entry.reasoning}</p>
        <p class="r-meta kicker">
          {#if entry.llm_model}{entry.llm_model} · {/if}{entry.privacy_tier_used}
          privacy{#if showConfidence} · {confidenceWord(entry.confidence)}{/if}
          {#if entry.rule_id}· rule {entry.rule_id}{/if}
        </p>
      </div>
    {/if}

    {#if adj}
      <!-- The card's tier and stamp never change here — this is visibly a
           second opinion, not a silent overwrite. -->
      <div class="adjudication" transition:slide={{ duration: 340 }}>
        <p class="adj-head kicker">Second opinion — argued on request, not applied</p>
        <p class="adj-verdict">
          {#if adj.agrees}
            The river agrees: it says {TIER_NAMES[adj.llm_tier]} too, and this card says{" "}
            {TIER_NAMES[adj.card_tier]}.
          {:else}
            The river says {TIER_NAMES[adj.llm_tier]}. This card says{" "}
            {TIER_NAMES[adj.card_tier]}.
            {#if adj.llm_safer}
              <strong>The river's verdict is the safer one.</strong>
            {/if}
          {/if}
        </p>
        <p>{adj.reasoning}</p>
        <p class="r-meta kicker">
          {adj.model} · {entry.privacy_tier_used} privacy · {confidenceWord(adj.confidence)}
          {#if adj.cost_usd > 0}· ${adj.cost_usd.toFixed(4)} spent{/if}
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

  <div class="size num">{sizeLabel(c)}</div>
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

  .ask {
    margin-left: 10px;
    font-size: 12.5px;
    color: var(--ink-faint);
    letter-spacing: 0.04em;
    border-bottom: 1px dotted var(--ink-faint);
    transition: color 0.25s;
  }

  .ask:hover:not(:disabled) {
    color: var(--river);
    border-bottom-color: var(--river);
  }

  .ask:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .adj-error {
    margin: 6px 0 2px;
    padding: 10px 16px;
    border-left: 3px solid var(--warn);
    background: rgba(255, 255, 255, 0.4);
    border-radius: 10px;
    max-width: 560px;
    font-size: 13px;
    color: var(--ink-soft);
  }

  .adjudication {
    margin: 6px 0 2px;
    padding: 14px 18px;
    background: rgba(255, 255, 255, 0.4);
    border: 1px solid var(--hairline-soft);
    border-left: 3px solid var(--warn);
    border-radius: 10px;
    max-width: 560px;
  }

  .adjudication > p:not(.adj-head):not(.adj-verdict) {
    font-size: 13.5px;
    line-height: 1.65;
    color: var(--ink-soft);
  }

  .adj-head {
    color: var(--warn);
    margin-bottom: 6px;
  }

  .adj-verdict {
    font-size: 13.5px;
    color: var(--ink);
    margin-bottom: 8px;
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
