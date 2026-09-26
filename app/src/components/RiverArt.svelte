<script lang="ts">
  /** Watercolor river art with the three kept effects from the notes:
   *  turbulence wobble (feTurbulence + feDisplacementMap), pulsing shimmer,
   *  and the paper grain already baked into the asset/backdrop.
   *  Feathered CSS masks blend the asset's own paper edge into the page. */
  import { reducedMotion } from "../lib/motion";

  let {
    variant = "horizontal",
    intensity = 1,
  }: {
    variant?: "horizontal" | "vertical" | "diagonal";
    /** 0 = still water, 1 = idle drift, 2 = full current (during scans) */
    intensity?: number;
  } = $props();

  const uid = $props.id();
  const fid = `dw-ripple-${uid}`;

  const src = $derived(
    {
      horizontal: "/assets/river-horizontal.png",
      vertical: "/assets/river-vertical.png",
      diagonal: "/assets/river-diagonal.png",
    }[variant],
  );

  const scale = $derived(
    reducedMotion || intensity === 0 ? 0 : intensity === 2 ? 22 : 11,
  );
</script>

<div class="river {variant}">
  {#if scale > 0}
    <svg class="defs" aria-hidden="true">
      <filter id={fid} x="-6%" y="-6%" width="112%" height="112%">
        <feTurbulence
          type="fractalNoise"
          baseFrequency="0.008 0.013"
          numOctaves="2"
          seed="7"
          result="wobble"
        >
          <animate
            attributeName="baseFrequency"
            dur="26s"
            values="0.008 0.013;0.0115 0.0095;0.008 0.013"
            repeatCount="indefinite"
          />
        </feTurbulence>
        <feDisplacementMap
          in="SourceGraphic"
          in2="wobble"
          {scale}
          xChannelSelector="R"
          yChannelSelector="G"
        />
      </filter>
    </svg>
  {/if}
  <img class="layer base" {src} alt="" style:filter={scale > 0 ? `url(#${fid})` : "none"} />
  {#if !reducedMotion}
    <img class="layer shimmer" {src} alt="" />
  {/if}
</div>

<style>
  .river {
    position: absolute;
    inset: 0;
    pointer-events: none;
    overflow: hidden;
    /* The asset's own paper is cooler than the page cream — multiply melts
       it into the backdrop so only pigment remains. The isolated group keeps
       the shimmer's lighten blend working against the base layer only. */
    mix-blend-mode: multiply;
    isolation: isolate;
  }

  .defs {
    position: absolute;
    width: 0;
    height: 0;
  }

  .layer {
    position: absolute;
    object-fit: cover;
    will-change: transform;
  }

  /* --- geometry per variant ------------------------------------------ */

  .horizontal .layer {
    left: -1%;
    right: -1%;
    bottom: -2%;
    width: 102%;
    height: 64%;
    -webkit-mask-image: linear-gradient(to bottom, transparent 0%, #000 22%);
    mask-image: linear-gradient(to bottom, transparent 0%, #000 22%);
  }

  .vertical .layer {
    right: -8%;
    top: -4%;
    bottom: -4%;
    width: min(46vw, 720px);
    -webkit-mask-image: linear-gradient(to right, transparent 0%, #000 26%);
    mask-image: linear-gradient(to right, transparent 0%, #000 26%);
  }

  .diagonal .layer {
    right: -10%;
    top: -6%;
    height: 112%;
    width: min(42vw, 660px);
    -webkit-mask-image: linear-gradient(105deg, transparent 2%, #000 30%);
    mask-image: linear-gradient(105deg, transparent 2%, #000 30%);
  }

  /* --- the living-water effects -------------------------------------- */

  .base {
    animation: drift 36s var(--ease-soft) infinite alternate;
  }

  .shimmer {
    mix-blend-mode: lighten;
    filter: brightness(1.22) blur(0.6px);
    opacity: 0;
    animation:
      shimmer 6.5s ease-in-out infinite alternate,
      drift 36s var(--ease-soft) infinite alternate;
    animation-delay: 0s, -6s; /* de-sync from the base so they breathe */
  }

  @keyframes drift {
    from {
      transform: translate3d(-7px, -4px, 0) scale(1.008);
    }
    to {
      transform: translate3d(7px, 5px, 0) scale(1.014);
    }
  }

  @keyframes shimmer {
    from {
      opacity: 0.05;
    }
    to {
      opacity: 0.22;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .base,
    .shimmer {
      animation: none;
    }
  }
</style>
