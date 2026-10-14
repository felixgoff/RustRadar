<script lang="ts" generics="T extends string | number">
  import type { Snippet } from "svelte";

  interface Props {
    /** Names the group for assistive tech; shown unless `hideLegend`. */
    legend: string;
    hideLegend?: boolean;
    /** Radio group name, unique on the page. */
    name: string;
    /** Value, label and an optional shortcut key for each option. */
    options: readonly (readonly [T, string, string?])[];
    value: T;
    disabled?: boolean;
    /** Two options a row; otherwise all on one row. */
    columns?: number;
    /** What an option shows in place of its label (which still names it): an icon, a count. */
    content?: Snippet<[T, string]>;
  }

  let { legend, hideLegend = false, name, options, value = $bindable(), disabled = false, columns, content }: Props = $props();
</script>

<fieldset class="segmented" {disabled}>
  <legend class:visually-hidden={hideLegend}>{legend}</legend>
  <!-- the border on a box of its own: a fieldset draws its legend into its border -->
  <div class="options" style:--columns={columns ?? options.length}>
    {#each options as [option, label, key] (option)}
      <label title={key ? `${label} (${key})` : undefined}>
        <input type="radio" {name} value={option} aria-keyshortcuts={key} bind:group={value} />
        <span>
          {#if content}{@render content(option, label)}{:else}{label}{/if}
          {#if key}<kbd aria-hidden="true">{key}</kbd>{/if}
        </span>
      </label>
    {/each}
  </div>
</fieldset>

<style>
  .segmented {
    min-width: 0;
    margin: 0;
    padding: 0;
    border: 0;
  }
  .options {
    display: grid;
    grid-template-columns: repeat(var(--columns), 1fr);
    gap: 2px;
    padding: 2px;
    border: 1px solid var(--line-control);
    border-radius: 0;
  }
  legend {
    margin-bottom: var(--space-2);
    padding: 0;
    font-size: var(--text-md);
    color: var(--text-1);
  }
  label {
    position: relative;
  }
  input {
    position: absolute;
    opacity: 0;
  }
  span {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 5px 0;
    border-radius: 0;
    font-size: var(--text-md);
    color: var(--text-2);
    cursor: pointer;
    transition: background-color 150ms var(--ease-out);
  }
  kbd {
    font: 600 var(--text-xs) var(--font-data);
    opacity: 0.7;
  }
  label:hover span {
    background: var(--surface-hover);
  }
  /* e.g. the place-name size while place names are off */
  .segmented:disabled {
    opacity: 0.4;
  }
  .segmented:disabled span {
    cursor: default;
  }
  .segmented:disabled label:hover span {
    background: none;
  }
  input:checked + span {
    background: var(--amber);
    color: var(--amber-ink);
    font-weight: 650;
  }
  input:focus-visible + span {
    outline: 2px solid var(--amber);
    outline-offset: 1px;
  }
</style>
