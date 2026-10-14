<script lang="ts">
  import type { Ship } from "./api";
  import Segmented from "./Segmented.svelte";
  import { SHIP_COVERAGE, SHIP_SIZES, shipKindShort, shipLength, shipName, type ShipSort } from "./ship-list";
  import { shipColor } from "./ships";
  import { distanceKm, distanceLabel } from "./train-list";

  interface Props {
    /** The ships that match the search and filters, in order. */
    ships: Ship[];
    /** How many are loaded in view, before the search and filters. */
    total: number;
    /** The first answer has come in. */
    loaded: boolean;
    /** The last request failed, or VesselFinder is resting. */
    failed: boolean;
    sort: ShipSort;
    /** The map's centre, for the distances. */
    centre: { lat: number; lon: number };
    /** The feed tells sizes apart only close in. */
    sizesKnown: boolean;
    selectedMmsi: number | null;
    onpick: (ship: Ship) => void;
    onhover?: (mmsi: number | null) => void;
    /** Clears the search and the filters. */
    onshowall: () => void;
  }

  let { ships, total, loaded, failed, sort = $bindable(), centre, sizesKnown, selectedMmsi, onpick, onhover, onshowall }: Props =
    $props();

  /** Rows drawn at most: a long list is narrowed with the search, not scrolled through. */
  const LIMIT = 150;
  const SORTS = [
    ["size", "Size"],
    ["name", "Name"],
    ["distance", "Distance"],
  ] as const;

  const rows = $derived(ships.slice(0, LIMIT));
  const swatch = (s: Ship) => `rgb(${shipColor(s.kind).join(", ")})`;
  /** Length when the feed gives dimensions, else the size class where it means something. */
  const size = (s: Ship) => {
    const length = shipLength(s);
    if (length) return `${length} m`;
    return sizesKnown ? (SHIP_SIZES[s.size]?.label ?? "") : "";
  };
</script>

<section aria-labelledby="ships-heading">
  <header>
    <h2 id="ships-heading">Ships in view</h2>
    {#if loaded && total}<span class="hint data">{ships.length.toLocaleString("en-GB")}</span>{/if}
  </header>
  <div class="sort" title="Distance is from the centre of the map">
    <Segmented legend="Sort ships by" hideLegend name="ship-sort" options={SORTS} bind:value={sort} />
  </div>

  {#if !loaded && !failed}
    <ol aria-busy="true" aria-label="Loading ships">
      {#each Array(6) as _, i (i)}
        <li class="row placeholder">
          <span class="skeleton" style="width: {80 + ((i * 23) % 60)}px"></span>
          <span class="skeleton" style="width: 40px"></span>
          <span class="skeleton wide" style="width: {100 + ((i * 31) % 70)}px"></span>
        </li>
      {/each}
    </ol>
  {:else if total === 0}
    <div class="message">
      {#if failed}
        <p>Couldn't reach VesselFinder. Trying again in a minute.</p>
      {:else}
        <p>No ships in view.</p>
      {/if}
      <p class="coverage">{SHIP_COVERAGE}</p>
    </div>
  {:else if ships.length === 0}
    <div class="message">
      <p>No ships in view match the search and filters.</p>
      <button class="button" onclick={onshowall}>Show all ships</button>
    </div>
  {:else}
    <ol>
      {#each rows as s (s.mmsi)}
        {@const name = shipName(s)}
        <li>
          <button
            class="row"
            class:active={s.mmsi === selectedMmsi}
            onclick={() => onpick(s)}
            onmouseenter={() => onhover?.(s.mmsi)}
            onmouseleave={() => onhover?.(null)}
            onfocus={() => onhover?.(s.mmsi)}
            onblur={() => onhover?.(null)}
          >
            <span class="name">
              <span class="swatch" style:background={swatch(s)} aria-hidden="true"></span>
              {#if name}
                <span class="text data" title={name}>{name}</span>
              {:else}
                <span class="text unnamed">MMSI <span class="data">{s.mmsi}</span></span>
              {/if}
            </span>
            <span class="measure data">
              {#if sort === "distance"}{distanceLabel(distanceKm(s.lat, s.lon, centre))}{:else}{size(s)}{/if}
            </span>
            <span class="sub">
              <span class="kind">{shipKindShort(s.kind)}</span>
              <span class="state">{s.heading !== null ? "Under way" : "Moored"}</span>
            </span>
            {#if s.old}
              <span class="badge warn" title="This ship hasn't reported its position lately">Old position</span>
            {:else}
              <span></span>
            {/if}
          </button>
        </li>
      {/each}
    </ol>
    {#if ships.length > LIMIT}
      <p class="more">{(ships.length - LIMIT).toLocaleString("en-GB")} more; search or filter to narrow the list.</p>
    {/if}
    <p class="coverage foot">{SHIP_COVERAGE}</p>
  {/if}
</section>

<style>
  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    padding: 0 var(--space-2) var(--space-2);
  }
  h2 {
    font-size: var(--text-base);
    font-weight: 650;
  }
  .hint {
    font-size: var(--text-sm);
    color: var(--text-3);
  }
  .sort {
    padding: 0 var(--space-2) var(--space-2);
  }
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: 2px var(--space-2);
    box-sizing: border-box;
    width: 100%;
    padding: var(--space-2);
    border: 0;
    border-radius: var(--radius-sm);
    background: none;
    text-align: left;
    cursor: pointer;
    transition: background-color 150ms var(--ease-out);
  }
  button.row:hover {
    background: var(--surface-hover);
  }
  button.row.active {
    background: var(--amber-soft);
  }
  button.row.active .text {
    color: var(--amber);
  }
  .name {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
    font-size: var(--text-md);
    font-weight: 650;
  }
  .text {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .unnamed {
    font-weight: 500;
    color: var(--text-2);
  }
  /* the ship's colour on the map, as its key */
  .swatch {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }
  .sub {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
    padding-left: 16px;
    font-size: var(--text-sm);
    color: var(--text-2);
  }
  .kind {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .state {
    flex: none;
    color: var(--text-3);
  }
  .measure {
    font-size: var(--text-sm);
    color: var(--text-2);
    text-align: right;
    white-space: nowrap;
  }
  .placeholder {
    cursor: default;
  }
  .placeholder .skeleton {
    height: 12px;
  }
  .placeholder .wide {
    grid-column: 1 / -1;
    height: 10px;
  }
  .message {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-3);
    padding: var(--space-2);
    font-size: var(--text-md);
    color: var(--text-2);
  }
  .coverage {
    font-size: var(--text-sm);
    color: var(--text-3);
  }
  .more,
  .foot {
    padding: var(--space-2) var(--space-2) var(--space-1);
    font-size: var(--text-xs);
    color: var(--text-3);
  }
  .more + .foot {
    padding-top: 0;
  }
</style>
