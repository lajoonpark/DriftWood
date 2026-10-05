<script lang="ts">
  import RiverArt from "../components/RiverArt.svelte";
  import { reveal, stagger } from "../lib/motion";

  let { onNext }: { onNext: () => void } = $props();
</script>

<div class="welcome">
  <RiverArt variant="vertical" intensity={1} />

  <div class="content">
    <p class="kicker rv" use:reveal style="--rv-delay: 100ms">A quieter Mac</p>
    <h1 class="rv" use:reveal style="--rv-delay: 260ms">DriftWood</h1>
    <p class="lead serif-lead rv" use:reveal style="--rv-delay: 560ms">
      Finds what the current let go of —
    </p>
    <p class="lead serif-lead second rv" use:reveal style="--rv-delay: 760ms">
      and tells you what's safe to let drift.
    </p>

    <div class="actions rv" use:reveal style="--rv-delay: 1_050ms">
      <button class="btn btn-primary" onclick={onNext}>Begin</button>
      <button class="btn-quiet skip" onclick={onNext}>I've been here before</button>
    </div>

    <p class="promise rv" use:reveal style="--rv-delay: 1_250ms">
      Read-only, always. It never deletes a thing — it only shows you the map.
    </p>
  </div>
</div>

<style>
  .welcome {
    position: relative;
    height: 100%;
  }

  .content {
    position: relative;
    z-index: 2;
    height: 100%;
    display: flex;
    flex-direction: column;
    justify-content: center;
    padding-left: 9vw;
    max-width: 640px;
  }

  h1 {
    font-size: clamp(64px, 8.4vw, 118px);
    margin: 14px 0 34px;
  }

  .lead {
    font-size: clamp(19px, 2vw, 24px);
    line-height: 1.35;
  }

  .second {
    margin-left: 2.2em;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 26px;
    margin-top: 46px;
  }

  .skip {
    font-size: 14px;
  }

  .promise {
    margin-top: 64px;
    font-size: 13px;
    color: var(--ink-soft);
    max-width: 340px;
  }
</style>
