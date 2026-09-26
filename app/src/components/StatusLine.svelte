<script lang="ts">
  import { fly, slide } from "svelte/transition";
  import { PHASE_LABELS, type Phase } from "../lib/types";
  import type { FeedLine } from "../lib/stores";

  let {
    phase,
    feed = [],
    running = false,
  }: { phase: Phase | null; feed?: FeedLine[]; running?: boolean } = $props();

  let open = $state(false);

  const ORDER: Phase[] = [
    "enumerating",
    "wading",
    "filtering",
    "scoring",
    "reasoning",
    "assembling",
  ];

  const done = $derived(phase ? ORDER.indexOf(phase) : -1);
</script>

<div class="status">
  {#if phase}
    <div class="past" aria-hidden="true">
      {#each ORDER.slice(0, done) as p, i (p)}
        <span class="past-label" style="--i:{i}">{PHASE_LABELS[p]}</span>
      {/each}
    </div>

    {#key phase}
      <button
        class="current"
        onclick={() => (open = !open)}
        transition:fly={{ y: 14, duration: 450 }}
      >
        <span class="label">{PHASE_LABELS[phase]}</span>
        {#if running}<span class="breath" aria-hidden="true"><i></i><i></i><i></i></span>{/if}
        <svg
          class="chev"
          class:open
          width="14"
          height="9"
          viewBox="0 0 14 9"
          aria-hidden="true"
        >
          <path d="M1 1l6 6 6-6" fill="none" stroke="currentColor" stroke-width="1.6" />
        </svg>
      </button>
    {/key}

    {#if open}
      <div class="feed" transition:slide={{ duration: 320 }}>
        {#each feed as e, i (i)}
          <p class="feed-line" class:warn={e.type === "warn"}>
            {e.message}
          </p>
        {/each}
        {#if feed.length === 0}
          <p class="feed-line">nothing to show yet</p>
        {/if}
      </div>
    {/if}
  {/if}
</div>

<style>
  .status {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    min-height: 44px;
  }

  .past {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 1px;
    max-height: 60px;
    overflow: hidden;
  }

  .past-label {
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    color: var(--ink-faint);
    opacity: calc(0.5 + var(--i) * 0.1);
    animation: settle 0.6s var(--ease-out);
  }

  @keyframes settle {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }

  .current {
    display: inline-flex;
    align-items: baseline;
    gap: 12px;
    color: var(--ink);
  }

  .label {
    font-family: var(--font-display);
    font-variation-settings: "opsz" 40, "SOFT" 60, "WONK" 1;
    font-style: italic;
    font-size: 27px;
    font-weight: 420;
    letter-spacing: -0.01em;
  }

  .chev {
    align-self: center;
    color: var(--river);
    transition: transform 0.35s var(--ease-out);
  }

  .chev.open {
    transform: rotate(180deg);
  }

  .breath {
    display: inline-flex;
    gap: 5px;
    align-self: center;
  }

  .breath i {
    width: 4.5px;
    height: 4.5px;
    border-radius: 50%;
    background: var(--river-mid);
    animation: breathe 1.5s ease-in-out infinite;
  }

  .breath i:nth-child(2) {
    animation-delay: 0.22s;
  }
  .breath i:nth-child(3) {
    animation-delay: 0.44s;
  }

  @keyframes breathe {
    0%,
    100% {
      opacity: 0.25;
      transform: translateY(0);
    }
    45% {
      opacity: 1;
      transform: translateY(-3px);
    }
  }

  .feed {
    margin-top: 10px;
    max-width: 460px;
    max-height: 130px;
    overflow-y: auto;
    padding: 12px 16px;
    border: 1px solid var(--hairline-soft);
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.35);
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .feed-line {
    font-family: ui-monospace, "SF Mono", Menlo, monospace;
    font-size: 11.5px;
    color: var(--ink-soft);
  }

  .feed-line.warn {
    color: var(--warn);
  }
</style>
