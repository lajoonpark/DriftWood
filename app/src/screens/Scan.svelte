<script lang="ts">
  import { fade, fly } from "svelte/transition";
  import RiverArt from "../components/RiverArt.svelte";
  import StatusLine from "../components/StatusLine.svelte";
  import Counter from "../components/Counter.svelte";
  import Toggle from "../components/Toggle.svelte";
  import { bridge } from "../lib/bridge";
  import { formatBytes, formatCount } from "../lib/format";
  import { onboarding, scan, toast, SCOPE_FOLDERS, appSettings, saveSettings } from "../lib/stores";
  import { PRIVACY_LABELS, type Report, type ScanConfig } from "../lib/types";

  let { onDone }: { onDone: () => void } = $props();

  let error = $state<string | null>(null);
  let finishing = $state(false);
  /** Acknowledge "Pull ashore" immediately — the button must never look
   *  dead while a batch finishes; the report then arrives as partial. */
  let stoppingAck = $state(false);

  const s = $derived($onboarding);

  const cfg = $derived.by<ScanConfig>(() => {
    const scopes = (["low", "medium", "high"] as const).filter((cat) =>
      SCOPE_FOLDERS[cat].some((f) => s.folders[f.path]),
    );
    return {
      scopes,
      privacy_tier: s.privacy,
      // Express Scan: skip Stage 2 entirely — heuristic tiers only,
      // instant and free, honestly labeled as fallback in the report.
      stage2: !$appSettings.expressScan,
      model: $appSettings.model,
      api_key: $appSettings.apiKey.trim() || undefined,
      cost_cap_usd: $appSettings.costCap,
      allow_non_zdr: $appSettings.allowNonZdr,
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
  const rs = $derived(st.reasoning);

  /* Stage 2 is the only phase with a knowable denominator, so these are
   * real numbers — percentage, ETA, speed — and shown only here. */
  const stage2 = $derived.by(() => {
    if (!rs || rs.totalBatches === 0) return null;
    const pct = rs.total > 0 ? rs.judged / rs.total : 0;
    const remaining = Math.max(0, rs.totalBatches - rs.batchesFinished);
    const etaMs = rs.batchEmaMs > 0 ? remaining * rs.batchEmaMs : null;
    return { rs, pct, remaining, etaMs };
  });

  const cap = $derived($appSettings.costCap);
  const capHeadroom = $derived(Math.max(0, cap - (rs?.costUsd ?? 0)));

  function fmtEta(ms: number): string {
    const totalSec = Math.max(1, Math.round(ms / 1000));
    if (totalSec < 60) return `~${totalSec}s remaining`;
    const m = Math.floor(totalSec / 60);
    return `~${m}m ${String(totalSec % 60).padStart(2, "0")}s remaining`;
  }

  async function start() {
    error = null;
    stoppingAck = false;
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
        // Early-phase cancel only — nothing worth reporting existed yet.
        // A Stage 2 cancel arrives here as a partial report instead.
        toast("The scan drifted back ashore.");
      } else {
        error = msg;
      }
    }
  }

  function cancel() {
    if (stoppingAck || st.stopping) return;
    stoppingAck = true;
    scan.setStopping();
    bridge.cancelScan();
  }

  function setExpress(v: boolean) {
    saveSettings({ ...$appSettings, expressScan: v });
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
        {#if rs && rs.speed > 0}
          <div class="counter">
            <span class="c-label kicker">Speed</span>
            <span class="c-value">{Math.round(rs.speed)} items/sec</span>
          </div>
        {/if}
      </div>

      {#if stage2}
        <!-- Stage 2: the one honest progress bar — real percentage, real
             ETA, live cost against its visible ceiling. -->
        <div class="stage2" in:fade={{ duration: 400 }}>
          <div class="bar" role="progressbar" aria-valuenow={Math.round(stage2.pct * 100)}>
            <div class="fill" style={`width:${(stage2.pct * 100).toFixed(1)}%`}></div>
          </div>
          <div class="s2-meta">
            <span class="num">{formatCount(stage2.rs.judged)} of {formatCount(stage2.rs.total)} items judged</span>
            <span class="num" title="EMA of settled batch pace">
              {#if stage2.etaMs !== null}{fmtEta(stage2.etaMs)}{:else}settling…{/if}
            </span>
          </div>
          <div class="s2-cost">
            <span class="num">${stage2.rs.costUsd.toFixed(4)} spent</span>
            <span class="num cap-line">
              of ${cap.toFixed(2)} cap · ${capHeadroom.toFixed(2)} headroom
            </span>
          </div>
        </div>
      {/if}

      {#if st.running && !finishing}
        <button
          class="btn-quiet cancel"
          onclick={cancel}
          disabled={st.stopping}
          title={st.stopping ? "In-flight work is being stopped — the partial report is on its way." : "Stops the scan and keeps everything judged so far."}
        >
          {st.stopping ? "Pulling ashore…" : "Pull ashore"}
        </button>
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
        <label class="express" in:fade={{ duration: 600, delay: 660 }}>
          <Toggle
            checked={$appSettings.expressScan}
            onchange={setExpress}
            label="Skip AI reasoning"
          />
          <span class="express-copy">
            Skip AI reasoning
            <span class="express-hint">— fast, free, heuristic tiers only</span>
          </span>
        </label>
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

  .express {
    margin-top: 16px;
    display: flex;
    align-items: center;
    gap: 12px;
    cursor: pointer;
    font-size: 13.5px;
    color: var(--ink-soft);
  }

  .express-hint {
    color: var(--ink-faint);
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

  .cancel:disabled {
    opacity: 0.55;
    cursor: default;
  }

  /* ---------- Stage 2: honest progress ---------- */

  .stage2 {
    width: min(520px, 72vw);
    margin-top: 26px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .bar {
    height: 6px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.42);
    box-shadow: inset 0 0 0 1px var(--hairline);
    overflow: hidden;
  }

  .fill {
    height: 100%;
    border-radius: 999px;
    background: linear-gradient(90deg, var(--river) 0%, var(--river-deep) 100%);
    transition: width 0.6s var(--ease-out);
  }

  .s2-meta,
  .s2-cost {
    display: flex;
    justify-content: space-between;
    gap: 14px;
    font-size: 12.5px;
    color: var(--ink-soft);
  }

  .cap-line {
    color: var(--ink-faint);
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
