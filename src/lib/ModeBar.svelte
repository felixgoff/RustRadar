<script lang="ts">
  import { Plane, Ship, TrainFront } from "@lucide/svelte";
  import Segmented from "./Segmented.svelte";
  import type { Mode } from "./filters";

  interface Props {
    mode: Mode;
    /** How many of each are loaded; null when that kind isn't (only the mode's own is fetched). */
    counts: Record<Mode, number | null>;
  }

  let { mode = $bindable(), counts }: Props = $props();

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
        {label}
        {#if counts[option] !== null}
          <span class="count data">{format.format(counts[option]!)}</span>
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
  /* the surface is the frame: no second border round the options */
  .modebar :global(.options) {
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
</style>
