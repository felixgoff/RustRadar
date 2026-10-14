<script lang="ts">
  import { Check, X } from "@lucide/svelte";
  import type { LiveFlight } from "./api";
  import { activeCount, CATEGORIES, DEFAULT_FILTERS, MAX_ALTITUDE, type Filters } from "./filters";
  import { airlineIcao, reference } from "./reference.svelte";
  import TokenInput, { type Suggestion } from "./TokenInput.svelte";

  interface Props {
    filters: Filters;
    /** Everything loaded, before filtering: for suggestions and their counts. */
    flights: LiveFlight[];
    shown: number;
    onclose: () => void;
  }

  let { filters = $bindable(), flights, shown, onclose }: Props = $props();

  const STEP = 1_000;
  const SUGGESTIONS = 6;
  const count = (n: number) => n.toLocaleString();

  const active = $derived(activeCount(filters));

  function reset() {
    Object.assign(filters, structuredClone(DEFAULT_FILTERS));
  }

  function toggleCategory(id: number) {
    filters.categories = filters.categories.includes(id)
      ? filters.categories.filter((c) => c !== id)
      : [...filters.categories, id].sort((a, b) => a - b);
  }

  // altitude: two thumbs on one rail, kept a step apart
  const low = $derived(filters.altitude[0]);
  const high = $derived(filters.altitude[1]);
  function setLow(e: Event & { currentTarget: HTMLInputElement }) {
    const v = Math.min(Number(e.currentTarget.value), high - STEP);
    filters.altitude = [v, high];
    e.currentTarget.value = String(v);
  }
  function setHigh(e: Event & { currentTarget: HTMLInputElement }) {
    const v = Math.max(Number(e.currentTarget.value), low + STEP);
    filters.altitude = [low, v];
    e.currentTarget.value = String(v);
  }
  const ft = (v: number) => `${v.toLocaleString()} ft`;
  const altitudeText = $derived(
    low === 0 && high >= MAX_ALTITUDE
      ? "Any altitude"
      : high >= MAX_ALTITUDE
        ? `Above ${ft(low)}`
        : low === 0
          ? `Below ${ft(high)}`
          : `${low.toLocaleString()} – ${ft(high)}`,
  );

  // live counts behind the suggestions
  const tally = (key: (f: LiveFlight) => string | null) => {
    const counts = new Map<string, number>();
    for (const f of flights) {
      const k = key(f);
      if (k) counts.set(k, (counts.get(k) ?? 0) + 1);
    }
    return counts;
  };
  const airlineCounts = $derived(tally((f) => airlineIcao(f.callsign)));
  const typeCounts = $derived(tally((f) => f.typecode || null));
  const airportCounts = $derived.by(() => {
    const counts = tally((f) => f.origin || null);
    for (const f of flights) if (f.destination) counts.set(f.destination, (counts.get(f.destination) ?? 0) + 1);
    return counts;
  });
  const live = (n: number | undefined) => (n ? `${count(n)} live` : "");

  function suggestAirlines(q: string): Suggestion[] {
    const query = q.trim().toUpperCase();
    const picked = new Set(filters.airlines);
    return reference.airlines
      .filter(
        (a) =>
          a.icao &&
          !picked.has(a.icao) &&
          (query
            ? a.icao.startsWith(query) || a.iata === query || a.name.toUpperCase().includes(query)
            : airlineCounts.has(a.icao)),
      )
      .map((a) => ({ a, n: airlineCounts.get(a.icao) ?? 0, exact: a.icao === query || a.iata === query }))
      .sort((x, y) => Number(y.exact) - Number(x.exact) || y.n - x.n || x.a.name.localeCompare(y.a.name))
      .slice(0, SUGGESTIONS)
      .map(({ a, n }) => ({ value: a.icao, label: a.name, detail: [a.icao, live(n)].filter(Boolean).join(" · ") }));
  }

  function suggestTypes(q: string): Suggestion[] {
    const query = q.trim().toUpperCase();
    const picked = new Set(filters.types);
    return [...typeCounts]
      .filter(([code]) => code.startsWith(query) && !picked.has(code))
      .sort((x, y) => y[1] - x[1])
      .slice(0, SUGGESTIONS)
      .map(([code, n]) => ({ value: code, label: code, detail: live(n) }));
  }

  function suggestAirports(q: string): Suggestion[] {
    const query = q.trim().toUpperCase();
    const picked = new Set(filters.airports);
    return reference.airports
      .filter(
        (a) =>
          a.iata &&
          !picked.has(a.iata) &&
          (query
            ? a.iata.startsWith(query) ||
              a.icao.startsWith(query) ||
              a.name.toUpperCase().includes(query) ||
              a.city.toUpperCase().includes(query)
            : airportCounts.has(a.iata)),
      )
      .map((a) => ({ a, n: airportCounts.get(a.iata) ?? 0, exact: a.iata === query || a.icao === query }))
      .sort((x, y) => Number(y.exact) - Number(x.exact) || y.n - x.n || y.a.size - x.a.size)
      .slice(0, SUGGESTIONS)
      .map(({ a, n }) => ({
        value: a.iata,
        label: a.city ? `${a.city} · ${a.name}` : a.name,
        detail: [a.iata, live(n)].filter(Boolean).join(" · "),
      }));
  }

  const airlineChip = (icao: string) => {
    const name = reference.airline(icao)?.name;
    return name ? `${icao} ${name}` : icao;
  };
  const airportChip = (iata: string) => {
    const a = reference.airport(iata);
    return a?.city ? `${iata} ${a.city}` : iata;
  };
</script>

<section class="filters surface" aria-labelledby="filters-heading">
  <header>
    <h2 id="filters-heading">Filters</h2>
    <span class="shown">{count(shown)} of {count(flights.length)} shown</span>
    <button class="text-button" onclick={reset} disabled={!active}>Reset</button>
    <button class="icon-button" aria-label="Close filters" title="Close filters (F)" onclick={onclose}>
      <X size={16} strokeWidth={1.75} />
    </button>
  </header>

  <div class="body">
    <label class="field">
      <span class="label">Callsign, flight or registration</span>
      <input type="text" bind:value={filters.text} placeholder="e.g. BAW, LH400, D-AIM" spellcheck="false" autocomplete="off" />
    </label>

    <fieldset>
      <legend>
        <span class="label">Categories</span>
        <span class="links">
          <button class="text-button" onclick={() => (filters.categories = CATEGORIES.map((c) => c.id))}>All</button>
          <button class="text-button" onclick={() => (filters.categories = [])}>None</button>
        </span>
      </legend>
      <div class="chips">
        {#each CATEGORIES as c (c.id)}
          {@const on = filters.categories.includes(c.id)}
          <button class="category" aria-pressed={on} onclick={() => toggleCategory(c.id)}>
            {#if on}<Check size={12} strokeWidth={2.5} />{/if}
            {c.label}
          </button>
        {/each}
      </div>
    </fieldset>

    <fieldset>
      <legend>
        <span class="label">Altitude</span>
        <output class="data value">{altitudeText}</output>
      </legend>
      <div class="dual" style:--low={low / MAX_ALTITUDE} style:--high={high / MAX_ALTITUDE}>
        <div class="rail"><div class="span"></div></div>
        <input
          type="range"
          min="0"
          max={MAX_ALTITUDE}
          step={STEP}
          value={low}
          oninput={setLow}
          aria-label="Lowest altitude"
          aria-valuetext={ft(low)}
          style:z-index={low > MAX_ALTITUDE - STEP * 3 ? 2 : 1}
        />
        <input
          type="range"
          min="0"
          max={MAX_ALTITUDE}
          step={STEP}
          value={high}
          oninput={setHigh}
          aria-label="Highest altitude"
          aria-valuetext={high >= MAX_ALTITUDE ? "No limit" : ft(high)}
        />
      </div>
      <div class="checks">
        <label><input type="checkbox" bind:checked={filters.airborne} /> In the air</label>
        <label><input type="checkbox" bind:checked={filters.onGround} /> On the ground</label>
      </div>
    </fieldset>

    <TokenInput
      label="Airlines"
      bind:values={filters.airlines}
      suggest={suggestAirlines}
      free={(q) => (/^[A-Z]{3}$/i.test(q) ? q.toUpperCase() : null)}
      chip={airlineChip}
      placeholder="Name or code"
    />
    <TokenInput
      label="Aircraft types"
      bind:values={filters.types}
      suggest={suggestTypes}
      free={(q) => (/^[A-Z0-9]{2,4}$/i.test(q) ? q.toUpperCase() : null)}
      placeholder="e.g. A388 or B77"
      hint="A type code, or its start: B77 matches every Boeing 777."
    />
    <TokenInput
      label="Airports"
      bind:values={filters.airports}
      suggest={suggestAirports}
      free={(q) => (/^[A-Z]{3}$/i.test(q) ? q.toUpperCase() : null)}
      chip={airportChip}
      placeholder="City, name or code"
      hint="Flights departing from or flying to any of them."
    />
  </div>
</section>

<style>
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
  .label {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-2);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .field input {
    height: 34px;
    padding: 0 var(--space-3);
    border: 1px solid var(--line-control);
    border-radius: var(--radius-sm);
    background: var(--ink);
    font-size: var(--text-md);
    outline: none;
    transition: border-color 150ms var(--ease-out);
  }
  .field input:focus {
    border-color: var(--amber);
  }
  .field input::placeholder {
    color: var(--text-3);
  }

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
  .links {
    display: flex;
    gap: var(--space-1);
  }
  .value {
    font-size: var(--text-sm);
    color: var(--text-1);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
  }
  .category {
    display: inline-flex;
    align-items: center;
    gap: 3px;
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
  .category:hover {
    background: var(--surface-hover);
  }
  .category[aria-pressed="true"] {
    border-color: var(--line-control);
    background: var(--surface-raised);
    color: var(--text-1);
  }
  .category :global(svg) {
    color: var(--amber);
  }
  .category[aria-pressed="false"] {
    color: var(--text-3);
  }

  /* two range inputs share one rail; only their thumbs take the pointer */
  .dual {
    position: relative;
    height: 20px;
  }
  .rail {
    position: absolute;
    top: 9px;
    left: 8px;
    right: 8px;
    height: 2px;
    border-radius: 1px;
    background: var(--line-control);
  }
  .span {
    position: absolute;
    top: 0;
    bottom: 0;
    left: calc(var(--low) * 100%);
    right: calc((1 - var(--high)) * 100%);
    background: var(--amber);
  }
  .dual input {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 20px;
    margin: 0;
    appearance: none;
    background: none;
    pointer-events: none;
  }
  .dual input::-webkit-slider-runnable-track {
    height: 20px;
    background: none;
  }
  .dual input::-webkit-slider-thumb {
    appearance: none;
    width: 16px;
    height: 16px;
    margin-top: 2px;
    border: 2px solid var(--amber);
    border-radius: 50%;
    background: var(--ink);
    pointer-events: auto;
    cursor: grab;
  }
  .dual input:focus-visible {
    outline: none;
  }
  .dual input:focus-visible::-webkit-slider-thumb {
    outline: 2px solid var(--amber);
    outline-offset: 2px;
  }
  .checks {
    display: flex;
    gap: var(--space-4);
    font-size: var(--text-md);
  }
  .checks label {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    cursor: pointer;
  }
  .checks input {
    margin: 0;
  }
</style>
