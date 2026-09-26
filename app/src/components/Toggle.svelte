<script lang="ts">
  let {
    checked = $bindable(false),
    label,
    onchange,
  }: {
    checked?: boolean;
    label?: string;
    onchange?: (v: boolean) => void;
  } = $props();
</script>

<button
  class="toggle"
  class:on={checked}
  role="switch"
  aria-checked={checked}
  aria-label={label}
  onclick={() => {
    checked = !checked;
    onchange?.(checked);
  }}
>
  <span class="knob"></span>
</button>

<style>
  .toggle {
    position: relative;
    width: 40px;
    height: 24px;
    border-radius: 999px;
    background: var(--paper-deep);
    box-shadow:
      inset 0 0 0 1px var(--hairline),
      inset 0 1px 3px rgba(41, 50, 60, 0.12);
    transition: background-color 0.35s var(--ease-out), box-shadow 0.35s var(--ease-out);
    flex: none;
  }

  .knob {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--paper-raised);
    box-shadow:
      0 1px 2px rgba(41, 50, 60, 0.35),
      0 3px 8px -2px rgba(41, 50, 60, 0.3);
    transition: transform 0.35s var(--ease-out), background-color 0.35s;
  }

  .on {
    background: linear-gradient(160deg, var(--river) 0%, var(--river-deep) 100%);
    box-shadow:
      inset 0 0 0 1px rgba(44, 77, 151, 0.4),
      inset 0 1px 3px rgba(30, 45, 80, 0.3);
  }

  .on .knob {
    transform: translateX(16px);
    background: #f7f3e8;
  }

  .toggle:active .knob {
    transition-duration: 0.15s;
  }
</style>
