<script lang="ts">
  import { Eye, Plane, Ship, TrainFront } from "@lucide/svelte";
  import Segmented from "./Segmented.svelte";
  import type { Mode } from "./filters";

  interface Props {
    /** The focus: the sidebar's kind, always on the map. */
    mode: Mode;
    /** Which kinds are on the map: the focus, and any shown with it. */
    shown: Record<Mode, boolean>;
    /** How many of each are loaded; null when that kind isn't on the map (or hasn't loaded yet). */
    counts: Record<Mode, number | null>;
    /** Show or hide a kind other than the focus on the map. */
    ontoggle: (kind: Mode) => void;
  }

  let { mode = $bindable(), shown, counts, ontoggle }: Props = $props();

  const MODES = [
    ["aircraft", "Aircraft", "P"],
    ["trains", "Trains", "I"],
    ["ships", "Ships", "S"],
  ] as const;
  const ICONS = { aircraft: Plane, trains: TrainFront, ships: Ship };

  // group digits only from five up, as the most tracked list does: "1443", "12,345"
  const format = new Intl.NumberFormat("en-GB", { useGrouping: "min2" });
</script>

<!-- centred over the map, clear of the sidebar on the left and the detail panels on the right -->
<nav class="wrap" aria-label="Traffic">
  <div class="modebar surface">
    <Segmented legend="Show" hideLegend name="mode" options={MODES} bind:value={mode}>
      {#snippet content(option, label)}
        {@const Icon = ICONS[option]}
        <Icon size={15} strokeWidth={1.75} aria-hidden="true" />
        <span class="name">{label}</span>
        {#if counts[option] !== null}
          <span class="count data">{format.format(counts[option]!)}</span>
        {/if}
      {/snippet}
      {#snippet trailing(option, label)}
        <!-- the focus is always on the map: no toggle of its own -->
        {#if option !== mode}
          {@const key = MODES.find(([m]) => m === option)![2]}
          <button
            type="button"
            class="show"
            class:on={shown[option]}
            aria-pressed={shown[option]}
            aria-keyshortcuts="Shift+{key}"
            title="Also show {label.toLowerCase()} on the map (Shift+{key})"
            onclick={() => ontoggle(option)}
          >
            <Eye size={14} strokeWidth={1.75} aria-hidden="true" />
            <span class="visually-hidden">Also show {label.toLowerCase()} on the map</span>
          </button>
        {/if}
      {/snippet}
    </Segmented>
  </div>
</nav>

<style>
  .wrap {
    position: absolute;
    top: var(--space-4);
    left: calc(320px + 2 * var(--space-4));
    right: calc(320px + 2 * var(--space-4));
    display: flex;
    justify-content: center;
    pointer-events: none;
  }
  .modebar {
    padding: 3px;
    pointer-events: auto;
  }
  /* the surface is the frame: no second border round the options; a row, so each toggle sits by its option */
  .modebar :global(.options) {
    display: flex;
    padding: 0;
    border: 0;
  }
  .modebar :global(.options > label > span) {
    gap: 7px;
    padding: 6px var(--space-3);
    white-space: nowrap;
  }
  /* the keys are in each option's tooltip and the shortcuts list; here they'd crowd the counts */
  .modebar :global(kbd) {
    display: none;
  }
  .count {
    font-size: var(--text-sm);
    font-weight: 500;
    color: var(--text-3);
  }
  .modebar :global(input:checked + span > .count) {
    color: inherit;
    opacity: 0.75;
  }

  /* folded away until the bar is pointed at or tabbed into (not merely clicked: a click leaves focus behind); one that's on stays, in the accent */
  .show {
    display: grid;
    place-items: center;
    width: 0;
    margin-left: -2px;
    padding: 0;
    overflow: hidden;
    border: 0;
    border-radius: var(--radius-sm);
    background: none;
    color: var(--text-3);
    opacity: 0;
    cursor: pointer;
    transition:
      width 150ms var(--ease-out),
      margin 150ms var(--ease-out),
      opacity 150ms var(--ease-out),
      background-color 150ms var(--ease-out);
  }
  .modebar:hover .show,
  .modebar:has(:focus-visible) .show,
  .show.on {
    width: 24px;
    margin-left: -4px;
    margin-right: 2px;
    opacity: 1;
  }
  .show:hover {
    background: var(--surface-hover);
    color: var(--text-1);
  }
  .show.on {
    color: var(--amber);
  }
  .show:focus-visible {
    outline: 2px solid var(--amber);
    outline-offset: -2px;
  }
  /* an option on the map beside the focus: its label reads as present, not just available */
  .modebar :global(label:has(+ .show.on) > span) {
    color: var(--text-1);
  }
  /* Phones (the query in phone.svelte.ts): beside the search, in the flow,
     icons alone (the names stay for screen readers; the counts are in the
     sheet's head) */
  @media (max-width: 899px), (max-height: 500px) {
    .wrap {
      position: static;
    }
    .modebar {
      border: 0;
      background: none;
      box-shadow: none;
      padding: 0;
    }
    .modebar :global(.options > label > span) {
      justify-content: center;
      width: 44px;
      height: 36px;
      padding: 0;
    }
    .name,
    .count {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip-path: inset(50%);
      white-space: nowrap;
    }
  }
</style>
