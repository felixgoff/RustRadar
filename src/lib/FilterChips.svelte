<script lang="ts" generics="T extends string | number">
  import { Check } from "@lucide/svelte";

  interface Props {
    label: string;
    /** Each chip: its value, name, colour on the map (a key) and how many are loaded. */
    options: readonly { value: T; label: string; color?: string; count?: number }[];
    /** The values left out: a chip is on unless its value is here. */
    hidden: T[];
    /** One quiet line under the chips. */
    hint?: string;
    disabled?: boolean;
  }

  let { label, options, hidden = $bindable(), hint, disabled = false }: Props = $props();

  function toggle(value: T) {
    hidden = hidden.includes(value) ? hidden.filter((v) => v !== value) : [...hidden, value];
  }
</script>

<fieldset {disabled}>
  <legend>
    <span class="label">{label}</span>
    <span class="links">
      <button class="text-button" disabled={!hidden.length} onclick={() => (hidden = [])}>All</button>
      <button class="text-button" disabled={hidden.length === options.length} onclick={() => (hidden = options.map((o) => o.value))}>
        None
      </button>
    </span>
  </legend>
  <div class="chips">
    {#each options as o (o.value)}
      {@const on = !hidden.includes(o.value)}
      <button class="chip" aria-pressed={on} onclick={() => toggle(o.value)}>
        {#if on}<Check size={12} strokeWidth={2.5} />{/if}
        {#if o.color}<span class="swatch" style:background={o.color} aria-hidden="true"></span>{/if}
        {o.label}
        {#if o.count}<span class="count data">{o.count.toLocaleString("en-GB")}</span>{/if}
      </button>
    {/each}
  </div>
  {#if hint}<p class="hint">{hint}</p>{/if}
</fieldset>

<style>
  /* as the aircraft filters' categories */
  fieldset {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin: 0;
    padding: 0;
    border: 0;
    min-width: 0;
  }
  legend {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    width: 100%;
    margin-bottom: var(--space-2);
    padding: 0;
  }
  .label {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-2);
  }
  .links {
    display: flex;
    gap: var(--space-1);
  }
  .text-button {
    padding: 2px 4px;
    border: 0;
    border-radius: 4px;
    background: none;
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--amber);
    cursor: pointer;
  }
  .text-button:hover {
    background: var(--amber-soft);
  }
  .text-button:disabled {
    color: var(--text-3);
    background: none;
    cursor: default;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 26px;
    padding: 0 var(--space-2);
    border: 1px solid var(--line-control);
    border-radius: 13px;
    background: none;
    font-size: var(--text-sm);
    color: var(--text-2);
    cursor: pointer;
    transition:
      background-color 150ms var(--ease-out),
      border-color 150ms var(--ease-out);
  }
  .chip:hover {
    background: var(--surface-hover);
  }
  .chip[aria-pressed="true"] {
    background: var(--surface-raised);
    color: var(--text-1);
  }
  .chip :global(svg) {
    margin-right: -2px;
    color: var(--amber);
  }
  .chip[aria-pressed="false"] {
    color: var(--text-3);
  }
  /* left out: its colour fades with it */
  .chip[aria-pressed="false"] .swatch {
    opacity: 0.35;
  }
  .swatch {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 2px;
  }
  .count {
    font-size: var(--text-xs);
    color: var(--text-3);
  }
  .hint {
    font-size: var(--text-sm);
    color: var(--text-3);
  }
  /* the hint says why, so it stays readable */
  fieldset:disabled legend,
  fieldset:disabled .chips {
    opacity: 0.45;
  }
  fieldset:disabled .chip,
  fieldset:disabled .text-button {
    cursor: default;
  }
  fieldset:disabled .chip:hover {
    background: none;
  }
</style>
