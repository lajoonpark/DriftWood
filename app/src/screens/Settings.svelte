<script lang="ts">
  import { reveal } from "../lib/motion";
  import { onboarding, resetEverything, saveOnboarding } from "../lib/stores";
  import { PRIVACY_LABELS, type PrivacyTier } from "../lib/types";

  let { onBack }: { onBack: () => void } = $props();

  let s = $state(structuredClone($onboarding));

  /* Settings beyond onboarding are UI-local until the shell persists them. */
  let model = $state("anthropic/claude-haiku-class");
  let costCap = $state(0.5);

  const MODELS = [
    { id: "anthropic/claude-haiku-class", label: "Claude Haiku-class — quick and cheap" },
    { id: "openai/gpt-4o-mini-class", label: "GPT-4o-mini-class — quick and cheap" },
    {
      id: "anthropic/claude-sonnet-class",
      label: "Claude Sonnet-class — more careful, more cost",
    },
  ];

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
            ? "Paths and names travel, contents never do. ZDR providers only."
            : "Adds a one-level folder listing for ambiguous items. Most expensive."}
      </p>
    </section>

    <section class="card block rv" use:reveal style="--rv-delay:420ms">
      <h2>The reasoning model</h2>
      <select bind:value={model}>
        {#each MODELS as m (m.id)}
          <option value={m.id}>{m.label}</option>
        {/each}
      </select>
      <p class="hint">Cheap-and-fast is the right default; the river is patient.</p>
    </section>

    <section class="card block rv" use:reveal style="--rv-delay:540ms">
      <h2>Cost cap</h2>
      <div class="cap-row">
        <input
          type="range"
          min="0.1"
          max="2"
          step="0.05"
          bind:value={costCap}
        />
        <span class="cap num">${costCap.toFixed(2)}</span>
      </div>
      <p class="hint">
        Hard ceiling per scan. Past it, heuristics finish the job — honestly labeled.
      </p>
    </section>

    <section class="card block rv" use:reveal style="--rv-delay:660ms">
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
        <button class="btn-quiet danger" onclick={resetEverything}>Forget everything</button>
      </div>
    </section>

    <footer class="rv" use:reveal style="--rv-delay:780ms">
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
    max-width: 640px;
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

  select {
    font: inherit;
    font-size: 14px;
    color: var(--ink);
    padding: 10px 14px;
    border-radius: 12px;
    border: 1px solid var(--hairline);
    background: rgba(255, 255, 255, 0.4);
    max-width: 380px;
    width: 100%;
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

  .danger:hover {
    color: var(--warn);
    border-color: var(--warn);
  }

  footer {
    margin-top: 26px;
  }
</style>
