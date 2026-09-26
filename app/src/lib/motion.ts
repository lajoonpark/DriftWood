import type { Action } from "svelte/action";

export const reducedMotion =
  typeof window !== "undefined" &&
  window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/**
 * Reveal-on-enter. Add `class="rv"` and optionally `style="--rv-delay: 120ms"`.
 * The element fades/rises/declutters the first time it enters the viewport.
 */
export const reveal: Action<HTMLElement, number | undefined> = (el, delay) => {
  if (delay !== undefined) el.style.setProperty("--rv-delay", `${delay}ms`);
  // Elements already in view on first paint still animate, just sooner.
  const io = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (e.isIntersecting) {
          el.classList.add("is-in");
          io.disconnect();
        }
      }
    },
    { threshold: 0.05, rootMargin: "0px 0px -4% 0px" },
  );
  io.observe(el);
  return { destroy: () => io.disconnect() };
};

/** Delay helper for stagger chains (ms). */
export const stagger = (i: number, step = 70, base = 0) => base + i * step;

export function clamp(v: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, v));
}
