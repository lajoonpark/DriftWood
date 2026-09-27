<script lang="ts">
  import { fade } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import type { TransitionConfig } from "svelte/transition";
  import Stamp from "./components/Stamp.svelte";
  import Welcome from "./screens/Welcome.svelte";
  import Scopes from "./screens/Scopes.svelte";
  import Trust from "./screens/Trust.svelte";
  import Privacy from "./screens/Privacy.svelte";
  import ScanScreen from "./screens/Scan.svelte";
  import ReportScreen from "./screens/Report.svelte";
  import SettingsScreen from "./screens/Settings.svelte";
  import { onboarding, saveOnboarding, scan, toasts } from "./lib/stores";
  import type { View } from "./lib/stores";

  /* ---------- which screen ---------- */

  const startView = (): View => {
    const q = new URLSearchParams(window.location.search).get("view") as View | null;
    if (q) return q;
    return $onboarding.done ? "scan" : "welcome";
  };

  let view = $state<View>(startView());

  const onboarded = $derived($onboarding.done);

  function go(v: View) {
    view = v;
  }

  function finishOnboarding() {
    saveOnboarding({ ...$onboarding, done: true });
    go("scan");
  }

  /* ---------- page transitions ---------- */

  /* Deliberately blur-free and out-transition-free. Two reasons:
     1. A full-viewport blur painted over the river's SVG-displaced layers
        stalls WKWebView's compositor for seconds (the "buttons feel dead
        after a snag" report) and the isolated blend group flashes the
        asset's raw paper white.
     2. Svelte drives `out:` removal from a requestAnimationFrame loop —
        when the webview suspends (occluded window, nap), the outgoing page
        never unmounts and the incoming one stays at opacity 0: every
        button looks dead. Entrance-only transitions self-heal: the swap is
        instant and the animation simply completes when rendering resumes.
        The per-element .rv reveals still carry the blur language. */

  const pageIn = (_node: Element, { delay = 0 } = {}): TransitionConfig => ({
    delay,
    duration: 640,
    easing: cubicOut,
    css: (t, u) => `opacity:${t}; transform: translateY(${28 * u}px)`,
  });

  const ONBOARDING_STEPS: View[] = ["welcome", "scopes", "trust", "privacy"];
  const stepIndex = $derived(ONBOARDING_STEPS.indexOf(view));

  const hasReport = $derived($scan.report !== null);
</script>

<div class="app">
  <header class="bar">
    <button class="brand" onclick={() => go(onboarded ? "scan" : "welcome")}>
      <Stamp tier={1} size={26} title="DriftWood" />
      <span class="wordmark">DriftWood</span>
    </button>

    <nav>
      {#if stepIndex >= 0}
        <div class="steps" aria-label="Onboarding progress">
          {#each ONBOARDING_STEPS as sv, i (sv)}
            <button
              class="step"
              class:cur={i === stepIndex}
              class:past={i < stepIndex}
              aria-label={`Step ${i + 1}`}
              onclick={() => go(sv)}
            ></button>
          {/each}
        </div>
      {:else}
        <button class="nav-link" class:cur={view === "scan"} onclick={() => go("scan")}>
          Scan
        </button>
        {#if hasReport}
          <button class="nav-link" class:cur={view === "report"} onclick={() => go("report")}>
            Report
          </button>
        {/if}
        <button class="nav-link" class:cur={view === "settings"} onclick={() => go("settings")}>
          Settings
        </button>
      {/if}
    </nav>
  </header>

  <main>
    {#key view}
      <div class="page" in:pageIn={{}}>
        {#if view === "welcome"}
          <Welcome onNext={() => go("scopes")} />
        {:else if view === "scopes"}
          <Scopes onNext={() => go("trust")} onBack={() => go("welcome")} />
        {:else if view === "trust"}
          <Trust onNext={() => go("privacy")} onBack={() => go("scopes")} />
        {:else if view === "privacy"}
          <Privacy onNext={finishOnboarding} onBack={() => go("trust")} />
        {:else if view === "scan"}
          <ScanScreen onDone={() => go("report")} />
        {:else if view === "report"}
          <ReportScreen onRescan={() => go("scan")} />
        {:else if view === "settings"}
          <SettingsScreen onBack={() => go(onboarded ? "scan" : "welcome")} />
        {/if}
      </div>
    {/key}
  </main>

  <div class="toasts">
    {#each $toasts as t (t.id)}
      <div class="toast" in:fade={{ duration: 300 }} out:fade={{ duration: 500 }}>
        {t.text}
      </div>
    {/each}
  </div>
</div>

<style>
  .app {
    height: 100vh;
    display: flex;
    flex-direction: column;
    position: relative;
  }

  .bar {
    position: relative;
    z-index: 10;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 18px 28px;
    flex: none;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .wordmark {
    font-family: var(--font-display);
    font-variation-settings: "opsz" 30, "SOFT" 60, "WONK" 1;
    font-size: 17px;
    font-weight: 460;
    letter-spacing: 0.01em;
  }

  nav {
    display: flex;
    align-items: center;
    gap: 22px;
  }

  .nav-link {
    font-size: 13.5px;
    font-weight: 600;
    color: var(--ink-faint);
    padding: 4px 2px;
    border-bottom: 1px solid transparent;
    transition: color 0.25s, border-color 0.25s;
  }

  .nav-link:hover {
    color: var(--ink);
  }

  .nav-link.cur {
    color: var(--ink);
    border-color: var(--river-mid);
  }

  .steps {
    display: flex;
    gap: 9px;
  }

  .step {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--paper-deep);
    box-shadow: inset 0 0 0 1px var(--hairline);
    transition:
      background-color 0.4s var(--ease-out),
      transform 0.4s var(--ease-out),
      box-shadow 0.4s;
  }

  .step:hover {
    transform: scale(1.25);
  }

  .step.past {
    background: var(--river-mid);
    box-shadow: none;
  }

  .step.cur {
    background: var(--river);
    transform: scale(1.3);
    box-shadow: 0 0 0 3px rgba(63, 102, 168, 0.18);
  }

  main {
    flex: 1;
    min-height: 0;
    position: relative;
  }

  .page {
    position: absolute;
    inset: 0;
    /* The river art melts into the page via mix-blend-mode: multiply, and any
       transform/filter (including page transitions) isolates that blend group.
       Carrying the paper — same color and grain as body, fixed so the grain
       lines up at the header seam — keeps the backdrop paper-colored while the
       river slides in, instead of flashing the asset's raw white paper. */
    background-color: var(--paper);
    background-image: url("/assets/paper-texture.png");
    background-size: 640px;
    background-position: center;
    background-attachment: fixed;
  }

  .toasts {
    position: fixed;
    bottom: 26px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 50;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    pointer-events: none;
  }

  .toast {
    background: var(--ink);
    color: var(--paper-raised);
    font-size: 13.5px;
    padding: 10px 20px;
    border-radius: 999px;
    box-shadow: 0 10px 30px -10px rgba(41, 50, 60, 0.5);
  }
</style>
