<script lang="ts">
  import { fly, fade } from "svelte/transition";
  import { bridge, type FdaStatus } from "../lib/bridge";
  import { reveal } from "../lib/motion";

  let { onNext, onBack }: { onNext: () => void; onBack: () => void } = $props();

  let status = $state<FdaStatus | "checking" | null>(null);

  async function check() {
    status = "checking";
    try {
      status = await withTimeout(bridge.checkFullDiskAccess(), 10_000);
    } catch {
      status = "unknown";
    }
  }

  function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
    return Promise.race([
      p,
      new Promise<T>((_, reject) => setTimeout(() => reject(new Error("timeout")), ms)),
    ]);
  }

  const granted = $derived(status === "granted");
</script>

<div class="trust">
  <div class="inner">
    <p class="kicker rv" use:reveal>Before we start</p>
    <h1 class="rv" use:reveal style="--rv-delay:120ms">One permission, asked honestly.</h1>

    <div class="points">
      <div class="point rv" use:reveal style="--rv-delay:320ms">
        <span class="num-mark">i</span>
        <p>
          <strong>What it unlocks.</strong> macOS hides your Library folders —
          where most driftwood collects. Full Disk Access lets DriftWood read
          file <em>metadata</em> there: names, sizes, dates. Never file contents.
        </p>
      </div>
      <div class="point rv" use:reveal style="--rv-delay:470ms">
        <span class="num-mark">ii</span>
        <p>
          <strong>Read-only, forever.</strong> DriftWood cannot delete, move, or
          modify a single file. It reports; you decide. It's open source, so you
          can verify that yourself.
        </p>
      </div>
      <div class="point rv" use:reveal style="--rv-delay:620ms">
        <span class="num-mark">iii</span>
        <p>
          <strong>What leaves your Mac.</strong> Only structural metadata travels
          to the AI, over providers contractually forbidden from storing it. On
          Minimal privacy, nothing leaves at all.
        </p>
      </div>
    </div>

    <div class="check rv" use:reveal style="--rv-delay:820ms">
      {#if status === null}
        <button class="btn btn-ghost" onclick={check}>Check for access</button>
        <div class="upfront">
          <p class="steps">
            Rather grant it first? System Settings → Privacy &amp; Security →
            Full Disk Access → add DriftWood.
          </p>
          <button class="btn-quiet" onclick={() => bridge.openSystemSettings()}>
            Open System Settings
          </button>
        </div>
      {:else if status === "checking"}
        <div class="pending" in:fade>
          <span class="ripple"></span>
          <span class="serif-lead">Testing the water…</span>
        </div>
      {:else if granted}
        <div class="verdict ok" in:fly={{ y: 10, duration: 400 }}>
          <span class="dot"></span>
          <span class="serif-lead">Clear water — access already granted.</span>
        </div>
      {:else}
        <div class="verdict" in:fly={{ y: 10, duration: 400 }}>
          <span class="dot deny"></span>
          <div>
            <p class="serif-lead">Snagged on the shore — not for long.</p>
            <p class="steps">
              System Settings → Privacy &amp; Security → Full Disk Access → add
              DriftWood. Then check again.
            </p>
            <div class="verdict-actions">
              <button class="btn btn-ghost" onclick={() => bridge.openSystemSettings()}>
                Open System Settings
              </button>
              <button class="btn-quiet" onclick={check}>Check again</button>
            </div>
          </div>
        </div>
      {/if}
    </div>

    <footer class="rv" use:reveal style="--rv-delay:950ms">
      <button class="btn-quiet" onclick={onBack}>Back</button>
      <button class="btn btn-primary" onclick={onNext}>
        {granted ? "That's everything — continue" : "Continue anyway"}
      </button>
    </footer>
  </div>
</div>

<style>
  .trust {
    height: 100%;
    overflow-y: auto;
    display: flex;
    justify-content: center;
  }

  .inner {
    width: 100%;
    max-width: 660px;
    padding: 10vh 32px 8vh;
    display: flex;
    flex-direction: column;
  }

  h1 {
    font-size: clamp(34px, 4vw, 52px);
    margin: 12px 0 40px;
  }

  .points {
    display: flex;
    flex-direction: column;
    gap: 22px;
  }

  .point {
    display: flex;
    gap: 18px;
    max-width: 600px;
  }

  .num-mark {
    font-family: var(--font-display);
    font-style: italic;
    color: var(--river);
    font-size: 17px;
    min-width: 22px;
    text-align: center;
    border-bottom: 1px solid var(--river-mid);
    height: fit-content;
    padding-bottom: 2px;
  }

  .point p {
    font-size: 14.5px;
    color: var(--ink-soft);
    line-height: 1.6;
  }

  .point strong {
    color: var(--ink);
    font-weight: 650;
  }

  .check {
    margin: 44px 0 8px;
    min-height: 64px;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 14px;
  }

  .upfront {
    display: flex;
    align-items: center;
    gap: 18px;
  }

  .upfront .steps {
    margin-top: 0;
  }

  .pending {
    display: flex;
    align-items: center;
    gap: 14px;
  }

  .ripple {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    border: 2px solid var(--river-mid);
    animation: ripple 1.6s ease-out infinite;
  }

  @keyframes ripple {
    from {
      transform: scale(0.4);
      opacity: 1;
    }
    to {
      transform: scale(1.5);
      opacity: 0.1;
    }
  }

  .verdict {
    display: flex;
    gap: 14px;
    align-items: flex-start;
  }

  .dot {
    width: 10px;
    height: 10px;
    margin-top: 7px;
    border-radius: 50%;
    background: var(--good);
    box-shadow: 0 0 0 4px rgba(78, 125, 91, 0.15);
    flex: none;
  }

  .dot.deny {
    background: var(--warn);
    box-shadow: 0 0 0 4px rgba(168, 85, 47, 0.15);
  }

  .steps {
    font-size: 13.5px;
    color: var(--ink-soft);
    margin-top: 6px;
    max-width: 420px;
  }

  .verdict-actions {
    display: flex;
    align-items: center;
    gap: 18px;
    margin-top: 14px;
  }

  footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-top: auto;
    padding-top: 40px;
  }
</style>
