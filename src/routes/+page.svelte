<script lang="ts">
  import { onDestroy, onMount, untrack } from "svelte";
  import { Funnel, Radar, RefreshCw, TriangleAlert } from "@lucide/svelte";
  import {
    followFlight,
    isRateLimited,
    isTauri,
    liveFlights,
    openskyTrack,
    sessionInfo,
    signIn,
    signOut,
    topFlights,
    unfollowFlight,
    type BoundingBox,
    type FindEntry,
    type FlightDetails,
    type FollowEvent,
    type LiveFlight,
    type LiveSnapshot,
    type OpenSkyPoint,
    type SessionInfo,
    type TopFlight,
  } from "$lib/api";
  import AirportPanel from "$lib/AirportPanel.svelte";
  import FilterPanel from "$lib/FilterPanel.svelte";
  import FlightPanel, { type FollowState } from "$lib/FlightPanel.svelte";
  import Globe, { type ViewState } from "$lib/Globe.svelte";
  import MapControls from "$lib/MapControls.svelte";
  import SearchBox from "$lib/SearchBox.svelte";
  import ShortcutsHelp from "$lib/ShortcutsHelp.svelte";
  import TopFlights from "$lib/TopFlights.svelte";
  import { activeCount, loadPersisted, matcher, persist, serverCategories, type Filters } from "$lib/filters";
  import { cosAngle, viewArea } from "$lib/geo";
  import { motion } from "$lib/motion";
  import { reference } from "$lib/reference.svelte";
  import { mergeTrail, routeFor } from "$lib/routes";

  // A world refresh costs ~20 requests, a regional one a handful: poll the
  // world less often, and back off when Flightradar24 says to slow down.
  const WORLD_REFRESH_MS = 15_000;
  const AREA_REFRESH_MS = 8_000;
  const MIN_GAP_MS = 4_000;
  const MAX_BACKOFF = 6;
  const TOP_REFRESH_MS = 60_000;
  const TILT = 55;
  /** The right-hand panels' width plus their gap, for the map controls. */
  const PANEL_INSET = 360 + 12;

  let globe: Globe;
  let searchBox: SearchBox;
  let controls: MapControls;
  // large arrays are kept as raw state: deep proxies over ~20k flights are slow
  let snapshot = $state.raw<LiveSnapshot | null>(null);
  let top = $state.raw<TopFlight[] | null>(null);
  let topFailed = $state(false);
  let details = $state.raw<FlightDetails | null>(null);
  /** The selected aircraft's track from the OpenSky Network, if it has one. */
  let openskyPoints = $state.raw<{ icao24: string; points: OpenSkyPoint[] } | null>(null);
  let selectedId = $state<number | null>(null);
  /** What we know about a selection before it appears in the feed. */
  let hint = $state.raw<LiveFlight | null>(null);
  let followState = $state<FollowState>("connecting");
  let feedError = $state<string | null>(null);
  let rateLimited = $state(false);
  let loading = $state(false);
  let lastUpdate = $state(0);
  let nextRefreshAt = $state(0);
  /** Whether the latest snapshot covers only the visible area. */
  let regional = $state(false);
  let backoff = 1;
  let lastRequestAt = 0;
  /** A refresh was asked for while one was running. */
  let refreshAgain = false;
  let fetchedArea: { area: BoundingBox | null; view: ViewState } | null = null;
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;
  let now = $state(Date.now());
  let view = $state<ViewState>({ longitude: 0, latitude: 0, zoom: 2, bearing: 0, pitch: 0 });

  const persisted = loadPersisted();
  let settings = $state(persisted.settings);
  let filters = $state(persisted.filters);
  let filtersOpen = $state(false);
  let helpOpen = $state(false);
  let session = $state.raw<SessionInfo | null>(null);
  /** The camera follows the selected aircraft. */
  let following = $state(false);
  /** IATA code of the airport whose board is open. */
  let boardCode = $state<string | null>(null);
  /** The open flight was picked from that board. */
  let fromBoard = $state(false);
  /** Fly to the selected flight once its position is known. */
  let flyPending = false;
  /** Zoom for that pending fly-to; undefined lets `flyTo` choose one. */
  let flyZoom: number | undefined;

  $effect(() => persist($state.snapshot(settings), $state.snapshot(filters)));

  const flights = $derived(snapshot?.flights ?? []);

  // field elevations, for landings, takeoffs and heights near airports
  $effect(() => {
    if (reference.ready) motion.setAirports(reference.airports);
  });
  const byId = $derived(new Map(flights.map((f) => [f.id, f])));
  const filtered = $derived(flights.filter(matcher($state.snapshot(filters) as Filters)));
  const active = $derived(activeCount(filters));

  // the follow stream updates every few seconds, the feed less often: use the fresher one
  const selected = $derived.by<LiveFlight | null>(() => {
    if (selectedId === null) return null;
    const feed = byId.get(selectedId) ?? hint;
    const live = details?.flight;
    if (!live || (feed && feed.timestampMs > live.timestamp_ms)) return feed;
    return {
      id: selectedId,
      lat: live.latitude,
      lon: live.longitude,
      track: live.track,
      alt: live.altitude,
      speed: live.ground_speed,
      onGround: live.on_ground,
      timestampMs: live.timestamp_ms,
      callsign: live.callsign || feed?.callsign || "",
      flight: feed?.flight || details?.schedule?.flight_number || "",
      reg: feed?.reg || details?.aircraft?.reg || "",
      typecode: feed?.typecode || details?.aircraft?.typecode || "",
      origin: feed?.origin ?? "",
      destination: feed?.destination ?? "",
      icon: feed?.icon ?? "",
    };
  });

  /**
   * What the globe draws: the filtered flights, with the selection substituted
   * in (and added when filtered out). The substitution matters: the selected
   * aircraft's model and its trail's tip must come from the same object, or
   * the model lags behind the trail every time the follow stream beats the
   * feed, then jumps forward when the feed catches up.
   */
  const shown = $derived.by(() => {
    if (!selected) return filtered;
    const i = filtered.findIndex((f) => f.id === selected.id);
    if (i === -1) return [...filtered, selected];
    const list = [...filtered];
    list[i] = selected;
    return list;
  });

  // OpenSky often saw the start of a flight that Flightradar24's trail lacks
  const icao24 = $derived(
    details?.aircraft?.icao_address ? details.aircraft.icao_address.toString(16).padStart(6, "0") : null,
  );
  const merged = $derived.by(() => {
    if (!details) return { details, added: 0 };
    const own = openskyPoints?.icao24 === icao24 ? openskyPoints.points : [];
    const { trail, added } = mergeTrail(details.trail, own);
    return { details: added ? { ...details, trail } : details, added };
  });
  const shownDetails = $derived(merged.details);

  const route = $derived.by(() => {
    void reference.ready; // airports resolve once the reference data is in
    return selected ? routeFor(selected, shownDetails) : null;
  });

  // fetch the track once the aircraft is known, then now and then while it is open
  const OPENSKY_REFRESH_MS = 300_000;
  $effect(() => {
    const code = icao24;
    if (!code || !isTauri()) return;
    let stopped = false;
    const load = () =>
      openskyTrack(code)
        .then((points) => {
          if (!stopped) openskyPoints = { icao24: code, points };
        })
        .catch(() => {}); // no track, or OpenSky's allowance is used up: Flightradar24's trail stands
    load();
    const timer = setInterval(load, OPENSKY_REFRESH_MS);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  });
  const boardAirport = $derived.by(() => {
    void reference.ready;
    return reference.airport(boardCode);
  });
  const authenticated = $derived(session?.authenticated ?? snapshot?.authenticated ?? false);

  function scheduleRefresh(ms: number) {
    clearTimeout(refreshTimer);
    nextRefreshAt = Date.now() + ms;
    refreshTimer = setTimeout(refresh, ms);
  }

  /** Refresh soon, without crowding Flightradar24. */
  function refreshSoon() {
    if (!isTauri() || backoff > 1) return;
    if (loading) {
      refreshAgain = true;
      return;
    }
    scheduleRefresh(Math.max(300, lastRequestAt + MIN_GAP_MS - Date.now()));
  }

  async function refresh() {
    if (loading) return;
    loading = true;
    refreshAgain = false;
    const area = viewArea(view, window.innerWidth, window.innerHeight);
    fetchedArea = { area, view: { ...view } };
    lastRequestAt = Date.now();
    let limited = false;
    try {
      snapshot = await liveFlights(area, serverCategories($state.snapshot(filters) as Filters));
      // aircraft are drawn on the feed's clock, so a wrong system clock can't misplace them
      motion.syncClock(snapshot.serverTimeMs);
      regional = area !== null;
      lastUpdate = Date.now();
      feedError = null;
      limited = snapshot.errors.some(isRateLimited);
    } catch (e) {
      feedError = String(e);
      limited = isRateLimited(feedError);
    } finally {
      loading = false;
      rateLimited = limited;
      backoff = limited ? Math.min(backoff * 2, MAX_BACKOFF) : 1;
      const interval = (area ? AREA_REFRESH_MS : WORLD_REFRESH_MS) * backoff;
      scheduleRefresh(refreshAgain && !limited ? MIN_GAP_MS : interval);
    }
  }

  // after a large move, fetch the new area soon instead of waiting a full interval
  let viewTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const v = view;
    if (!fetchedArea || backoff > 1) return;
    const before = fetchedArea.view;
    const moved = cosAngle(v.longitude, v.latitude, before.longitude, before.latitude) < Math.cos((3 * Math.PI) / 180);
    const zoomed = Math.abs(v.zoom - before.zoom) > 1.5;
    if (!moved && !zoomed) return;
    clearTimeout(viewTimer);
    viewTimer = setTimeout(() => {
      const wait = Math.max(0, lastRequestAt + MIN_GAP_MS - Date.now());
      if (Date.now() + wait < nextRefreshAt) scheduleRefresh(wait);
    }, 800);
  });

  // categories are filtered by the server: changing them needs a new snapshot
  const categoryKey = $derived(filters.categories.join(","));
  let fetchedCategories = persisted.filters.categories.join(",");
  $effect(() => {
    const key = categoryKey;
    if (key === fetchedCategories) return;
    fetchedCategories = key;
    untrack(refreshSoon);
  });

  async function refreshTop() {
    try {
      top = await topFlights();
      topFailed = false;
    } catch {
      topFailed = true;
    }
  }

  function startFollow(id: number) {
    followState = "connecting";
    followFlight(id, (e: FollowEvent) => {
      if (selectedId !== id) return; // a late event for a previous selection
      if (e.event === "details") {
        details = e.data;
        followState = "live";
        if (flyPending && e.data.flight) {
          flyPending = false;
          globe.flyTo(e.data.flight.longitude, e.data.flight.latitude, flyZoom);
        }
      } else if (e.event === "error") {
        followState = "reconnecting";
      } else {
        followState = "ended";
      }
    }).catch(() => (followState = "ended"));
  }

  function select(
    id: number | null,
    opts: { hint?: LiveFlight; fly?: boolean; follow?: boolean; fromBoard?: boolean } = {},
  ) {
    if (id === selectedId && !opts.fly) return;
    selectedId = id;
    details = null;
    hint = opts.hint ?? null;
    flyPending = false;
    following = opts.follow ?? false;
    fromBoard = opts.fromBoard ?? false;
    if (id === null) {
      unfollowFlight();
      return;
    }
    const known = byId.get(id) ?? opts.hint;
    if (opts.fly) {
      // picks from a list fly in; a click on the map leaves the camera alone,
      // and only the panel's follow button (or C) locks it to the aircraft
      flyZoom = undefined;
      if (known) globe.flyTo(known.lon, known.lat, flyZoom);
      else flyPending = true;
    }
    startFollow(id);
  }

  function openAirport(code: string) {
    boardCode = code.toUpperCase();
    if (selectedId !== null) select(null);
  }

  function pickSearch(entry: FindEntry) {
    if (entry.type === "airport") {
      globe.flyTo(entry.detail.lon, entry.detail.lat, 11);
      // airport results are keyed by IATA code
      if (/^[A-Z0-9]{3}$/.test(entry.id)) openAirport(entry.id);
    } else if (entry.type === "live") {
      const id = parseInt(entry.id, 16);
      const { lat, lon, callsign } = entry.detail;
      select(id, {
        fly: true,
        hint: {
          id, lat, lon, track: 0, alt: 0, speed: 0, onGround: false, timestampMs: 0,
          callsign: callsign ?? "", flight: "", reg: "", typecode: "", origin: "", destination: "", icon: "",
        },
      });
    }
  }

  async function handleSignIn(email: string, password: string) {
    session = await signIn(email, password);
    // fetch again with the session: the feed, and the open flight's details (flight plan)
    refreshSoon();
    if (selectedId !== null) startFollow(selectedId);
  }

  async function handleSignOut() {
    session = await signOut();
    refreshSoon();
    if (selectedId !== null) startFollow(selectedId);
  }

  const toggles: Record<string, () => void> = {
    "/": () => searchBox.focus(),
    "?": () => (helpOpen = !helpOpen),
    f: () => (filtersOpen = !filtersOpen),
    c: () => selectedId !== null && (following = !following),
    t: () => globe.setPitch(view.pitch > 5 ? 0 : TILT),
    n: () => globe.resetOrientation(),
    "+": () => globe.zoomBy(1),
    "=": () => globe.zoomBy(1),
    "-": () => globe.zoomBy(-1),
    "1": () => (settings.basemap = "dark"),
    "2": () => (settings.basemap = "satellite"),
    l: () => (settings.labels = !settings.labels),
    b: () => (settings.buildings = !settings.buildings),
    p: () => (settings.liveries = !settings.liveries),
    d: () => (settings.daylight = !settings.daylight),
    w: () => (settings.weather = !settings.weather),
  };

  function onkeydown(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    const typing = t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement || !!t?.isContentEditable;
    if (typing || e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === "Escape") {
      if (helpOpen) helpOpen = false;
      else if (selectedId !== null) select(null);
      else if (boardCode) boardCode = null;
      else if (filtersOpen) filtersOpen = false;
      return;
    }
    if (helpOpen && e.key !== "?") return;
    const action = toggles[e.key.length === 1 ? e.key.toLowerCase() : e.key];
    if (!action) return;
    e.preventDefault();
    action();
  }

  const age = $derived(lastUpdate ? Math.max(0, Math.round((now - lastUpdate) / 1000)) : null);
  const attribution = $derived(
    [
      `Flight data: Flightradar24 (unofficial API${authenticated ? ", signed in" : ""})`,
      // DWD's open data licence asks to be named wherever its radar is shown
      settings.weather && "Radar: NOAA MRMS, Deutscher Wetterdienst, RainViewer",
      settings.basemap === "satellite" && "Imagery: Esri, Maxar, Earthstar Geographics",
      merged.added > 0 && "Track: OpenSky Network",
      "Map, names and buildings: OpenFreeMap, © OpenMapTiles, © OpenStreetMap contributors",
    ]
      .filter(Boolean)
      .join(" · "),
  );
  const partial = $derived(snapshot?.errors.length ?? 0);
  const retryIn = $derived(Math.max(0, Math.ceil((nextRefreshAt - now) / 1000)));
  const count = (n: number) => n.toLocaleString();

  let timers: ReturnType<typeof setInterval>[] = [];
  onMount(() => {
    if (!isTauri()) {
      feedError = "RustRadar needs its Rust backend. Start it with `bun tauri dev`.";
      return;
    }
    unfollowFlight(); // drop a stream left over from before a reload
    reference.load();
    sessionInfo()
      .then((s) => (session = s))
      .catch(() => {});
    refresh();
    refreshTop();
    timers = [setInterval(refreshTop, TOP_REFRESH_MS), setInterval(() => (now = Date.now()), 1000)];
  });
  onDestroy(() => {
    timers.forEach(clearInterval);
    clearTimeout(refreshTimer);
    clearTimeout(viewTimer);
  });
</script>

<svelte:window {onkeydown} />

<main>
  <Globe
    bind:this={globe}
    flights={shown}
    {selected}
    details={shownDetails}
    {route}
    airport={boardAirport ?? null}
    exaggeration={settings.exaggeration}
    basemap={settings.basemap}
    labels={settings.labels}
    buildings={settings.buildings}
    liveries={settings.liveries}
    daylight={settings.daylight}
    weather={settings.weather}
    follow={following}
    onselect={(f) => select(f?.id ?? null)}
    onairport={openAirport}
    onviewchange={(v) => (view = v)}
  />

  <div class="sidebar">
    <section class="surface primary">
      <header class="brand">
        <h1><Radar size={20} strokeWidth={1.75} /> RustRadar</h1>
        {#if snapshot}
          <p class="status" title="{snapshot.requests} requests in {snapshot.elapsedMs} ms">
            {#if loading}Updating…{:else}Updated {age} s ago{/if}
          </p>
        {:else if !feedError}
          <p class="status"><span class="skeleton" style="width: 90px; height: 12px"></span></p>
        {/if}
      </header>

      <SearchBox bind:this={searchBox} onpick={pickSearch} />

      <div class="toolbar">
        <button
          class="button filter-toggle"
          aria-pressed={filtersOpen}
          aria-expanded={filtersOpen}
          title="Filters (F)"
          onclick={() => (filtersOpen = !filtersOpen)}
        >
          <Funnel size={14} strokeWidth={1.75} />
          Filters
          {#if active}<span class="active-count" aria-label="{active} active">{active}</span>{/if}
        </button>
        {#if snapshot}
          <p class="counts">
            <span class="count">{count(filtered.length)}</span>
            {#if filtered.length !== flights.length}of {count(flights.length)}{/if}
            {regional ? "in view" : "worldwide"}
          </p>
        {/if}
      </div>

      {#if rateLimited}
        <div class="alert" role="status">
          <TriangleAlert size={16} strokeWidth={1.75} />
          <div>
            <p>Flightradar24 is limiting requests. Slowing down; next update in {retryIn} s.</p>
          </div>
        </div>
      {:else if feedError}
        <div class="alert" role="alert">
          <TriangleAlert size={16} strokeWidth={1.75} />
          <div>
            {#if isTauri()}
              <p>Can't reach Flightradar24. Trying again in {retryIn} s.</p>
            {:else}
              <p>No flight data in a plain browser.</p>
            {/if}
            <p class="detail">{feedError}</p>
            {#if isTauri()}
              <button class="button" onclick={refresh} disabled={loading}>
                <RefreshCw size={14} strokeWidth={1.75} /> Retry now
              </button>
            {/if}
          </div>
        </div>
      {:else if partial}
        <p class="note">{partial} region{partial === 1 ? "" : "s"} didn't load this round; some aircraft may be missing.</p>
      {/if}
    </section>

    {#if filtersOpen}
      <FilterPanel bind:filters {flights} shown={filtered.length} onclose={() => (filtersOpen = false)} />
    {/if}

    <section class="surface list">
      <TopFlights
        flights={top}
        failed={topFailed}
        {selectedId}
        onpick={(f) => select(f.flight_id, { fly: true })}
        onretry={refreshTop}
      />
    </section>
  </div>

  {#if boardCode}
    <!-- kept while a flight from the board is open, so going back keeps its tab and pages -->
    <div hidden={selected !== null}>
      <AirportPanel
        code={boardCode}
        airport={boardAirport}
        {selectedId}
        onselect={(id) => select(id, { fly: true, fromBoard: true })}
        onlocate={() => boardAirport && globe.flyTo(boardAirport.lon, boardAirport.lat, 11)}
        onclose={() => (boardCode = null)}
      />
    </div>
  {/if}

  {#if selected}
    <FlightPanel
      flight={selected}
      details={shownDetails}
      openskyAdded={merged.added}
      {route}
      status={followState}
      {authenticated}
      {following}
      back={fromBoard && boardCode ? { label: `${boardCode} departures and arrivals`, onclick: () => select(null) } : null}
      onfollow={() => (following = !following)}
      onselect={(id) => select(id, { fly: true })}
      onairport={openAirport}
      onsignin={() => controls.openAccount()}
      onclose={() => select(null)}
    />
  {/if}

  <MapControls
    bind:this={controls}
    zoom={view.zoom}
    bearing={view.bearing}
    pitch={view.pitch}
    bind:basemap={settings.basemap}
    bind:labels={settings.labels}
    bind:buildings={settings.buildings}
    bind:liveries={settings.liveries}
    bind:daylight={settings.daylight}
    bind:weather={settings.weather}
    bind:exaggeration={settings.exaggeration}
    {session}
    inset={selected || boardCode ? PANEL_INSET : 0}
    onzoom={(d) => globe.zoomBy(d)}
    onreset={() => globe.resetOrientation()}
    ontilt={(p) => globe.setPitch(p)}
    onsignin={handleSignIn}
    onsignout={handleSignOut}
    onhelp={() => (helpOpen = true)}
  />

  <ShortcutsHelp open={helpOpen} onclose={() => (helpOpen = false)} />

  <p class="attribution">{attribution}</p>
</main>

<style>
  main {
    position: fixed;
    inset: 0;
    background: radial-gradient(ellipse at 50% 45%, oklch(0.2 0.03 255) 0%, var(--ink) 60%);
  }
  .sidebar {
    position: absolute;
    top: var(--space-4);
    left: var(--space-4);
    bottom: 40px;
    width: 320px;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    pointer-events: none;
  }
  .sidebar > :global(section) {
    pointer-events: auto;
  }
  .primary {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-4);
  }
  /* with filters open, the list gives way first */
  .sidebar > :global(.filters) {
    flex-shrink: 1;
  }
  .list {
    flex: 0 3 auto;
    min-height: 96px;
    overflow-y: auto;
    padding: var(--space-3) var(--space-2);
  }
  .brand {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }
  h1 {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-lg);
    font-weight: 750;
    letter-spacing: -0.01em;
  }
  h1 :global(svg) {
    color: var(--amber);
  }
  .status {
    font-size: var(--text-sm);
    font-variant-numeric: tabular-nums;
    color: var(--text-3);
    white-space: nowrap;
  }
  .toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }
  .filter-toggle[aria-pressed="true"] {
    border-color: var(--amber);
    background: var(--amber-soft);
    color: var(--amber);
  }
  .active-count {
    display: inline-grid;
    place-items: center;
    min-width: 18px;
    height: 18px;
    padding: 0 4px;
    border-radius: 9px;
    background: var(--amber);
    font-size: var(--text-xs);
    font-weight: 700;
    color: var(--amber-ink);
  }
  .counts {
    font-size: var(--text-sm);
    font-variant-numeric: tabular-nums;
    color: var(--text-2);
    white-space: nowrap;
  }
  .count {
    font-size: var(--text-md);
    font-weight: 650;
    color: var(--text-1);
  }
  .alert {
    display: flex;
    gap: var(--space-2);
    padding: var(--space-3);
    border-radius: var(--radius-sm);
    background: var(--danger-soft);
    color: var(--danger);
    font-size: var(--text-md);
  }
  .alert :global(svg) {
    flex: none;
    margin-top: 1px;
  }
  .alert > div {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-2);
  }
  .alert p {
    color: var(--text-1);
  }
  .alert .detail {
    font-size: var(--text-sm);
    color: var(--text-2);
    word-break: break-word;
  }
  .note {
    font-size: var(--text-sm);
    color: var(--text-2);
  }
  .attribution {
    position: absolute;
    right: 0;
    bottom: 0;
    max-width: calc(100% - 352px);
    padding: 3px var(--space-2);
    border-top-left-radius: var(--radius-sm);
    background: oklch(0.15 0.02 255 / 0.85);
    font-size: var(--text-xs);
    color: var(--text-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
