<script lang="ts">
  import { X } from "@lucide/svelte";
  import type { Ship } from "./api";
  import FilterChips from "./FilterChips.svelte";
  import Segmented from "./Segmented.svelte";
  import ToggleRow from "./ToggleRow.svelte";
  import { DEFAULT_SHIP_FILTERS, SHIP_SIZES, shipFilterCount, shipKindShort, type ShipFilters } from "./ship-list";
  import { SHIP_COLOR_KEYS, shipColor, shipKindKey } from "./ships";

  interface Props {
    filters: ShipFilters;
    /** Everything loaded, before filtering: for the counts. */
    ships: Ship[];
    shown: number;
    /** The feed tells sizes apart only close in; zoomed out every ship is "medium". */
    sizesKnown: boolean;
    onclose: () => void;
  }

  let { filters = $bindable(), ships, shown, sizesKnown, onclose }: Props = $props();

  const count = (n: number) => n.toLocaleString("en-GB");
  const active = $derived(shipFilterCount(filters));

  const kinds = $derived.by(() => {
    const n = new Map<number, number>();
    for (const s of ships) n.set(shipKindKey(s.kind), (n.get(shipKindKey(s.kind)) ?? 0) + 1);
    // the known types first, the unknown last
    return [...SHIP_COLOR_KEYS.filter((k) => k !== 0), 0].map((k) => ({
      value: k,
      label: shipKindShort(k),
      color: `rgb(${shipColor(k).join(", ")})`,
      count: n.get(k),
    }));
  });
  const sizes = $derived.by(() => {
    const n = new Map<number, number>();
    for (const s of ships) n.set(s.size, (n.get(s.size) ?? 0) + 1);
    return SHIP_SIZES.map((s) => ({ value: s.size, label: s.label, count: sizesKnown ? n.get(s.size) : undefined }));
  });

  const MOTIONS = [
    ["all", "All"],
    ["moving", "Under way"],
    ["moored", "Moored"],
  ] as const;

  function reset() {
    const { sort } = filters;
    Object.assign(filters, structuredClone(DEFAULT_SHIP_FILTERS), { sort });
  }
</script>

<section class="filters surface" aria-labelledby="ship-filters-heading">
  <header>
    <h2 id="ship-filters-heading">Filters</h2>
    <span class="shown">{count(shown)} of {count(ships.length)} shown</span>
    <button class="text-button" onclick={reset} disabled={!active}>Reset</button>
    <button class="icon-button" aria-label="Close filters" title="Close filters (F)" onclick={onclose}>
      <X size={16} strokeWidth={1.75} />
    </button>
  </header>

  <div class="body">
    <FilterChips label="Type" options={kinds} bind:hidden={filters.hiddenKinds} />
    <div class="group">
      <span class="label" aria-hidden="true">Movement</span>
      <Segmented legend="Movement" hideLegend name="ship-motion" options={MOTIONS} bind:value={filters.motion} />
      <p class="hint">Moored includes ships reporting no heading.</p>
    </div>
    <FilterChips
      label="Size"
      options={sizes}
      bind:hidden={filters.hiddenSizes}
      disabled={!sizesKnown}
      hint={sizesKnown ? undefined : "Zoom in to tell sizes apart: zoomed out, VesselFinder lists every ship as medium."}
    />
    <ToggleRow label="Hide old positions" hint="Ships that haven't reported lately" bind:checked={filters.hideOld} />
  </div>
</section>

<style>
  /* as the aircraft filters */
  .filters {
    display: flex;
    flex-direction: column;
    min-height: 0;
    flex: 0 1 auto;
  }
  header {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-2) var(--space-2) var(--space-4);
    border-bottom: 1px solid var(--line);
  }
  h2 {
    font-size: var(--text-base);
    font-weight: 650;
  }
  .shown {
    flex: 1;
    font-size: var(--text-sm);
    font-variant-numeric: tabular-nums;
    color: var(--text-3);
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
  .body {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding: var(--space-4);
    overflow-y: auto;
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .label {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-2);
  }
  .hint {
    font-size: var(--text-sm);
    color: var(--text-3);
  }
</style>
