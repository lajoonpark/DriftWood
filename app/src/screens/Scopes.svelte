<script lang="ts">
  import Waters from "../components/Waters.svelte";
  import { reveal } from "../lib/motion";
  import { onboarding, saveOnboarding } from "../lib/stores";

  let { onNext, onBack }: { onNext: () => void; onBack: () => void } = $props();

  let s = $state(structuredClone($onboarding));

  function save() {
    saveOnboarding(s);
  }
</script>

<div class="scopes">
  <header>
    <p class="kicker rv" use:reveal>Where to look</p>
    <h1 class="rv" use:reveal style="--rv-delay:120ms">Choose your waters.</h1>
    <p class="lead serif-lead rv" use:reveal style="--rv-delay:280ms">
      DriftWood only walks the banks you point it at. Start narrow; widen when it
      earns your trust.
    </p>
  </header>

  <div class="picker rv" use:reveal style="--rv-delay:380ms">
    <Waters folders={s.folders} onchange={save} />
  </div>

  <footer class="rv" use:reveal style="--rv-delay:920ms">
    <button class="btn-quiet" onclick={onBack}>Back</button>
    <button class="btn btn-primary" onclick={onNext} disabled={!Object.values(s.folders).some(Boolean)}>
      Continue
    </button>
  </footer>
</div>

<style>
  .scopes {
    height: 100%;
    overflow-y: auto;
    padding: 9vh 0 8vh;
    display: flex;
    flex-direction: column;
    align-items: center;
  }

  header {
    max-width: 720px;
    width: 100%;
    padding: 0 32px;
  }

  h1 {
    font-size: clamp(38px, 4.6vw, 58px);
    margin: 12px 0 16px;
  }

  .lead {
    font-size: 17px;
    max-width: 540px;
  }

  .picker {
    display: flex;
    flex-direction: column;
    align-items: center;
    max-width: 720px;
    width: 100%;
    padding: 26px 32px 0;
  }

  footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    max-width: 720px;
    width: 100%;
    padding: 34px 32px 0;
  }
</style>
