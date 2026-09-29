<script lang="ts">
  import Waters from "../components/Waters.svelte";
  import Toggle from "../components/Toggle.svelte";
  import { reveal } from "../lib/motion";
  import {
    onboarding,
    saveOnboarding,
    appSettings,
    saveSettings,
    resetEverything,
  } from "../lib/stores";
  import { PRIVACY_LABELS, type PrivacyTier } from "../lib/types";

  let { onBack }: { onBack: () => void } = $props();

  let s = $state(structuredClone($onboarding));
  let tune = $state(structuredClone($appSettings));

  function saveTune() {
    saveSettings(tune);
  }

  /* ---------- OpenRouter catalog ---------- */

  interface ORModel {
    id: string;
    name: string;
    context_length?: number;
    pricing?: { prompt?: string; completion?: string };
    architecture?: { output_modalities?: string[] };
  }

  const CATALOG_CACHE_KEY = "driftwood.catalog.v1";
  const CATALOG_TTL = 24 * 60 * 60 * 1000;

  /* Offline fallback so the picker is never empty. */
  const FALLBACK_MODELS: ORModel[] = [
    { id: "anthropic/claude-haiku-4.5", name: "Anthropic: Claude Haiku 4.5", context_length: 200000, pricing: { prompt: "0.000001", completion: "0.000005" } },
    { id: "openai/gpt-4o-mini", name: "OpenAI: GPT-4o-mini", context_length: 128000, pricing: { prompt: "0.00000015", completion: "0.0000006" } },
    { id: "openai/gpt-5-mini", name: "OpenAI: GPT-5 Mini", context_length: 400000, pricing: { prompt: "0.00000025", completion: "0.000002" } },
    { id: "meta-llama/llama-3.3-70b-instruct", name: "Meta: Llama 3.3 70B Instruct", context_length: 131072, pricing: { prompt: "0.0000001", completion: "0.00000032" } },
  ];

  let catalogList = $state<ORModel[]>([]);
  let catalogState = $state<"loading" | "ok" | "failed">("loading");

  function isTextModel(m: ORModel): boolean {
    const outs = m.architecture?.output_modalities;
    return !outs || outs.includes("text");
  }

  function readCache(): { list: ORModel[] } | null {
    try {
      const raw = localStorage.getItem(CATALOG_CACHE_KEY);
      if (!raw) return null;
      const cached = JSON.parse(raw) as { at: number; list: ORModel[] };
      if (Date.now() - cached.at > CATALOG_TTL || !Array.isArray(cached.list)) return null;
      return cached;
    } catch {
      return null;
    }
  }

  async function loadCatalog(force = false) {
    if (!force) {
      const cached = readCache();
      if (cached) {
        catalogList = cached.list.filter(isTextModel);
        catalogState = "ok";
        return;
      }
    }
    catalogState = "loading";
    try {
      const res = await fetch("https://openrouter.ai/api/v1/models");
      if (!res.ok) throw new Error(String(res.status));
      const j = (await res.json()) as { data: ORModel[] };
      catalogList = j.data.filter(isTextModel);
      catalogState = "ok";
      try {
        localStorage.setItem(CATALOG_CACHE_KEY, JSON.stringify({ at: Date.now(), list: j.data }));
      } catch {
        /* storage full — cache is optional */
      }
    } catch {
      catalogList = FALLBACK_MODELS;
      catalogState = "failed";
    }
  }

  loadCatalog();

  let query = $state("");

  const results = $derived.by(() => {
    if (catalogState === "loading") return [];
    const q = query.trim().toLowerCase();
    const pool = catalogList;
    if (!q) return [];
    const terms = q.split(/\s+/);
    const scored = pool
      .map((m) => {
        const hay = `${m.id} ${m.name}`.toLowerCase();
        const idx = terms.reduce((acc, t) => Math.max(acc, hay.indexOf(t)), 0);
        return { m, hit: terms.every((t) => hay.includes(t)), idx };
      })
      .filter((r) => r.hit);
    scored.sort((a, b) => a.idx - b.idx || a.m.id.localeCompare(b.m.id));
    return scored.slice(0, 60).map((r) => r.m);
  });

  const selected = $derived(
    catalogList.find((m) => m.id === tune.model) ??
      FALLBACK_MODELS.find((m) => m.id === tune.model) ??
      null,
  );

  /* ZDR posture: OpenRouter's catalog doesn't expose zero-data-retention
     flags per model, and most models are NOT ZDR (fast/free ones almost
     never are). So when the user hasn't opted into non-ZDR routing, the
     selected model may simply fail to route — and when they have, the
     consequence must be visible right here, not buried in the danger
     zone's fine print. Never claim a model *is* ZDR. */
  const zdrNote = $derived.by(() => {
    if (tune.allowNonZdr) {
      return "This model may be served by providers that keep your requests — that's what \"allow non-ZDR\" means. Only what your privacy tier lets travel can reach them (paths and names at Standard).";
    }
    return "Requests for this model are routed only to providers that never keep your prompts (ZDR-only). If OpenRouter can't serve it under that policy, the river crossing fails — most free and fast models are not ZDR, and using them requires the danger-zone opt-in below.";
  });

  function fmtPrice(perToken?: string): string | null {
    const n = parseFloat(perToken ?? "");
    if (!isFinite(n) || n < 0) return null; // "-1" = dynamic pricing
    const perM = n * 1_000_000;
    return perM === 0 ? "free" : `$${perM < 1 ? perM.toFixed(2) : perM.toFixed(perM < 10 ? 2 : 0)}`;
  }

  function ctxLabel(n?: number): string {
    if (!n) return "";
    return n >= 1_000_000 ? `${(n / 1_000_000).toFixed(n % 1_000_000 ? 1 : 0)}M` : `${Math.round(n / 1000)}k`;
  }

  function pick(m: ORModel) {
    tune.model = m.id;
    saveTune();
    query = "";
  }

  /* ---------- OpenRouter key ---------- */

  let keyStatus = $state<"idle" | "checking" | "ok" | "bad" | "offline">("idle");
  let keyCredits = $state<string | null>(null);

  async function verifyKey() {
    if (!tune.apiKey.trim()) {
      keyStatus = "idle";
      return;
    }
    keyStatus = "checking";
    try {
      const res = await fetch("https://openrouter.ai/api/v1/key", {
        headers: { Authorization: `Bearer ${tune.apiKey.trim()}` },
      });
      if (res.ok) {
        keyStatus = "ok";
        try {
          const j = (await res.json()) as { data?: { limit?: number | null; usage?: number } };
          if (j.data && typeof j.data.limit === "number") {
            keyCredits = `$${Math.max(0, j.data.limit - (j.data.usage ?? 0)).toFixed(2)} of credit left`;
          } else {
            keyCredits = "unlimited credit";
          }
        } catch {
          keyCredits = null;
        }
      } else {
        keyStatus = "bad";
        keyCredits = null;
      }
    } catch {
      keyStatus = "offline";
      keyCredits = null;
    }
  }

  /* Shown as read-only context until the memory screens land (Phase 7). */
  const RULES = [
    { id: "dmg-in-downloads", detail: ".dmg in ~/Downloads → Message in a Bottle", uses: 1 },
    { id: "epic-orphan", detail: "orphaned com.epicgames.* → Driftwood", uses: 3 },
    { id: "keep-mobilesync", detail: "MobileSync backups → always Source", uses: 2 },
  ];

  function setPrivacy(p: PrivacyTier) {
    s.privacy = p;
    saveOnboarding(s);
  }

  /* ---------- Danger zone: non-ZDR routing ---------- */

  let zdrConfirm = $state(false);

  function toggleZdr(v: boolean) {
    if (v) {
      // Enabling is a two-step: the toggle only arms the inline
      // confirmation — the user must accept the consequences explicitly.
      zdrConfirm = true;
    } else {
      tune.allowNonZdr = false;
      zdrConfirm = false;
      saveTune();
    }
  }

  function confirmZdr() {
    tune.allowNonZdr = true;
    zdrConfirm = false;
    saveTune();
  }

  function cancelZdr() {
    zdrConfirm = false;
  }

  const ZDR_OFF_HINT =
    "On by default — recommended. Every river crossing is routed only to providers that never keep your prompts (OpenRouter's zero-data-retention policy). Most free models don't qualify, so they can't be used while this is off.";

  const ZDR_ON_HINT =
    "Not recommended. Requests may now be routed to any provider, including free ones — which usually pay for themselves by logging or training on what you send. Only what your privacy tier allows still travels (paths and names at Standard, plus a one-level folder listing at Deep; never file contents). Nothing on your Mac changes — this only loosens where the thinking happens, and it does nothing at Minimal, where nothing leaves your machine anyway.";

  function saveWaters() {
    saveOnboarding(s);
  }
</script>

<div class="settings">
  <div class="inner">
    <p class="kicker rv" use:reveal>Settings</p>
    <h1 class="rv" use:reveal style="--rv-delay:120ms">Tune the river.</h1>

    <section class="card block rv" use:reveal style="--rv-delay:300ms">
      <h2>Privacy tier</h2>
      <div class="seg">
        {#each ["minimal", "standard", "deep"] as p (p)}
          <button
            class="seg-btn"
            class:sel={s.privacy === p}
            onclick={() => setPrivacy(p as PrivacyTier)}
          >
            {PRIVACY_LABELS[p as PrivacyTier]}
          </button>
        {/each}
      </div>
      <p class="hint">
        {s.privacy === "minimal"
          ? "Nothing leaves your machine. Judged by shape alone."
          : s.privacy === "standard"
            ? tune.allowNonZdr
              ? "Paths and names travel, contents never do. ZDR-only routing is off (danger zone below)."
              : "Paths and names travel, contents never do. ZDR providers only."
            : "Adds a one-level folder listing for ambiguous items. Most expensive."}
      </p>
    </section>

    <section class="waters-block rv" use:reveal style="--rv-delay:400ms">
      <div class="waters-head">
        <h2>Where the river may look</h2>
        <p class="hint">The banks DriftWood may walk — same choice you made at first setup.</p>
      </div>
      <Waters folders={s.folders} onchange={saveWaters} staggerBase={480} />
    </section>

    <section class="card block rv" use:reveal style="--rv-delay:520ms">
      <h2>The reasoning model</h2>

      <label class="field-label kicker" for="dw-apikey">OpenRouter API key</label>
      <div class="key-line">
        <input
          id="dw-apikey"
          type="password"
          placeholder="sk-or-…"
          autocomplete="off"
          spellcheck="false"
          bind:value={tune.apiKey}
          onblur={saveTune}
          oninput={() => (keyStatus = "idle")}
        />
        <button class="btn-ghost btn key-btn" onclick={verifyKey} disabled={keyStatus === "checking" || !tune.apiKey.trim()}>
          {keyStatus === "checking" ? "Asking…" : "Test key"}
        </button>
      </div>
      <p class="hint">
        {#if keyStatus === "ok"}
          <span class="ok-dot"></span>Key works{keyCredits ? ` — ${keyCredits}` : ""}.
        {:else if keyStatus === "bad"}
          OpenRouter rejected that key. Check it at openrouter.ai/keys.
        {:else if keyStatus === "offline"}
          Couldn't reach openrouter.ai — check your connection.
        {:else}
          Your key stays on this Mac.
          <a class="quiet-link" href="https://openrouter.ai/keys" target="_blank" rel="noreferrer">Get one here</a>.
        {/if}
      </p>

      <label class="field-label kicker" for="dw-modelq">Model</label>
      <div class="current-model">
        <span class="m-name">{selected?.name ?? tune.model}</span>
        {#if selected}
          <span class="m-meta num">
            {ctxLabel(selected.context_length)} ctx
            {#if fmtPrice(selected.pricing?.prompt)}
              · {fmtPrice(selected.pricing?.prompt)} in / {fmtPrice(selected.pricing?.completion)} out per 1M
            {:else}
              · dynamic pricing
            {/if}
          </span>
        {/if}
      </div>
      <p class="hint zdr-note" class:zdr-warn={tune.allowNonZdr}>{zdrNote}</p>

      <input
        id="dw-modelq"
        type="search"
        class="model-search"
        placeholder="Search all OpenRouter text models…"
        bind:value={query}
      />

      {#if catalogState === "loading"}
        <p class="hint">Fetching the catalog…</p>
      {:else if catalogState === "failed"}
        <p class="hint">
          Couldn't reach openrouter.ai — showing known defaults.
          <button class="btn-quiet" onclick={() => loadCatalog(true)}>Try again</button>
        </p>
      {/if}

      {#if results.length}
        <ul class="models">
          {#each results as m (m.id)}
            <li>
              <button class="m-row" class:sel={m.id === tune.model} onclick={() => pick(m)}>
                <span class="m-name">{m.name}</span>
                <span class="m-meta num">
                  {ctxLabel(m.context_length)} ctx
                  {#if fmtPrice(m.pricing?.prompt)}
                    · {fmtPrice(m.pricing?.prompt)} / {fmtPrice(m.pricing?.completion)} per 1M
                  {:else}
                    · dynamic
                  {/if}
                </span>
                <span class="m-id mono">{m.id}</span>
              </button>
            </li>
          {/each}
        </ul>
      {:else if query.trim() && catalogState === "ok"}
        <p class="hint">Nothing in the catalog matches “{query}”.</p>
      {/if}
    </section>

    <section class="card block rv" use:reveal style="--rv-delay:640ms">
      <h2>Cost cap</h2>
      <div class="cap-row">
        <input
          type="range"
          min="0.1"
          max="2"
          step="0.05"
          bind:value={tune.costCap}
          oninput={saveTune}
        />
        <span class="cap num">${tune.costCap.toFixed(2)}</span>
      </div>
      <p class="hint">
        Hard ceiling per scan. Past it, heuristics finish the job — honestly labeled.
      </p>
    </section>

    <section class="card block rv" use:reveal style="--rv-delay:760ms">
      <h2>What the river remembers</h2>
      <p class="hint">
        Your corrections distill into local rules — plain JSON, editable, portable.
      </p>
      <ul class="rules">
        {#each RULES as r (r.id)}
          <li>
            <span class="mono">{r.id}</span>
            <span class="r-detail">{r.detail}</span>
            <span class="uses num">×{r.uses}</span>
          </li>
        {/each}
      </ul>
      <div class="block-actions">
        <button class="btn-quiet">Open memory folder</button>
      </div>
    </section>

    <section class="card block danger-zone rv" use:reveal style="--rv-delay:820ms">
      <h2>Danger zone</h2>

      <div class="dz-row">
        <div class="dz-text">
          <p class="dz-title">Allow providers that keep your data</p>
          <p class="hint">{tune.allowNonZdr ? ZDR_ON_HINT : ZDR_OFF_HINT}</p>
        </div>
        <Toggle checked={tune.allowNonZdr} label="Allow non-ZDR providers" onchange={toggleZdr} />
      </div>

      {#if zdrConfirm}
        <div class="dz-confirm">
          <p>
            With ZDR-only routing off, whatever your privacy tier lets travel can reach
            providers that may keep it or train on it. Free models usually pay for
            themselves that way. DriftWood won't stop you — but this is not recommended.
          </p>
          <div class="dz-actions">
            <button class="btn-quiet" onclick={cancelZdr}>Keep my data protected</button>
            <button class="btn-quiet danger" onclick={confirmZdr}>Allow anyway</button>
          </div>
        </div>
      {/if}

      <div class="block-actions">
        <button class="btn-quiet danger" onclick={resetEverything}>Forget everything</button>
      </div>
    </section>

    <footer class="rv" use:reveal style="--rv-delay:940ms">
      <button class="btn btn-ghost" onclick={onBack}>Back</button>
    </footer>
  </div>
</div>

<style>
  .settings {
    height: 100%;
    overflow-y: auto;
  }

  .inner {
    max-width: 720px;
    margin: 0 auto;
    padding: 9vh 32px 10vh;
  }

  h1 {
    font-size: clamp(34px, 4vw, 50px);
    margin: 12px 0 34px;
  }

  .block {
    padding: 24px 28px;
    margin-bottom: 18px;
  }

  h2 {
    font-size: 19px;
    font-weight: 450;
    margin-bottom: 14px;
  }

  .hint {
    font-size: 13px;
    color: var(--ink-faint);
    margin-top: 10px;
  }

  .waters-block {
    margin-bottom: 18px;
  }

  .waters-head {
    padding: 0 4px;
    margin-bottom: 16px;
  }

  .waters-head h2 {
    margin-bottom: 4px;
  }

  .seg {
    display: inline-flex;
    border: 1px solid var(--hairline);
    border-radius: 999px;
    padding: 3px;
    gap: 2px;
    background: rgba(255, 255, 255, 0.3);
  }

  .seg-btn {
    padding: 8px 22px;
    border-radius: 999px;
    font-size: 13.5px;
    font-weight: 600;
    color: var(--ink-soft);
    transition:
      background-color 0.3s var(--ease-out),
      color 0.3s,
      box-shadow 0.3s;
  }

  .seg-btn:hover {
    color: var(--ink);
  }

  .seg-btn.sel {
    color: #f5f1e6;
    background: linear-gradient(160deg, #4873b4 0%, var(--river-deep) 100%);
    box-shadow: 0 4px 12px -4px rgba(44, 77, 151, 0.55);
  }

  input[type="password"],
  input[type="search"],
  .model-search {
    font: inherit;
    font-size: 14px;
    color: var(--ink);
    padding: 10px 14px;
    border-radius: 12px;
    border: 1px solid var(--hairline);
    background: rgba(255, 255, 255, 0.4);
    width: 100%;
  }

  input:focus-visible {
    outline: 2px solid var(--river);
    outline-offset: 1px;
  }

  .field-label {
    display: block;
    margin: 18px 0 8px;
  }

  .block h2 + .field-label {
    margin-top: 0;
  }

  .key-line {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .key-line input {
    flex: 1;
  }

  .key-btn {
    flex: none;
    padding: 10px 18px;
    font-size: 13.5px;
  }

  .key-btn:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .ok-dot {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--good);
    margin-right: 4px;
    vertical-align: baseline;
  }

  .quiet-link {
    color: var(--river-deep);
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  .current-model {
    display: flex;
    align-items: baseline;
    gap: 14px;
    flex-wrap: wrap;
    padding: 12px 16px;
    border: 1px solid var(--hairline-soft);
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.3);
  }

  .m-name {
    font-size: 14px;
    font-weight: 600;
    color: var(--ink);
  }

  .m-meta {
    font-size: 12.5px;
    color: var(--ink-faint);
  }

  .zdr-note {
    margin-top: 8px;
    padding: 10px 14px;
    border: 1px solid var(--hairline-soft);
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.3);
  }

  .zdr-note.zdr-warn {
    border-color: rgba(168, 85, 47, 0.4);
    color: var(--warn);
  }

  .models {
    list-style: none;
    margin: 10px 0 0;
    padding: 0;
    max-height: 300px;
    overflow-y: auto;
    border: 1px solid var(--hairline-soft);
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.3);
  }

  .models li + li {
    border-top: 1px solid var(--hairline-soft);
  }

  .m-row {
    display: grid;
    grid-template-columns: 1fr auto;
    grid-template-areas:
      "name meta"
      "id meta";
    align-items: baseline;
    column-gap: 14px;
    width: 100%;
    text-align: left;
    padding: 9px 14px;
    transition: background-color 0.2s;
  }

  .m-row:hover {
    background: rgba(127, 163, 205, 0.12);
  }

  .m-row.sel {
    background: rgba(63, 102, 168, 0.14);
    box-shadow: inset 2px 0 0 var(--river);
  }

  .m-row .m-id {
    grid-area: id;
    font-size: 11.5px;
    color: var(--ink-faint);
  }

  .m-row .m-meta {
    grid-area: meta;
    white-space: nowrap;
  }

  .cap-row {
    display: flex;
    align-items: center;
    gap: 18px;
  }

  input[type="range"] {
    flex: 1;
    max-width: 320px;
    accent-color: var(--river);
  }

  .cap {
    font-family: var(--font-display);
    font-size: 20px;
    color: var(--river-deep);
  }

  .rules {
    list-style: none;
    padding: 0;
    margin: 4px 0 0;
  }

  .rules li {
    display: flex;
    align-items: baseline;
    gap: 14px;
    padding: 9px 2px;
    border-top: 1px solid var(--hairline-soft);
    font-size: 13px;
  }

  .r-detail {
    color: var(--ink-soft);
    flex: 1;
  }

  .uses {
    color: var(--ink-faint);
  }

  .block-actions {
    display: flex;
    gap: 24px;
    margin-top: 16px;
  }

  /* ---------- Danger zone ---------- */

  .danger-zone {
    border-color: rgba(168, 85, 47, 0.35);
  }

  .danger-zone h2 {
    color: var(--warn);
    font-weight: 500;
  }

  .dz-row {
    display: flex;
    align-items: flex-start;
    gap: 20px;
  }

  .dz-text {
    flex: 1;
  }

  .dz-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--ink);
  }

  .dz-text .hint {
    margin-top: 4px;
  }

  .dz-confirm {
    margin-top: 14px;
    padding: 14px 16px;
    border: 1px solid rgba(168, 85, 47, 0.4);
    border-radius: 12px;
    background: rgba(168, 85, 47, 0.07);
  }

  .dz-confirm p {
    font-size: 13px;
    color: var(--ink-soft);
    margin: 0;
  }

  .dz-actions {
    display: flex;
    gap: 20px;
    margin-top: 12px;
  }

  .dz-confirm .danger {
    color: var(--warn);
    border-color: rgba(168, 85, 47, 0.45);
  }

  .dz-confirm .danger:hover {
    background: rgba(168, 85, 47, 0.1);
  }

  .danger:hover {
    color: var(--warn);
    border-color: var(--warn);
  }

  footer {
    margin-top: 26px;
  }
</style>
