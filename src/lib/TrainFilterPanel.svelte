<script lang="ts">
  import { X } from "@lucide/svelte";
  import type { Train } from "./api";
  import FilterChips from "./FilterChips.svelte";
  import ToggleRow from "./ToggleRow.svelte";
  import { DEFAULT_TRAIN_FILTERS, TRAIN_REGIONS, trainFilterCount, trainOperators, type TrainFilters } from "./train-list";
  import { TRAIN_KINDS, trainColor, trainColorOf, trainLabel } from "./trains";

  interface Props {
    filters: TrainFilters;
    /** Everything loaded, before filtering: for the counts. */
    trains: Train[];
    shown: number;
    onclose: () => void;
  }

  let { filters = $bindable(), trains, shown, onclose }: Props = $props();

  const count = (n: number) => n.toLocaleString("en-GB");
  const active = $derived(trainFilterCount(filters));

  const tally = <K,>(key: (t: Train) => K) => {
    const counts = new Map<K, number>();
    for (const t of trains) counts.set(key(t), (counts.get(key(t)) ?? 0) + 1);
    return counts;
  };
  const kinds = $derived.by(() => {
    const n = tally((t) => t.kind);
    return TRAIN_KINDS.map((k) => ({ value: k, label: trainLabel(k), color: `rgb(${trainColor(k).join(", ")})`, count: n.get(k) }));
  });
  const regions = $derived.by(() => {
    const n = tally((t) => t.source);
    return TRAIN_REGIONS.map((r) => ({ value: r.source, label: r.label, count: n.get(r.source) }));
  });

  // those of the trains loaded, and any left out that aren't running now (so they can be put back)
  const operators = $derived.by(() => {
    const running = trainOperators(trains);
    const gone = filters.hiddenOperators.filter((o) => !running.some((r) => r.name === o));
    return [...running, ...gone.map((name) => ({ name, count: 0 }))].map((o) => ({
      value: o.name,
      label: o.name || "Unknown",
      color: o.name ? `rgb(${trainColorOf({ kind: "other", operator: o.name }).join(", ")})` : undefined,
      count: o.count,
    }));
  });

  function reset() {
    filters.hiddenKinds = [];
    filters.hiddenSources = [];
    filters.hiddenOperators = [];
    filters.delayedOnly = DEFAULT_TRAIN_FILTERS.delayedOnly;
  }
</script>

<section class="filters surface" aria-labelledby="train-filters-heading">
  <header>
    <h2 id="train-filters-heading">Filters</h2>
    <span class="shown">{count(shown)} of {count(trains.length)} shown</span>
    <button class="text-button" onclick={reset} disabled={!active}>Reset</button>
    <button class="icon-button" aria-label="Close filters" title="Close filters (F)" onclick={onclose}>
      <X size={16} strokeWidth={1.75} />
    </button>
  </header>

  <div class="body">
    <FilterChips label="Kind" options={kinds} bind:hidden={filters.hiddenKinds} />
    <FilterChips label="Region" options={regions} bind:hidden={filters.hiddenSources} />
    <FilterChips
      label="Operator"
      options={operators}
      bind:hidden={filters.hiddenOperators}
      hint="In Sweden each operator's trains come live on their own stream; one left out isn't streamed."
    />
    <ToggleRow label="Delayed only" hint="5 min or more late, as the operator reports" bind:checked={filters.delayedOnly} />
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
</style>
