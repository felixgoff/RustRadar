<script lang="ts">
  import { ArrowRight } from "@lucide/svelte";
  import type { Train } from "./api";
  import Segmented from "./Segmented.svelte";
  import { distanceKm, distanceLabel, TRAIN_COVERAGE, type TrainSort } from "./train-list";
  import { delayText, delayTone, trainColor, trainLabel } from "./trains";

  interface Props {
    /** The trains that match the search and filters, in order. */
    trains: Train[];
    /** How many are loaded in view, before the search and filters. */
    total: number;
    /** The first answer has come in. */
    loaded: boolean;
    /** The last request failed (every source in view). */
    failed: boolean;
    sort: TrainSort;
    /** The map's centre, for the distances. */
    centre: { lat: number; lon: number };
    selectedId: string | null;
    onpick: (train: Train) => void;
    onhover?: (id: string | null) => void;
    /** Clears the search and the filters. */
    onshowall: () => void;
  }

  let { trains, total, loaded, failed, sort = $bindable(), centre, selectedId, onpick, onhover, onshowall }: Props = $props();

  /** Rows drawn at most: a long list is narrowed with the search, not scrolled through. */
  const LIMIT = 150;
  const SORTS = [
    ["delay", "Delay"],
    ["speed", "Speed"],
    ["name", "Name"],
    ["distance", "Distance"],
  ] as const;

  const rows = $derived(trains.slice(0, LIMIT));
  const swatch = (t: Train) => `rgb(${trainColor(t.kind).join(", ")})`;
  const capital = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
</script>

<section aria-labelledby="trains-heading">
  <header>
    <h2 id="trains-heading">Trains in view</h2>
    {#if loaded && total}<span class="hint data">{trains.length.toLocaleString("en-GB")}</span>{/if}
  </header>
  <div class="sort" title="Distance is from the centre of the map">
    <Segmented legend="Sort trains by" hideLegend name="train-sort" options={SORTS} bind:value={sort} />
  </div>

  {#if !loaded && !failed}
    <ol aria-busy="true" aria-label="Loading trains">
      {#each Array(6) as _, i (i)}
        <li class="row placeholder">
          <span class="skeleton" style="width: {70 + ((i * 23) % 50)}px"></span>
          <span class="skeleton" style="width: 52px"></span>
          <span class="skeleton wide" style="width: {110 + ((i * 31) % 80)}px"></span>
        </li>
      {/each}
    </ol>
  {:else if total === 0}
    <div class="message">
      {#if failed}
        <p>Couldn't reach the train feeds. Trying again shortly.</p>
      {:else}
        <p>No trains in view.</p>
      {/if}
      <p class="coverage">{TRAIN_COVERAGE}</p>
    </div>
  {:else if trains.length === 0}
    <div class="message">
      <p>No trains in view match the search and filters.</p>
      <button class="button" onclick={onshowall}>Show all trains</button>
    </div>
  {:else}
    <ol>
      {#each rows as t (t.id)}
        {@const delay = delayText(t.delayMinutes)}
        <li>
          <button
            class="row"
            class:active={t.id === selectedId}
            onclick={() => onpick(t)}
            onmouseenter={() => onhover?.(t.id)}
            onmouseleave={() => onhover?.(null)}
            onfocus={() => onhover?.(t.id)}
            onblur={() => onhover?.(null)}
          >
            <span class="name" title={t.name}>
              <span class="swatch" style:background={swatch(t)} title="{trainLabel(t.kind)} train"></span>
              <span class="text">{t.name}</span>
            </span>
            {#if delay}
              <span class="badge {delayTone(t.delayMinutes) ?? 'neutral'}">{capital(delay)}</span>
            {:else}
              <span></span>
            {/if}
            <span class="sub">
              {#if t.destination}
                <ArrowRight size={11} strokeWidth={2} />
                <span class="dest" title={t.destination}>{t.destination}</span>
              {:else}
                <span class="dest quiet">{trainLabel(t.kind)}</span>
              {/if}
            </span>
            <span class="measure data">
              {#if sort === "distance"}
                {distanceLabel(distanceKm(t.lat, t.lon, centre))}
              {:else if t.speedKmh !== null}
                {Math.round(t.speedKmh)} km/h
              {/if}
            </span>
          </button>
        </li>
      {/each}
    </ol>
    {#if trains.length > LIMIT}
      <p class="more">{(trains.length - LIMIT).toLocaleString("en-GB")} more; search or filter to narrow the list.</p>
    {/if}
    <p class="coverage foot">{TRAIN_COVERAGE}</p>
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
    border-radius: 0;
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
  /* the train's colour on the map, as its key */
  .swatch {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 0;
  }
  .sub {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    min-width: 0;
    padding-left: 16px;
    font-size: var(--text-sm);
    color: var(--text-2);
  }
  .sub :global(svg) {
    flex: none;
    color: var(--text-3);
  }
  .dest {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .quiet {
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
