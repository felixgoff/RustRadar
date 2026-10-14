<script lang="ts" module>
  import type { Component } from "svelte";

  /** A row for something loaded on the map, found without asking a server. */
  export interface LocalItem {
    key: string;
    label: string;
    detail: string;
    /** Its colour on the map, as a key. */
    color?: string;
  }

  /** Searching the mode's own loaded traffic (trains, ships) in place of Flightradar24's flights and airports. */
  export interface LocalSearch {
    /** The group's heading: "Trains in view". */
    heading: string;
    /** For the empty message: "trains". */
    noun: string;
    icon: Component<{ size?: number; strokeWidth?: number }>;
    placeholder: string;
    search: (query: string) => LocalItem[];
    onpick: (key: string) => void;
  }
</script>

<script lang="ts">
  import { tick } from "svelte";
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
    X,
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
    /** Search the loaded trains or ships instead of flights and airports; places stay. */
    local?: LocalSearch | null;
    /** What is typed: in a local search it narrows the sidebar's list too. */
    query?: string;
  }

  let {
    onpick,
    onplace,
    onquery,
    near = null,
    find = search,
    lookup = geocode,
    local = null,
    query = $bindable(""),
  }: Props = $props();

  /** The most rows a local search lists: the sidebar's list has the rest. */
  const LOCAL_MAX = 6;

  type Phase<T> = { kind: "idle" } | { kind: "searching" } | { kind: "done"; results: T[] } | { kind: "error" };
  type Item =
    | { key: string; flight: FindEntry; place?: never; local?: never }
    | { key: string; place: Place; flight?: never; local?: never }
    | { key: string; local: LocalItem; flight?: never; place?: never };

  /** FR24 answers fast: a short pause is enough. */
  const FLIGHT_DEBOUNCE_MS = 250;
  /** Photon is a shared free service: wait until typing pauses (its fair use asks ≥ 300 ms). */
  const PLACE_DEBOUNCE_MS = 350;
  const PLACE_MIN_CHARS = 3;

  let input: HTMLInputElement;
  let list = $state<HTMLDivElement>();
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
  const localItems = $derived<Item[]>(
    local && query.trim() ? local.search(query.trim()).slice(0, LOCAL_MAX).map((l) => ({ key: `l:${l.key}`, local: l })) : [],
  );
  const firstItems = $derived(local ? localItems : flightItems);
  const items = $derived([...firstItems, ...placeItems]);
  const active = $derived(Math.max(0, items.findIndex((i) => i.key === activeKey)));
  const showPlaces = $derived(places.kind !== "idle");
  const searching = $derived(flights.kind === "searching" || places.kind === "searching");
  const visible = $derived(open && (local ? query.trim().length > 0 : settled(flights) || settled(places)));

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
    if (local || q.length < 2) {
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
    else if (item.local) local?.onpick(item.local.key);
    else onplace?.(item.place);
  }

  /** Empties the field, which also lets go of the list it narrowed. */
  function clear() {
    reset();
    onquery?.();
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
    {#if item.local}
      {@const Icon = local?.icon ?? MapPin}
      <span class="kind local" style:--swatch={item.local.color}><Icon size={14} strokeWidth={1.75} /></span>
      <span class="text">
        <span class="label">{item.local.label}</span>
        {#if item.local.detail}<span class="detail">{item.local.detail}</span>{/if}
      </span>
    {:else if item.flight}
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
    <span class="visually-hidden">{local ? `Search ${local.noun} and places` : "Search flights, airports and places"}</span>
    <Search size={16} strokeWidth={1.75} />
    <input
      bind:this={input}
      type="search"
      placeholder={local?.placeholder ?? "Flight, airport or address"}
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
    {:else if local}
      <!-- the typed words narrow the list below: clearing them is one click -->
      <button class="clear" aria-label="Clear the search" title="Clear the search" onmousedown={(e) => e.preventDefault()} onclick={clear}>
        <X size={14} strokeWidth={1.75} />
      </button>
    {/if}
  </label>

  {#if visible}
    <div class="results surface" id="search-results" role="listbox" bind:this={list}>
      <div role="group" aria-labelledby={showPlaces || local ? "flights-heading" : undefined}>
        {#if local}
          <h3 class="group" id="flights-heading">{local.heading}</h3>
          {#if localItems.length === 0}
            <p class="message">No {local.noun} in view match “{query.trim()}”.</p>
          {:else}
            {#each localItems as item, i (item.key)}{@render row(item, i)}{/each}
          {/if}
        {:else}
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
            {#each placeItems as item, i (item.key)}{@render row(item, firstItems.length + i)}{/each}
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
  /* a train or ship: its colour on the map */
  .kind.local {
    color: var(--swatch, var(--text-2));
  }
  .clear {
    display: inline-grid;
    place-items: center;
    width: 22px;
    height: 22px;
    margin-right: -4px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: none;
    color: var(--text-3);
    cursor: pointer;
  }
  .clear:hover {
    background: var(--surface-hover);
    color: var(--text-1);
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
