<script lang="ts">
  import { tick, type Component } from "svelte";
  import {
    Building,
    Flag,
    House,
    Landmark,
    LoaderCircle,
    Mailbox,
    Map as MapIcon,
    MapPin,
    Plane,
    PlaneLanding,
    Search,
    Signpost,
  } from "@lucide/svelte";
  import { geocode, search, type FindEntry, type Place, type PlaceKind } from "./api";
  import { placeDetail, placeLanguage } from "./places";

  interface Props {
    onpick: (entry: FindEntry) => void;
    /** A place or address was picked. */
    onplace?: (place: Place) => void;
    /** The user started a new search. */
    onquery?: () => void;
    /** Rank places near here (the view's centre), when zoomed in enough for that to mean something. */
    near?: { lat: number; lon: number } | null;
    /** The lookups, replaceable for a harness without Tauri. */
    find?: (query: string) => Promise<FindEntry[]>;
    lookup?: (query: string, near?: { lat: number; lon: number } | null, lang?: string) => Promise<Place[]>;
  }

  let { onpick, onplace, onquery, near = null, find = search, lookup = geocode }: Props = $props();

  type Phase<T> = { kind: "idle" } | { kind: "searching" } | { kind: "done"; results: T[] } | { kind: "error" };
  type Item = { key: string; flight: FindEntry; place?: never } | { key: string; place: Place; flight?: never };

  /** FR24 answers fast: a short pause is enough. */
  const FLIGHT_DEBOUNCE_MS = 250;
  /** Photon is a shared free service: wait until typing pauses (its fair use asks ≥ 300 ms). */
  const PLACE_DEBOUNCE_MS = 350;
  const PLACE_MIN_CHARS = 3;

  let input: HTMLInputElement;
  let list = $state<HTMLDivElement>();
  let query = $state("");
  let flights = $state.raw<Phase<FindEntry>>({ kind: "idle" });
  let places = $state.raw<Phase<Place>>({ kind: "idle" });
  let open = $state(false);
  /** The highlighted row, by key, so it stays put when the other group's results arrive. */
  let activeKey = $state<string | null>(null);
  let flightTimer: ReturnType<typeof setTimeout> | undefined;
  let placeTimer: ReturnType<typeof setTimeout> | undefined;
  let flightLatest = 0;
  let placeLatest = 0;

  // only these can be shown on the globe
  const selectable = (e: FindEntry) => e.type === "live" || e.type === "airport";
  const settled = (p: Phase<unknown>) => p.kind === "done" || p.kind === "error";

  const flightItems = $derived<Item[]>(
    flights.kind === "done" ? flights.results.map((e) => ({ key: `f:${e.type}:${e.id}`, flight: e })) : [],
  );
  const placeItems = $derived<Item[]>(
    places.kind === "done" ? places.results.map((p) => ({ key: `p:${p.kind}:${p.label}:${p.lat},${p.lon}`, place: p })) : [],
  );
  const items = $derived([...flightItems, ...placeItems]);
  const active = $derived(Math.max(0, items.findIndex((i) => i.key === activeKey)));
  const showPlaces = $derived(places.kind !== "idle");
  const searching = $derived(flights.kind === "searching" || places.kind === "searching");
  const visible = $derived(open && (settled(flights) || settled(places)));

  export function focus() {
    input.focus();
    input.select();
  }

  function oninput() {
    clearTimeout(flightTimer);
    clearTimeout(placeTimer);
    open = true;
    activeKey = null;
    onquery?.();
    const q = query.trim();
    if (q.length < 2) {
      flightLatest++;
      flights = { kind: "idle" };
    } else {
      flights = { kind: "searching" };
      flightTimer = setTimeout(runFlights, FLIGHT_DEBOUNCE_MS);
    }
    if (q.length < PLACE_MIN_CHARS) {
      placeLatest++;
      places = { kind: "idle" };
    } else {
      places = { kind: "searching" };
      placeTimer = setTimeout(runPlaces, PLACE_DEBOUNCE_MS);
    }
  }

  async function runFlights() {
    const id = ++flightLatest;
    try {
      const found = (await find(query.trim())).filter(selectable);
      if (id === flightLatest) flights = { kind: "done", results: found };
    } catch {
      if (id === flightLatest) flights = { kind: "error" };
    }
  }

  // independent of the flight search: a slow or resting geocoder never holds flights back
  async function runPlaces() {
    const id = ++placeLatest;
    try {
      const found = await lookup(query.trim(), near, placeLanguage());
      if (id === placeLatest) places = { kind: "done", results: found };
    } catch {
      if (id === placeLatest) places = { kind: "error" };
    }
  }

  function reset() {
    clearTimeout(flightTimer);
    clearTimeout(placeTimer);
    flightLatest++;
    placeLatest++;
    open = false;
    query = "";
    flights = { kind: "idle" };
    places = { kind: "idle" };
    activeKey = null;
    input.blur();
  }

  function pick(item: Item) {
    reset();
    if (item.flight) onpick(item.flight);
    else onplace?.(item.place);
  }

  async function move(step: number) {
    activeKey = items[(active + step + items.length) % items.length].key;
    await tick();
    list?.querySelector<HTMLElement>(`[aria-selected="true"]`)?.scrollIntoView({ block: "nearest" });
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "ArrowDown" && visible && items.length) {
      e.preventDefault();
      move(1);
    } else if (e.key === "ArrowUp" && visible && items.length) {
      e.preventDefault();
      move(-1);
    } else if (e.key === "Enter" && visible && items[active]) {
      pick(items[active]);
    } else if (e.key === "Escape") {
      e.stopPropagation();
      open = false;
      input.blur();
    }
  }

  const detailOf = (e: FindEntry) =>
    e.type === "live" ? (e.detail.route ?? e.detail.ac_type ?? "") : e.type === "airport" ? "Airport" : "";

  const PLACE_ICON: Record<PlaceKind, Component<{ size?: number; strokeWidth?: number }>> = {
    address: House,
    street: Signpost,
    poi: Landmark,
    airport: PlaneLanding,
    district: Building,
    city: Building,
    town: Building,
    postcode: Mailbox,
    region: MapIcon,
    country: Flag,
  };
</script>

{#snippet row(item: Item, i: number)}
  <button
    id="result-{i}"
    role="option"
    aria-selected={i === active}
    class:active={i === active}
    onmouseenter={() => (activeKey = item.key)}
    onmousedown={(e) => e.preventDefault()}
    onclick={() => pick(item)}
  >
    {#if item.flight}
      <span class="kind {item.flight.type}">
        {#if item.flight.type === "live"}<Plane size={14} strokeWidth={1.75} />{:else}<MapPin size={14} strokeWidth={1.75} />{/if}
      </span>
      <span class="text">
        <span class="label">{item.flight.label}</span>
        {#if detailOf(item.flight)}<span class="detail">{detailOf(item.flight)}</span>{/if}
      </span>
    {:else}
      {@const Icon = PLACE_ICON[item.place.kind] ?? MapPin}
      <span class="kind place"><Icon size={14} strokeWidth={1.75} /></span>
      <span class="text">
        <span class="label">{item.place.name}</span>
        <span class="detail" title={item.place.label}>{placeDetail(item.place)}</span>
      </span>
    {/if}
  </button>
{/snippet}

<div class="search">
  <label class="field">
    <span class="visually-hidden">Search flights, airports and places</span>
    <Search size={16} strokeWidth={1.75} />
    <input
      bind:this={input}
      type="search"
      placeholder="Flight, airport or address"
      bind:value={query}
      {oninput}
      {onkeydown}
      onfocus={() => (open = true)}
      onblur={() => setTimeout(() => (open = false), 120)}
      spellcheck="false"
      autocomplete="off"
      role="combobox"
      aria-expanded={visible}
      aria-controls="search-results"
      aria-activedescendant={visible && items[active] ? `result-${active}` : undefined}
    />
    {#if searching}
      <span class="spin" aria-label="Searching"><LoaderCircle size={14} strokeWidth={2} /></span>
    {:else if !query}
      <kbd title="Press / to search">/</kbd>
    {/if}
  </label>

  {#if visible}
    <div class="results surface" id="search-results" role="listbox" bind:this={list}>
      <div role="group" aria-labelledby={showPlaces ? "flights-heading" : undefined}>
        {#if showPlaces}<h3 class="group" id="flights-heading">Flights and airports</h3>{/if}
        {#if flights.kind === "searching"}
          <p class="message quiet"><span class="spin"><LoaderCircle size={12} strokeWidth={2} /></span> Searching flights…</p>
        {:else if flights.kind === "error"}
          <p class="message">Flight search failed. Check your connection and try again.</p>
        {:else if flightItems.length === 0}
          <p class="message">No live flights or airports match “{query.trim()}”.</p>
        {:else}
          {#each flightItems as item, i (item.key)}{@render row(item, i)}{/each}
        {/if}
      </div>

      {#if showPlaces}
        <div role="group" aria-labelledby="places-heading">
          <h3 class="group" id="places-heading">
            Places
            <!-- ODbL: credited wherever the data is shown, and in view without scrolling -->
            {#if placeItems.length}<span class="credit">© OpenStreetMap contributors, via Photon</span>{/if}
          </h3>
          {#if places.kind === "searching"}
            <p class="message quiet"><span class="spin"><LoaderCircle size={12} strokeWidth={2} /></span> Looking up places…</p>
          {:else if places.kind === "error"}
            <p class="message quiet">Places unavailable</p>
          {:else if placeItems.length === 0}
            <p class="message">No places match “{query.trim()}”.</p>
          {:else}
            {#each placeItems as item, i (item.key)}{@render row(item, flightItems.length + i)}{/each}
          {/if}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .search {
    position: relative;
  }
  .field {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    height: 36px;
    padding: 0 var(--space-3);
    border: 1px solid var(--line-control);
    border-radius: var(--radius-sm);
    background: var(--ink);
    color: var(--text-3);
    transition: border-color 150ms var(--ease-out);
  }
  .field:focus-within {
    border-color: var(--amber);
    color: var(--text-2);
  }
  input {
    flex: 1;
    min-width: 0;
    height: 100%;
    border: 0;
    outline: none;
    background: none;
    font-size: var(--text-base);
    color: var(--text-1);
  }
  input::placeholder {
    color: var(--text-3);
  }
  input::-webkit-search-cancel-button {
    display: none;
  }
  kbd {
    display: inline-grid;
    place-items: center;
    min-width: 18px;
    height: 18px;
    border: 1px solid var(--line-control);
    border-radius: 4px;
    font: 600 var(--text-xs) var(--font-data);
    color: var(--text-3);
  }
  .spin {
    display: inline-grid;
    animation: spin 900ms linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .results {
    position: absolute;
    top: calc(100% + var(--space-2));
    left: 0;
    right: 0;
    z-index: 10;
    max-height: min(440px, 70vh);
    overflow-y: auto;
    padding: var(--space-1);
    background: var(--surface-raised);
  }
  .group {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-2) var(--space-1);
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-3);
  }
  [role="group"] + [role="group"] {
    margin-top: var(--space-1);
    border-top: 1px solid var(--line);
  }
  .message {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-2) var(--space-3);
    font-size: var(--text-md);
    color: var(--text-2);
  }
  .message.quiet {
    color: var(--text-3);
  }
  .credit {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-size: var(--text-xs);
    font-weight: 400;
    color: var(--text-3);
  }
  .results button {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
    padding: var(--space-2);
    border: 0;
    border-radius: var(--radius-sm);
    background: none;
    text-align: left;
    cursor: pointer;
  }
  .results button.active {
    background: var(--surface-hover);
  }
  .kind {
    flex: none;
    display: inline-grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border-radius: var(--radius-sm);
    background: var(--surface);
  }
  .kind.live {
    color: var(--amber);
  }
  .kind.airport,
  .kind.place {
    color: var(--text-2);
  }
  .text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .label,
  .detail {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .label {
    font-size: var(--text-md);
  }
  .detail {
    font-size: var(--text-sm);
    color: var(--text-3);
  }
</style>
