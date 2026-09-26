<script lang="ts">
  import { Tween } from "svelte/motion";
  import { cubicOut } from "svelte/easing";

  let {
    value = 0,
    fmt = (v: number) => String(Math.round(v)),
    duration = 1_000,
  }: {
    value?: number;
    fmt?: (v: number) => string;
    duration?: number;
  } = $props();

  const tween = new Tween(0, {
    duration: () => duration,
    easing: cubicOut,
  });

  $effect(() => {
    tween.target = value;
  });

  const shown = $derived(fmt(tween.current));
</script>

<span class="num">{shown}</span>
