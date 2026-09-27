<script lang="ts">
  /* The "choose your waters" picker, shared between onboarding (Scopes)
     and Settings so folder access can be tuned at any time. */
  import Toggle from "./Toggle.svelte";
  import { reveal } from "../lib/motion";
  import { SCOPE_FOLDERS } from "../lib/stores";
  import { SCOPE_HINTS, SCOPE_LABELS, type ScopeCategory } from "../lib/types";

  let {
    folders,
    onchange,
    staggerBase = 0,
  }: {
    folders: Record<string, boolean>;
    onchange: () => void;
    staggerBase?: number;
  } = $props();

  const ORDER: ScopeCategory[] = ["low", "medium", "high"];

  function catOn(cat: ScopeCategory): boolean {
    return SCOPE_FOLDERS[cat].some((f) => folders[f.path]);
  }

  function setCat(cat: ScopeCategory, on: boolean) {
    for (const f of SCOPE_FOLDERS[cat]) folders[f.path] = on;
    onchange();
  }

  function setFolder(path: string, on: boolean) {
    folders[path] = on;
    onchange();
  }

  function preset(kind: "recommended" | "everything" | "no-caches" | "none") {
    for (const cat of ORDER)
      for (const f of SCOPE_FOLDERS[cat])
        folders[f.path] =
          kind === "everything" ||
          (kind === "recommended" && cat === "low") ||
          (kind === "no-caches" && cat !== "low");
    onchange();
  }
</script>

<div class="presets rv" use:reveal style="--rv-delay:{staggerBase}ms">
  <span class="kicker">Presets</span>
  <button class="btn-quiet" onclick={() => preset("recommended")}>Recommended</button>
  <button class="btn-quiet" onclick={() => preset("everything")}>Everything</button>
  <button class="btn-quiet" onclick={() => preset("no-caches")}>Skip all caches</button>
  <button class="btn-quiet" onclick={() => preset("none")}>Clear all</button>
</div>

<div class="cards">
  {#each ORDER as cat, i (cat)}
    {@const on = catOn(cat)}
    <section
      class="card cat rv {on ? 'on' : ''}"
      use:reveal
      style="--rv-delay:{staggerBase + 40 + i * 120}ms"
    >
      <div class="cat-head">
        <div>
          <p class="kicker">{SCOPE_LABELS[cat]}</p>
          <p class="hint">{SCOPE_HINTS[cat]}</p>
        </div>
        <Toggle checked={on} label={SCOPE_LABELS[cat]} onchange={(v) => setCat(cat, v)} />
      </div>
      <ul>
        {#each SCOPE_FOLDERS[cat] as f (f.path)}
          <li>
            <div class="folder">
              <span class="path mono">{f.path}</span>
              <span class="fhint">{f.hint}</span>
            </div>
            <Toggle
              checked={folders[f.path] ?? false}
              label={f.path}
              onchange={(v) => setFolder(f.path, v)}
            />
          </li>
        {/each}
      </ul>
    </section>
  {/each}
</div>

<style>
  .presets {
    display: flex;
    align-items: baseline;
    gap: 22px;
    max-width: 720px;
    width: 100%;
    margin: 0 0 18px;
  }

  .cards {
    display: flex;
    flex-direction: column;
    gap: 18px;
    max-width: 720px;
    width: 100%;
  }

  .cat {
    padding: 22px 26px 14px;
    transition:
      border-color 0.45s var(--ease-out),
      box-shadow 0.45s var(--ease-out),
      background-color 0.45s var(--ease-out);
  }

  .cat.on {
    border-color: rgba(63, 102, 168, 0.35);
    background:
      linear-gradient(165deg, rgba(127, 163, 205, 0.13), rgba(217, 229, 241, 0.06) 55%),
      var(--paper-raised);
    box-shadow:
      0 1px 0 rgba(255, 255, 255, 0.7) inset,
      0 18px 40px -24px rgba(44, 77, 151, 0.45);
  }

  .cat-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }

  .hint {
    font-size: 13.5px;
    color: var(--ink-soft);
    margin-top: 3px;
  }

  ul {
    list-style: none;
    padding: 0;
    margin: 16px 0 4px;
  }

  li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 9px 2px;
    border-top: 1px solid var(--hairline-soft);
  }

  .folder {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }

  .path {
    color: var(--ink);
    font-size: 13px;
  }

  .fhint {
    font-size: 12.5px;
    color: var(--ink-faint);
  }
</style>
