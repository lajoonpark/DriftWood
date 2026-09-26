<script lang="ts">
  import type { Tier } from "../lib/types";

  let {
    tier,
    size = 44,
    press = false,
    title,
  }: { tier: Tier; size?: number; press?: boolean; title?: string } = $props();

  const src: Record<Tier, string> = {
    1: "/assets/stamp-driftwood.png",
    2: "/assets/stamp-bottle.png",
    3: "/assets/stamp-current.png",
    4: "/assets/stamp-source.png",
  };
</script>

<button
  class="stamp"
  class:press
  style:width="{size}px"
  style:height="{size}px"
  {title}
  tabindex={-1}
  disabled
>
  <img src={src[tier]} alt="" draggable="false" />
</button>

<style>
  .stamp {
    border-radius: 50%;
    flex: none;
    -webkit-mask-image: radial-gradient(circle, #000 99%, transparent 100%);
    mask-image: radial-gradient(circle, #000 99%, transparent 100%);
  }

  .stamp img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
    filter: saturate(0.96);
  }

  .press {
    animation: press 0.55s var(--ease-out) backwards;
  }

  /* a stamp arriving on paper: drops in slightly over-inked, settles */
  @keyframes press {
    0% {
      opacity: 0;
      transform: scale(1.28) rotate(-7deg);
      filter: blur(3px);
    }
    55% {
      opacity: 1;
      transform: scale(0.94) rotate(1.5deg);
      filter: blur(0) saturate(1.25);
    }
    100% {
      transform: scale(1) rotate(0deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .press {
      animation: none;
    }
  }
</style>
