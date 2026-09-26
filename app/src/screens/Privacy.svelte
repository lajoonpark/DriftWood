<script lang="ts">
  import { fly } from "svelte/transition";
  import { reveal } from "../lib/motion";
  import { onboarding, saveOnboarding, DEFAULT_SETTINGS } from "../lib/stores";
  import { PRIVACY_LABELS, type PrivacyTier } from "../lib/types";

  let { onNext, onBack }: { onNext: () => void; onBack: () => void } = $props();

  let s = $state(structuredClone($onboarding));

  const OPTIONS: {
    tier: PrivacyTier;
    title: string;
    body: string;
    note: string;
  }[] = [
    {
      tier: "minimal",
      title: "Minimal",
      body: "Only shapes and sizes travel — file kinds, dates, orphan status. No paths, no names, nothing personal.",
      note: "Free · nothing leaves your Mac",
    },
    {
      tier: "standard",
      title: "Standard",
      body: "Full paths and filenames are sent so the river can read context, but never a byte of file content.",
      note: "≈ $0.04–0.12 per scan",
    },
    {
      tier: "deep",
      title: "Deep",
      body: "For the genuinely ambiguous: Standard, plus a one-level folder listing (names only, capped) for trickier items.",
      note: "≈ $0.10–0.30 per scan",
    },
  ];

  function choose(tier: PrivacyTier) {
    s.privacy = tier;
    saveOnboarding(s);
  }

  const est = $derived(
    s.privacy === "minimal" ? "free" : s.privacy === "standard" ? "≈ $0.08" : "≈ $0.19",
  );
</script>

<div class="privacy">
  <div class="inner">
    <p class="kicker rv" use:reveal>Privacy &amp; cost</p>
    <h1 class="rv" use:reveal style="--rv-delay:120ms">What may the river carry?</h1>
    <p class="lead serif-lead rv" use:reveal style="--rv-delay:280ms">
      The AI that reasons about your files runs in the cloud. You choose exactly
      how much it may see — and a hard cap keeps any scan from costing more than
      a stamp.
    </p>

    <div class="options">
      {#each OPTIONS as o, i (o.tier)}
        <button
          class="card opt rv"
          class:sel={s.privacy === o.tier}
          use:reveal
          style="--rv-delay:{480 + i * 130}ms"
          onclick={() => choose(o.tier)}
        >
          <div class="opt-head">
            <span class="opt-name">{PRIVACY_LABELS[o.tier]}</span>
            <span class="opt-note">{o.note}</span>
          </div>
          <p class="opt-body">{o.body}</p>
        </button>
      {/each}
    </div>

    <p class="cap rv" use:reveal style="--rv-delay:930ms">
      Cost is capped hard at <span class="mono">${DEFAULT_SETTINGS.costCap.toFixed(2)}</span>
      per scan — if the river runs long, the rest is labeled by heuristics alone
      and marked honestly.
    </p>

    <footer class="rv" use:reveal style="--rv-delay:1_050ms">
      <button class="btn-quiet" onclick={onBack}>Back</button>
      <div class="next-group" in:fly={{ x: 0, duration: 0 }}>
        <span class="est kicker">This scan: {est}</span>
        <button class="btn btn-primary" onclick={onNext}>Wade in</button>
      </div>
    </footer>
  </div>
</div>

<style>
  .privacy {
    height: 100%;
    overflow-y: auto;
    display: flex;
    justify-content: center;
  }

  .inner {
    width: 100%;
    max-width: 680px;
    padding: 9vh 32px 8vh;
    display: flex;
    flex-direction: column;
  }

  h1 {
    font-size: clamp(34px, 4vw, 52px);
    margin: 12px 0 18px;
  }

  .lead {
    font-size: 16px;
    max-width: 580px;
    margin-bottom: 40px;
  }

  .options {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .opt {
    text-align: left;
    padding: 20px 24px;
    cursor: pointer;
    transition:
      border-color 0.4s var(--ease-out),
      transform 0.4s var(--ease-out),
      box-shadow 0.4s var(--ease-out),
      background-color 0.4s;
  }

  .opt:hover {
    transform: translateY(-2px);
  }

  .opt.sel {
    border-color: rgba(63, 102, 168, 0.45);
    background:
      linear-gradient(165deg, rgba(127, 163, 205, 0.14), rgba(217, 229, 241, 0.05) 60%),
      var(--paper-raised);
    box-shadow:
      0 1px 0 rgba(255, 255, 255, 0.7) inset,
      0 18px 40px -24px rgba(44, 77, 151, 0.5);
  }

  .opt-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 12px;
  }

  .opt-name {
    font-family: var(--font-display);
    font-variation-settings: "opsz" 30, "SOFT" 50, "WONK" 0;
    font-size: 21px;
    font-weight: 460;
  }

  .opt-note {
    font-size: 12px;
    color: var(--ink-faint);
    letter-spacing: 0.04em;
  }

  .opt-body {
    margin-top: 8px;
    font-size: 14px;
    color: var(--ink-soft);
    line-height: 1.6;
    max-width: 520px;
  }

  .cap {
    margin-top: 28px;
    font-size: 13px;
    color: var(--ink-faint);
    max-width: 520px;
  }

  footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-top: auto;
    padding-top: 40px;
  }

  .next-group {
    display: flex;
    align-items: center;
    gap: 20px;
  }

  .est {
    color: var(--river);
  }
</style>
