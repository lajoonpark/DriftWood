<script lang="ts">
  import { fade, fly } from "svelte/transition";
  import RiverArt from "../components/RiverArt.svelte";
  import StatusLine from "../components/StatusLine.svelte";
  import Counter from "../components/Counter.svelte";
  import { bridge } from "../lib/bridge";
  import { formatBytes, formatCount } from "../lib/format";
  import { onboarding, scan, toast, SCOPE_FOLDERS, appSettings } from "../lib/stores";
  import { PRIVACY_LABELS, type Report, type ScanConfig } from "../lib/types";

  let { onDone }: { onDone: () => void } = $props();

  let error = $state<string | null>(null);
  let finishing = $state(false);

  const s = $derived($onboarding);

  const cfg = $derived.by<ScanConfig>(() => {
    const scopes = (["low", "medium", "high"] as const).filter((cat) =>
      SCOPE_FOLDERS[cat].some((f) => s.folders[f.path]),
    );
    return {
      scopes,
      privacy_tier: s.privacy,
      stage2: true,
      model: $appSettings.model,
      api_key: $appSettings.apiKey.trim() || undefined,
      cost_cap_usd: $appSettings.costCap,
    };
  });

  const scopeSummary = $derived(
    cfg.scopes.length === 0
      ? "no waters chosen"
      : `${cfg.scopes.length} area${cfg.scopes.length === 1 ? "" : "s"} · ${
          PRIVACY_LABELS[cfg.privacy_tier]
        } privacy`,
  );

  const st = $derived($scan);

  async function start() {
    error = null;
    scan.reset();
    scan.setRunning(true);
    try {
      const report: Report = await bridge.startScan(cfg, (e) => scan.applyEvent(e));
      scan.setReport(report);
      finishing = true;
      setTimeout(onDone, 1_100);
    } catch (err) {
      scan.setRunning(false);
      const msg = err instanceof Error ? err.message : String(err);
      if (msg === "cancelled") {
        toast("The scan drifted back ashore.");
      } else {
        error = msg;
      }
    }
  }

  function cancel() {
    bridge.cancelScan();
  }

  const intensity = $derived(st.running ? 2 : 1);
</script>

<div class="scan">
  <RiverArt variant="horizontal" {intensity} />

  {#if error}
    <div class="snagged" in:fade={{ duration: 500 }}>
      <img class="snag-art" src="/assets/snagged.png" alt="" />
      <div class="snag-copy">
        <p class="kicker">The scan ran aground</p>
        <h1>Snagged.</h1>
        <p class="serif-lead">{error}</p>
        <div class="actions">
          <button class="btn btn-primary" onclick={start}>Try the crossing again</button>
          <button class="btn-quiet" onclick={() => (error = null)}>Back to the bank</button>
        </div>
      </div>
    </div>
  {:else if st.running || finishing}
    <div class="running" in:fade={{ duration: 600 }}>
      <div class="status-wrap">
        <StatusLine phase={st.phase} feed={st.feed} running={st.running} />
      </div>

      <div class="counters" out:fade={{ duration: 250 }}>
        <div class="counter">
          <span class="c-label kicker">Files searched</span>
          <span class="c-value"><Counter value={st.filesSearched} fmt={formatCount} /></span>
        </div>
        <div class="counter">
          <span class="c-label kicker">River walked</span>
          <span class="c-value"><Counter value={st.bytesSearched} fmt={(v) => formatBytes(v)} /></span>
        </div>
        <div class="counter hero">
          <span class="c-label kicker">Recoverable so far</span>
          <span class="c-value big"><Counter value={st.recoverableBytes} fmt={(v) => formatBytes(v)} duration={1_200} /></span>
        </div>
      </div>

      {#if st.running && !finishing}
        <button class="btn-quiet cancel" onclick={cancel}>Pull ashore</button>
      {/if}
    </div>
  {:else}
    <div class="idle">
      <div class="hero-copy">
        <p class="kicker rv" in:fade={{ duration: 600, delay: 150 }}>Ready when you are</p>
        <h1 in:fly={{ y: 22, duration: 700, delay: 280 }}>Search the river?</h1>
        <p class="scope-line" in:fade={{ duration: 600, delay: 560 }}>
          {scopeSummary} · cap ${$appSettings.costCap.toFixed(2)}
        </p>
        <div class="actions" in:fly={{ y: 14, duration: 600, delay: 780 }}>
          <button class="btn btn-primary cta" onclick={start}>Search the river</button>
          {#if st.report}
            <button class="btn-quiet" onclick={onDone}>Read the last report →</button>
          {/if}
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .scan {
    position: relative;
    height: 100%;
    overflow: hidden;
  }

  /* ---------- idle ---------- */

  .idle {
    position: relative;
    height: 100%;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    text-align: center;
  }

  .hero-copy {
    margin-top: 20vh;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
  }

  h1 {
    font-size: clamp(44px, 5.6vw, 76px);
    margin: 10px 0 4px;
  }

  .scope-line {
    font-size: 13px;
    color: var(--ink-faint);
    letter-spacing: 0.06em;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 26px;
    margin-top: 30px;
  }

  .cta {
    padding: 15px 38px;
    font-size: 15.5px;
  }

  /* ---------- running ---------- */

  .running {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
  }

  /* Morning mist rising off the water — keeps the counters readable
     while the river runs at full current beneath them. */
  .running::after {
    content: "";
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 36vh;
    background: linear-gradient(
      to bottom,
      rgba(243, 237, 222, 0) 0%,
      rgba(243, 237, 222, 0.5) 42%,
      rgba(243, 237, 222, 0.88) 100%
    );
    pointer-events: none;
  }

  .running > * {
    position: relative;
    z-index: 1;
  }

  .status-wrap {
    margin-top: 16vh;
    display: flex;
    justify-content: center;
    width: 100%;
  }

  .counters {
    margin-top: auto;
    margin-bottom: 13vh;
    display: flex;
    align-items: flex-end;
    gap: clamp(36px, 6vw, 90px);
  }

  .counter {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
  }

  .c-value {
    font-family: var(--font-display);
    font-variation-settings: "opsz" 30, "SOFT" 40, "WONK" 0;
    font-size: 24px;
    font-weight: 420;
    color: var(--ink);
  }

  .c-value.big {
    font-size: 42px;
    color: var(--river-deep);
  }

  .cancel {
    position: absolute;
    bottom: 4.5vh;
    font-size: 13px;
  }

  /* ---------- snagged ---------- */

  .snagged {
    position: relative;
    height: 100%;
    display: flex;
    align-items: center;
    gap: 6vw;
    padding: 0 8vw;
  }

  .snag-art {
    height: min(58vh, 520px);
    aspect-ratio: 1;
    object-fit: cover;
    border-radius: 24px;
    -webkit-mask-image: radial-gradient(120% 120% at 50% 50%, #000 62%, transparent 98%);
    mask-image: radial-gradient(120% 120% at 50% 50%, #000 62%, transparent 98%);
    /* melt the asset's own paper into the page, like RiverArt */
    mix-blend-mode: multiply;
    flex: none;
  }

  .snag-copy {
    position: relative;
    /* local stacking context so the wash's -1 stays behind the copy
       but the whole group still blends above the river */
    isolation: isolate;
  }

  /* soft wash of paper behind the copy so it reads over the current */
  .snag-copy::before {
    content: "";
    position: absolute;
    inset: -48px -72px -56px -48px;
    background: radial-gradient(
      62% 62% at 42% 46%,
      rgba(243, 237, 222, 0.94) 0%,
      rgba(243, 237, 222, 0.55) 55%,
      rgba(243, 237, 222, 0) 100%
    );
    z-index: -1;
    pointer-events: none;
  }

  .snag-copy h1 {
    font-size: clamp(44px, 5vw, 68px);
    margin: 10px 0 14px;
  }

  .snag-copy .serif-lead {
    font-size: 16px;
    max-width: 440px;
  }

  .snag-copy .actions {
    display: flex;
    align-items: center;
    gap: 24px;
    margin-top: 34px;
  }
</style>
