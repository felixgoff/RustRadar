<script lang="ts">
  import { onDestroy, onMount, untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { Funnel, Radar, RefreshCw, Ship as ShipIcon, TrainFront, TriangleAlert } from "@lucide/svelte";
  import {
    adsbCallsign,
    adsbHex,
    followFlight,
    isRateLimited,
    isTauri,
    liveFlights,
    openskyTrack,
    sessionInfo,
    signIn,
    shipDetails,
    shipsInView,
    signOut,
    topFlights,
    trainsInView,
    unfollowFlight,
    type AdsbAircraft,
    type BoundingBox,
    type FindEntry,
    type FlightDetails,
    type FollowEvent,
    type LiveFlight,
    type LiveSnapshot,
    type OpenSkyPoint,
    type Place,
    type SessionInfo,
    type Ship,
    type ShipDetails,
    type TopFlight,
    type Train,
    type TrainStop,
    type TrainRoute,
    trainRoute,
    watchTrains,
    hideTrainOperators,
    followTrain,
    unfollowTrain,
    trafikverketKeyStatus,
  } from "$lib/api";
  import AirportPanel from "$lib/AirportPanel.svelte";
  import FilterPanel from "$lib/FilterPanel.svelte";
  import FlightPanel, { type FollowState } from "$lib/FlightPanel.svelte";
  import Globe, { type ViewState } from "$lib/Globe.svelte";
  import MapControls from "$lib/MapControls.svelte";
  import ModeBar from "$lib/ModeBar.svelte";
  import BottomSheet, { type Detent } from "$lib/BottomSheet.svelte";
  import { layout } from "$lib/phone.svelte";
  import { pushState } from "$app/navigation";
  import { page } from "$app/state";
  import SearchBox, { type LocalSearch } from "$lib/SearchBox.svelte";
  import ShipFilterPanel from "$lib/ShipFilterPanel.svelte";
  import ShipList from "$lib/ShipList.svelte";
  import ShipPanel from "$lib/ShipPanel.svelte";
  import ShortcutsHelp from "$lib/ShortcutsHelp.svelte";
  import KeysDialog from "$lib/KeysDialog.svelte";
  import DevPanel from "$lib/DevPanel.svelte";
  import TrainFilterPanel from "$lib/TrainFilterPanel.svelte";
  import TrainList from "$lib/TrainList.svelte";
  import TrainPanel from "$lib/TrainPanel.svelte";
  import TopFlights from "$lib/TopFlights.svelte";
  import {
    activeCount,
    loadPersisted,
    matcher,
    persist,
    serverCategories,
    shownKinds,
    type Filters,
    type Mode,
  } from "$lib/filters";
  import { DEFAULT_TRAIN_FILTERS, sortTrains, trainFilter, trainFilterCount, trainMatches } from "$lib/train-list";
  import { DEFAULT_SHIP_FILTERS, shipFilter, shipFilterCount, shipMatches, shipName, sortShips } from "$lib/ship-list";
  import { shipColor, shipKindLabel } from "$lib/ships";
  import { cosAngle, viewArea, visibleRadius } from "$lib/geo";
  import { motion } from "$lib/motion";
  import { matchAdsb } from "$lib/adsb";
  import { reference } from "$lib/reference.svelte";
  import { mergeTrail, routeFor } from "$lib/routes";
  import { placeView } from "$lib/places";
  import { afterRound, feedFreshness, type FeedClock } from "$lib/freshness";
  import { clockAt } from "$lib/time";
  import { delayText, trainColorOf, trainCredits, trainLabel } from "$lib/trains";
  import { RAIL_SPEED_ATTRIBUTION } from "$lib/rail-layer";
  import { SGL_ATTRIBUTION, SGL_ZOOM } from "$lib/streets-gl";
  import { LANDCOVER_ATTRIBUTION } from "$lib/landcover";
  import { CANOPY_ATTRIBUTION } from "$lib/canopy";
  import { STREET_TREES_ATTRIBUTION } from "$lib/street-trees";
  import { OVERTURE_ATTRIBUTION } from "$lib/overture";
  import { IMAGERY_ATTRIBUTION } from "$lib/imagery";
  import { TERRAIN_ATTRIBUTION } from "$lib/terrain-layer";
  import { conditionsOf, fetchWeather, PRESETS, type Weather } from "$lib/weather";
  import { distanceKm as distanceKmBetween } from "$lib/flightpath";
  import { openUrl } from "@tauri-apps/plugin-opener";

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
  /** One per mode: the one on show. */
  let searchBox = $state<SearchBox>();
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
  /** When the follow stream gave up: the newest report it had (ms, feed clock). Frozen, so "Last seen" stays put. */
  let endedSeenMs = $state(0);
  /** The camera was locked on the aircraft when its stream gave up: lock on again if it resumes. */
  let relockOnResume = false;
  let feedError = $state<string | null>(null);
  let rateLimited = $state(false);
  let loading = $state(false);
  let nextRefreshAt = $state(0);
  /** When the snapshot on show arrived, and the schedule it was fetched on: what its age is measured against. */
  let shownFeed = $state.raw<FeedClock | null>(null);
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
  let trainFilters = $state(persisted.trainFilters);
  let shipFilters = $state(persisted.shipFilters);
  /** What is on the map, so fetched and drawn: the focus, and any kinds shown with it. */
  const kinds = $derived(shownKinds(settings));
  const aircraftOn = $derived(kinds.aircraft);
  const trainsOn = $derived(kinds.trains);
  const shipsOn = $derived(kinds.ships);
  /** The kind the sidebar is for: its search, filters and list. */
  const focus = $derived(settings.mode);

  /** Shift+P, I or S, or the mode bar's eye: another kind on or off the map. The focus is always on. */
  // The trains are Sweden's, from Trafikverket: offered once its key is saved
  // and Trafikverket has accepted it (asked at the start and when the keys
  // page closes; while that's unknown, a trains mode already chosen stays).
  let trafikverketKey = $state<"none" | "invalid" | "ok" | null>(null);
  let keyRetry = 0;
  function checkTrafikverketKey() {
    clearTimeout(keyRetry);
    if (!isTauri()) {
      trafikverketKey = "none";
      return;
    }
    trafikverketKeyStatus().then(
      (status) => (trafikverketKey = status),
      // Trafikverket out of reach: the key may be fine, ask again shortly
      () => (keyRetry = window.setTimeout(checkTrafikverketKey, 30_000)),
    );
  }
  const trainsOffered = $derived(trafikverketKey === "ok");
  $effect(() => {
    if (trafikverketKey !== "none" && trafikverketKey !== "invalid") return;
    untrack(() => {
      if (settings.mode === "trains") settings.mode = "aircraft";
      if (settings.alsoShow.includes("trains")) settings.alsoShow = settings.alsoShow.filter((k) => k !== "trains");
    });
  });

  function toggleShown(kind: Mode) {
    if (kind === settings.mode) return;
    const list = settings.alsoShow;
    settings.alsoShow = list.includes(kind) ? list.filter((k) => k !== kind) : [...list, kind];
  }
  let filtersOpen = $state(false);
  let helpOpen = $state(false);
  let keysOpen = $state(false);
  /** The map controls' open menu. */
  let menu = $state<"layers" | "settings" | null>(null);
  /** On a phone: how far up the sheet is, and its height in px. */
  let sheetDetent = $state<Detent>("peek");
  let sheetHeight = $state(0);
  /** The keys page opened from a "sign in" prompt: the caret goes to the login, and back to the prompt after. */
  let keysPrompt = $state<(() => HTMLElement | null) | null>(null);
  let session = $state.raw<SessionInfo | null>(null);
  /** The camera follows the selected aircraft. */
  let following = $state(false);
  /** The camera follows the selected train or ship. */
  let followingVehicle = $state(false);

  // The realistic map's weather, live: where the map is looking, asked for
  // again every ten minutes or after a move of 15 km; after a failure, not
  // for another two minutes.
  const WEATHER_REFRESH_MS = 10 * 60_000;
  const WEATHER_RETRY_MS = 2 * 60_000;
  const WEATHER_MOVE_KM = 15;
  let liveWeather = $state.raw<Weather | null>(null);
  let weatherAsked: { lat: number; lon: number; at: number; ok: boolean } | null = null;
  let weatherBusy = false;
  async function refreshWeather(lat: number, lon: number) {
    const last = weatherAsked;
    const age = last ? Date.now() - last.at : Infinity;
    const moved = last ? distanceKmBetween([last.lon, last.lat], [lon, lat]) : Infinity;
    if (weatherBusy || (last && age < (last.ok ? WEATHER_REFRESH_MS : WEATHER_RETRY_MS) && (moved < WEATHER_MOVE_KM || !last.ok))) return;
    weatherBusy = true;
    try {
      liveWeather = await fetchWeather(lat, lon);
      weatherAsked = { lat, lon, at: Date.now(), ok: true };
    } catch {
      weatherAsked = { lat, lon, at: Date.now(), ok: false };
    } finally {
      weatherBusy = false;
    }
  }
  const weatherLive = $derived(settings.basemap === "realistic" && settings.sky === "live");
  $effect(() => {
    if (!weatherLive) return;
    const { latitude, longitude } = view;
    untrack(() => void refreshWeather(latitude, longitude));
  });
  $effect(() => {
    if (!weatherLive) return;
    const timer = setInterval(() => untrack(() => void refreshWeather(view.latitude, view.longitude)), 60_000);
    return () => clearInterval(timer);
  });
  /** How the weather lights the realistic map: the live weather's, or the one chosen (until the live one comes, none). */
  const skyConditions = $derived(settings.sky === "live" ? (liveWeather ? conditionsOf(liveWeather) : null) : PRESETS[settings.sky]);
  /** IATA code of the airport whose board is open. */
  let boardCode = $state<string | null>(null);
  /** The open flight was picked from that board. */
  let fromBoard = $state(false);
  /** Fly to the selected flight once its position is known. */
  let flyPending = false;
  /** Zoom for that pending fly-to; undefined lets `flyTo` choose one. */
  let flyZoom: number | undefined;
  /** A place picked in the search, marked on the globe until dismissed. */
  let place = $state.raw<Place | null>(null);
  /** Places rank toward the view's centre once it is zoomed in past the whole planet. */
  const near = $derived(view.zoom >= 5 ? { lat: view.latitude, lon: view.longitude } : null);

  $effect(() =>
    persist({
      settings: $state.snapshot(settings),
      filters: $state.snapshot(filters) as Filters,
      trainFilters: $state.snapshot(trainFilters),
      shipFilters: $state.snapshot(shipFilters),
    }),
  );

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
    // other traffic shows which runways are in use
    return selected ? routeFor(selected, shownDetails, flights) : null;
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
  // adsb.lol: what the selected aircraft's own transponder reports (heading,
  // bank, rates, air data, autopilot targets). Asked for that aircraft only,
  // every few seconds while the window is visible, slower after errors; the
  // backend adds its own spacing, cache and backoff.
  const ADSB_POLL_MS = 3_000;
  const ADSB_MAX_BACKOFF = 16;
  /** Measured data is shown for this long after the aircraft last reported. */
  const ADSB_SHOW_MS = 60_000;
  let adsb = $state.raw<{ id: number; aircraft: AdsbAircraft; seenAt: number } | null>(null);
  const selectedCallsign = $derived(selected?.callsign ?? "");
  const adsbQuery = $derived(
    settings.adsb && selectedId !== null && (icao24 || selectedCallsign)
      ? `${selectedId}|${icao24 ?? `cs:${selectedCallsign}`}`
      : null,
  );
  $effect(() => {
    if (!adsbQuery || !isTauri()) return;
    const id = selectedId!;
    const hex = icao24;
    const callsign = selectedCallsign;
    let stopped = false;
    let failures = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      if (stopped) return;
      if (document.visibilityState !== "hidden") {
        try {
          const snap = hex ? await adsbHex(hex) : await adsbCallsign(callsign);
          if (stopped) return;
          // without the address, the callsign match is checked against where the flight is
          const f = untrack(() => selected);
          const match = matchAdsb(snap.aircraft, { hex, callsign, reg: f?.reg ?? "", lat: f?.lat ?? NaN, lon: f?.lon ?? NaN });
          if (match) {
            // its `now` is when adsb.lol answered: the age of the data on our clock
            adsb = { id, aircraft: match, seenAt: Date.now() - Math.max(0, snap.nowMs - match.seenMs) };
            motion.measure(id, match);
          }
          failures = 0;
        } catch {
          failures++; // unreachable or resting: Flightradar24's data stands alone meanwhile
        }
      }
      if (!stopped) timer = setTimeout(poll, ADSB_POLL_MS * Math.min(2 ** failures, ADSB_MAX_BACKOFF));
    };
    poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  });
  // a deselected aircraft (or the source switched off) goes back to the estimates, eased
  let measuredId: number | null = null;
  $effect(() => {
    const want = settings.adsb ? selectedId : null;
    if (measuredId !== null && measuredId !== want) motion.measure(measuredId, null);
    measuredId = want;
    if (untrack(() => adsb)?.id !== want) adsb = null;
  });
  const measured = $derived(
    adsb && adsb.id === selectedId && settings.adsb && now - adsb.seenAt < ADSB_SHOW_MS ? adsb.aircraft : null,
  );

  // Ships from VesselFinder: the area in view at the zoom in view, once the
  // camera settles and then every minute while the window is visible. The
  // backend runs one request at a time and answers an area it fetched in the
  // last 10 s from memory; an answer for a view since left behind is dropped.
  const SHIPS_SETTLE_MS = 600;
  const SHIPS_REFRESH_MS = 60_000;
  let ships = $state.raw<Ship[]>([]);
  /** The first answer since the ships came on has arrived; when the latest one did (ms). */
  let shipsAt = $state(0);
  let shipsFailed = $state(false);
  let shipsRequest = 0;
  /** Waits for the camera to settle; reset by every move. */
  let shipsSettle: ReturnType<typeof setTimeout> | undefined;
  /** The next refresh; kept apart, so a camera that never settles (following an aircraft) still gets one. */
  let shipsRefresh: ReturnType<typeof setTimeout> | undefined;
  /** The feed's zoom steps: an answer at another one has the wrong density. */
  // the feed's zoom step (see ships.rs `feed_zoom`): 2..16, as it is empty from 17 up
  const shipZoom = (zoom: number) => Math.min(16, Math.max(2, Math.floor(zoom + 1)));
  function stopShips() {
    clearTimeout(shipsSettle);
    clearTimeout(shipsRefresh);
  }
  async function loadShips() {
    stopShips();
    if (!shipsOn || !isTauri()) return;
    shipsRefresh = setTimeout(loadShips, SHIPS_REFRESH_MS);
    if (document.visibilityState === "hidden") return;
    const asked = { ...view };
    const area = viewArea(asked, window.innerWidth, window.innerHeight) ?? { west: -180, south: -90, east: 180, north: 90 };
    const request = ++shipsRequest;
    try {
      const list = await shipsInView(area, asked.zoom);
      // dropped if newer was asked meanwhile, or the camera has gone elsewhere
      const v = view;
      const radius = visibleRadius(v.zoom, v.pitch, window.innerWidth, window.innerHeight);
      const away = cosAngle(v.longitude, v.latitude, asked.longitude, asked.latitude) < Math.cos((radius * Math.PI) / 360);
      if (request !== shipsRequest || !shipsOn || shipZoom(v.zoom) !== shipZoom(asked.zoom) || away) return;
      ships = list;
      shipsAt = Date.now();
      shipsFailed = false;
    } catch {
      // resting after a failure, or unreachable: the ships on show stay
      if (request === shipsRequest) shipsFailed = true;
    }
  }
  $effect(() => {
    void view;
    if (!shipsOn) {
      stopShips();
      shipsRequest++;
      // only when there is something to clear: a fresh empty list on every view change would
      // redraw the map, which moves the view again while it follows an aircraft
      untrack(() => {
        if (ships.length) ships = [];
        if (shipsAt) shipsAt = 0;
        if (shipsFailed) shipsFailed = false;
      });
      return;
    }
    clearTimeout(shipsSettle);
    shipsSettle = setTimeout(loadShips, SHIPS_SETTLE_MS);
  });

  // The selected ship: its card from VesselFinder, asked for again every
  // minute while open (the backend keeps each card a minute and shares the
  // map feed's spacing and backoff). Its position comes from the ship list as
  // the map refreshes, and stays where it was last seen if the list drops it.
  const SHIP_DETAILS_REFRESH_MS = 60_000;
  let pickedShip = $state.raw<Ship | null>(null);
  let shipCard = $state.raw<{ mmsi: number; details: ShipDetails | null; error: string | null } | null>(null);
  let shipCardRequest = 0;
  const selectedShip = $derived.by<Ship | null>(() => {
    if (!pickedShip) return null;
    const mmsi = pickedShip.mmsi;
    return ships.find((s) => s.mmsi === mmsi) ?? pickedShip;
  });
  const selectedMmsi = $derived(pickedShip?.mmsi ?? null);
  // keep the last position on show once the list drops the ship
  $effect(() => {
    const live = selectedShip;
    if (live && live !== untrack(() => pickedShip)) pickedShip = live;
  });

  async function loadShipCard(mmsi: number) {
    const request = ++shipCardRequest;
    // a refresh keeps the card on show; only a new ship (or a retry after a failure) shows the skeletons
    if (untrack(() => shipCard)?.mmsi !== mmsi || untrack(() => shipCard)?.error) shipCard = { mmsi, details: null, error: null };
    try {
      const details = await shipDetails(mmsi);
      if (request === shipCardRequest) shipCard = { mmsi, details, error: null };
    } catch (e) {
      // a failed refresh keeps the card it had
      const had = untrack(() => shipCard);
      if (request === shipCardRequest && !(had?.mmsi === mmsi && had.details)) shipCard = { mmsi, details: null, error: String(e) };
    }
  }
  $effect(() => {
    const mmsi = selectedMmsi;
    if (mmsi === null || !isTauri()) return;
    untrack(() => loadShipCard(mmsi));
    const timer = setInterval(() => document.visibilityState !== "hidden" && loadShipCard(mmsi), SHIP_DETAILS_REFRESH_MS);
    return () => clearInterval(timer);
  });
  // the ships off the map: nothing left to show
  $effect(() => {
    if (!shipsOn) untrack(() => selectShip(null));
  });

  /** Opens a ship's card; any flight or airport board closes. */
  function selectShip(ship: Ship | null) {
    if (ship?.mmsi === pickedShip?.mmsi && ship) return;
    pickedShip = ship;
    shipCardRequest++;
    shipCard = null;
    if (ship) {
      // one panel at a time; the focus (and the sidebar) stays where it is
      if (selectedId !== null) select(null);
      selectTrain(null);
      boardCode = null;
    }
  }

  // Trains from regional public feeds (trains.rs): the backend keeps each
  // feed whole and answers for the area in view, so the view is asked for
  // once the camera settles and then every 30 s while the window is visible.
  // An answer for a view since left behind is dropped.
  const TRAINS_SETTLE_MS = 600;
  const TRAINS_REFRESH_MS = 10_000;
  /** Zoomed in this far, the trains in view come live, every few seconds, like a followed one. */
  const LIVE_ZOOM = 9;
  /** At most this many of them, nearest the centre first. */
  const MAX_LIVE = 40;
  /** The ids last sent, to send again only when they change. */
  let watchedKey = "";
  function watchInView(list: Train[], v: ViewState) {
    if (!isTauri()) return;
    const close = trainsOn && v.zoom >= LIVE_ZOOM;
    const ids = close
      ? list
          .filter((t) => t.source === "trafikverket")
          .map((t) => ({ id: t.id, d: cosAngle(v.longitude, v.latitude, t.lon, t.lat) }))
          .sort((a, b) => b.d - a.d)
          .slice(0, MAX_LIVE)
          .map((t) => t.id)
          .sort()
      : [];
    const key = ids.join(",");
    if (key === watchedKey) return;
    watchedKey = key;
    watchTrains(ids).catch(() => {});
  }
  let trains = $state.raw<Train[]>([]);
  /** When the latest answer since the trains came on arrived (ms); 0 before the first. */
  let trainsAt = $state(0);
  let trainsFailed = $state(false);
  let trainsRequest = 0;
  let trainsSettle: ReturnType<typeof setTimeout> | undefined;
  let trainsRefresh: ReturnType<typeof setTimeout> | undefined;
  function stopTrains() {
    clearTimeout(trainsSettle);
    clearTimeout(trainsRefresh);
  }
  async function loadTrains() {
    stopTrains();
    // dev builds in a plain browser, `#demo` in the address: made-up trains in the middle of the view
    if (import.meta.env.DEV && !isTauri() && trainsOn && location.hash.includes("demo")) {
      const { demoTrains } = await import("$lib/demo-trains");
      trains = demoTrains(view, Date.now());
      return;
    }
    if (!trainsOn || !isTauri()) return;
    trainsRefresh = setTimeout(loadTrains, TRAINS_REFRESH_MS);
    if (document.visibilityState === "hidden") return;
    const asked = { ...view };
    const area = viewArea(asked, window.innerWidth, window.innerHeight) ?? { west: -180, south: -90, east: 180, north: 90 };
    const request = ++trainsRequest;
    try {
      const list = await trainsInView(area, asked.zoom);
      const v = view;
      const radius = visibleRadius(v.zoom, v.pitch, window.innerWidth, window.innerHeight);
      const away = cosAngle(v.longitude, v.latitude, asked.longitude, asked.latitude) < Math.cos((radius * Math.PI) / 360);
      if (request !== trainsRequest || !trainsOn || away) return;
      trains = list;
      trainsAt = Date.now();
      trainsFailed = false;
      watchInView(list, v);
    } catch {
      // every source failed or is resting: the trains on show stay
      if (request === trainsRequest) trainsFailed = true;
    }
  }
  // Trafikverket's trains come live, a stream for each operator: the backend
  // moves them as reports come and says so; asked again for the view, at
  // most this often
  const TRAINS_LIVE_MS = 1_500;
  let trainsLiveAt = 0;
  let trainsLivePending: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    if (!isTauri()) return;
    let stop: (() => void) | undefined;
    let gone = false;
    listen("trains-changed", () => {
      if (!trainsOn || trainsLivePending) return;
      const wait = Math.max(0, trainsLiveAt + TRAINS_LIVE_MS - Date.now());
      trainsLivePending = setTimeout(() => {
        trainsLivePending = undefined;
        trainsLiveAt = Date.now();
        loadTrains();
      }, wait);
    }).then((un) => (gone ? un() : (stop = un)));
    return () => {
      gone = true;
      stop?.();
      clearTimeout(trainsLivePending);
      trainsLivePending = undefined;
    };
  });
  $effect(() => {
    void view;
    if (!trainsOn) {
      stopTrains();
      trainsRequest++;
      // as for the ships: nothing written when already clear
      untrack(() => {
        if (trains.length) trains = [];
        if (trainsAt) trainsAt = 0;
        if (trainsFailed) trainsFailed = false;
        // no more live trains in view
        watchInView([], view);
      });
      return;
    }
    clearTimeout(trainsSettle);
    trainsSettle = setTimeout(loadTrains, TRAINS_SETTLE_MS);
  });

  // The selected train: its position and report follow the list as the map
  // refreshes, and stay where they were last seen if the list drops it.
  let pickedTrain = $state.raw<Train | null>(null);
  /** The selected train as its live stream last reported it, and its stops (Trafikverket's trains). */
  let liveTrain = $state.raw<Train | null>(null);
  let liveStops = $state.raw<TrainStop[] | null>(null);
  const selectedTrain = $derived.by<Train | null>(() => {
    if (!pickedTrain) return null;
    const id = pickedTrain.id;
    const listed = trains.find((t) => t.id === id) ?? pickedTrain;
    // the stream's report is newer than the list's more often than not
    return liveTrain?.id === id && liveTrain.positionUnix >= listed.positionUnix ? liveTrain : listed;
  });
  // follow the selected train live while its panel is open
  const followedId = $derived(pickedTrain?.id ?? null);
  // another train or ship (or none): the camera lets go (not on a refresh of the same one)
  const followableKey = $derived(`${followedId ?? ""}|${selectedShip?.mmsi ?? ""}`);
  $effect(() => {
    void followableKey;
    untrack(() => (followingVehicle = false));
  });
  $effect(() => {
    const id = followedId;
    liveTrain = null;
    liveStops = null;
    if (!id || !isTauri()) return;
    followTrain(id, (e) => {
      if (e.kind === "train" && e.train.id === id) liveTrain = e.train;
      else if (e.kind === "stops") liveStops = e.stops;
    }).catch(() => {});
    return () => void unfollowTrain().catch(() => {});
  });
  /** The selected train's whole way along the track, as the backend works it out. */
  let shownRoute = $state.raw<TrainRoute | null>(null);
  /** Asked again this soon while the track is still loading, else this often (the train moves along it). */
  const ROUTE_LOADING_MS = 2_500;
  const ROUTE_REFRESH_MS = 30_000;
  /** A train drawn off its route has it asked for again, at most this often. */
  const ROUTE_OFF_MS = 3_000;
  /** Asks for the selected train's route again now (the train isn't on the one shown); set while one is followed. */
  let reloadRoute: (() => void) | null = null;
  $effect(() => {
    const id = followedId;
    shownRoute = null;
    if (!id || !id.startsWith("trafikverket:") || !isTauri()) return;
    let live = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let asking = false;
    let askedAt = 0;
    const load = () => {
      clearTimeout(timer);
      asking = true;
      askedAt = Date.now();
      trainRoute(id).then(
        (r) => {
          asking = false;
          if (!live) return;
          shownRoute = r;
          timer = setTimeout(load, r.complete ? ROUTE_REFRESH_MS : ROUTE_LOADING_MS);
        },
        // no timetable yet (it is learned with the next refresh): ask again
        () => {
          asking = false;
          if (live) timer = setTimeout(load, ROUTE_REFRESH_MS / 3);
        },
      );
    };
    reloadRoute = () => {
      if (live && !asking && Date.now() - askedAt >= ROUTE_OFF_MS) load();
    };
    load();
    return () => {
      live = false;
      reloadRoute = null;
      clearTimeout(timer);
    };
  });
  $effect(() => {
    const live = selectedTrain;
    if (live && live !== untrack(() => pickedTrain)) pickedTrain = live;
  });
  $effect(() => {
    if (!trainsOn) untrack(() => selectTrain(null));
  });

  /** Opens a train's panel; any flight, ship or airport board closes. */
  function selectTrain(train: Train | null) {
    if (train && train.id === pickedTrain?.id) return;
    pickedTrain = train;
    if (train) {
      // one panel at a time; the focus (and the sidebar) stays where it is
      if (selectedId !== null) select(null);
      selectShip(null);
      boardCode = null;
    }
  }

  // The trains and ships sidebars. The filters narrow what the map draws as
  // well as the list; the search narrows the list only, as in aircraft mode.
  // The selection stays drawn when the filters would leave it out.
  let trainQuery = $state("");
  let shipQuery = $state("");
  let hoveredTrainId = $state<string | null>(null);
  let hoveredShipMmsi = $state<number | null>(null);
  /** The map's centre for the distance sorts, in steps, so a moving camera re-sorts now and then rather than every frame. */
  const centreLat = $derived(Math.round(view.latitude * 20) / 20);
  const centreLon = $derived(Math.round(view.longitude * 20) / 20);
  const listCentre = $derived({ lat: centreLat, lon: centreLon });
  const ORIGIN = { lat: 0, lon: 0 };

  const trainsFiltered = $derived(trains.filter(trainFilter(trainFilters)));
  // the operators left out aren't streamed live either
  $effect(() => {
    const hidden = [...trainFilters.hiddenOperators];
    if (isTauri()) hideTrainOperators(hidden).catch(() => {});
  });
  const trainsListed = $derived.by(() => {
    const q = trainQuery.trim();
    const sort = trainFilters.sort;
    return sortTrains(q ? trainsFiltered.filter((t) => trainMatches(t, q)) : trainsFiltered, sort, sort === "distance" ? listCentre : ORIGIN);
  });
  const trainsOnMap = $derived.by(() => {
    if (!selectedTrain) return trainsFiltered;
    const at = trainsFiltered.findIndex((t) => t.id === selectedTrain.id);
    if (at === -1) return [...trainsFiltered, selectedTrain];
    // drawn from its live report rather than the list's
    if (trainsFiltered[at] === selectedTrain) return trainsFiltered;
    const list = trainsFiltered.slice();
    list[at] = selectedTrain;
    return list;
  });
  const shipsFiltered = $derived(ships.filter(shipFilter(shipFilters)));
  const shipsListed = $derived.by(() => {
    const q = shipQuery.trim();
    const sort = shipFilters.sort;
    return sortShips(q ? shipsFiltered.filter((s) => shipMatches(s, q)) : shipsFiltered, sort, sort === "distance" ? listCentre : ORIGIN);
  });
  const shipsOnMap = $derived(
    selectedShip && !shipsFiltered.some((s) => s.mmsi === selectedShip.mmsi) ? [...shipsFiltered, selectedShip] : shipsFiltered,
  );
  /** VesselFinder tells ship sizes apart above its zoom step 6 (ships.rs). */
  const shipSizesKnown = $derived(shipZoom(view.zoom) > 6);
  const trainFilterActive = $derived(trainFilterCount(trainFilters));
  const shipFilterActive = $derived(shipFilterCount(shipFilters));

  /** Picked from the list or the search: selected, and the camera flies there. */
  function pickTrain(train: Train) {
    place = null;
    following = false;
    selectTrain(train);
    globe.flyTo(train.lon, train.lat, Math.max(view.zoom, 10));
  }
  function pickShip(ship: Ship) {
    place = null;
    following = false;
    selectShip(ship);
    globe.flyTo(ship.lon, ship.lat, Math.max(view.zoom, 11));
  }
  const rgb = (c: readonly number[]) => `rgb(${c.join(", ")})`;
  const trainSearch: LocalSearch = {
    heading: "Trains in view",
    noun: "trains",
    icon: TrainFront,
    // the mode buttons share a phone's search row
    get placeholder() {
      return layout.phone ? "Train or address" : "Train, destination or address";
    },
    search: () =>
      trainsListed.slice(0, 6).map((t) => ({
        key: t.id,
        label: t.name,
        detail: [t.destination && `To ${t.destination}`, delayText(t.delayMinutes) || trainLabel(t.kind)].filter(Boolean).join(" · "),
        color: rgb(trainColorOf(t)),
      })),
    onpick: (id) => {
      const t = trains.find((t) => t.id === id);
      if (t) pickTrain(t);
    },
  };
  const shipSearch: LocalSearch = {
    heading: "Ships in view",
    noun: "ships",
    icon: ShipIcon,
    get placeholder() {
      return layout.phone ? "Ship or address" : "Ship name, MMSI or address";
    },
    search: () =>
      shipsListed.slice(0, 6).map((s) => ({
        key: String(s.mmsi),
        label: shipName(s) || `MMSI ${s.mmsi}`,
        detail: `${shipKindLabel(s.kind)} · MMSI ${s.mmsi}`,
        color: rgb(shipColor(s.kind)),
      })),
    onpick: (key) => {
      const s = ships.find((s) => String(s.mmsi) === key);
      if (s) pickShip(s);
    },
  };

  // the hover belongs to the list it came from
  $effect(() => {
    void focus;
    hoveredTrainId = null;
    hoveredShipMmsi = null;
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
    if (!isTauri() || backoff > 1 || !aircraftOn) return;
    if (loading) {
      refreshAgain = true;
      return;
    }
    scheduleRefresh(Math.max(300, lastRequestAt + MIN_GAP_MS - Date.now()));
  }

  async function refresh() {
    // the aircraft off the map: no live fetches at all, and none scheduled until it is back
    if (loading || !aircraftOn) return;
    loading = true;
    refreshAgain = false;
    const area = viewArea(view, window.innerWidth, window.innerHeight);
    fetchedArea = { area, view: { ...view } };
    lastRequestAt = Date.now();
    let limited = false;
    let arrived = false;
    try {
      const next = await liveFlights(area, serverCategories($state.snapshot(filters) as Filters));
      // the aircraft left the map while the request was out: drop what it brought
      if (!aircraftOn) return;
      snapshot = next;
      // aircraft are drawn on the feed's clock, so a wrong system clock can't misplace them
      motion.syncClock(snapshot.serverTimeMs);
      regional = area !== null;
      arrived = true;
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
      // only new data moves the clock: a failure's longer backoff can't make old data look fresh
      shownFeed = afterRound(shownFeed, { arrived, at: Date.now(), interval });
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

  // The aircraft off the map: the open flight and airport board close (the
  // follow stream and the camera's lock with them) and the feed stops. Back
  // in it, it fetches again at once, or as soon as the gap between requests
  // allows.
  let aircraftWere = untrack(() => aircraftOn);
  $effect(() => {
    const on = aircraftOn;
    if (on === aircraftWere) return;
    aircraftWere = on;
    untrack(() => {
      if (!on) {
        clearTimeout(refreshTimer);
        nextRefreshAt = 0;
        if (selectedId !== null) select(null);
        boardCode = null;
        // what comes back on is a fresh feed, not aircraft extrapolated from minutes ago
        snapshot = null;
        shownFeed = null;
      } else if (isTauri()) {
        if (!loading) scheduleRefresh(Math.max(0, lastRequestAt + MIN_GAP_MS - Date.now()));
      }
    });
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

  // the most tracked list is the aircraft sidebar's: fetched while the aircraft are the focus
  $effect(() => {
    if (focus === "aircraft" && isTauri()) untrack(refreshTop);
  });

  async function refreshTop() {
    try {
      top = await topFlights();
      topFailed = false;
    } catch {
      topFailed = true;
    }
  }

  function startFollow(id: number, resuming = false) {
    followState = resuming ? "reconnecting" : "connecting";
    followFlight(id, (e: FollowEvent) => {
      if (selectedId !== id) return; // a late event for a previous selection
      if (e.event === "details") {
        details = e.data;
        followState = "live";
        if (relockOnResume) following = true;
        relockOnResume = false;
        if (flyPending && e.data.flight) {
          flyPending = false;
          globe.flyTo(e.data.flight.longitude, e.data.flight.latitude, flyZoom);
        }
      } else if (e.event === "error") {
        followState = "reconnecting";
      } else {
        followEnded();
      }
    }).catch(() => selectedId === id && followEnded());
  }

  /** The open flight stopped reporting: its last values stay on show, and the camera lets go. */
  function followEnded() {
    followState = "ended";
    endedSeenMs = selected?.timestampMs ?? 0;
    // nothing moves any more to keep in frame; Follow (or C) can still lock on again
    relockOnResume ||= following;
    following = false;
  }

  /** The panel's Follow, or C: the viewer's own choice, which a resumed stream doesn't override. */
  function toggleFollow() {
    following = !following;
    relockOnResume = false;
  }

  // The stream can give up while the aircraft still reports (its endpoint
  // rate limited, say). A newer position in the feed shows it is still
  // flying: ask again, as reconnecting rather than ended. At most once per
  // feed refresh, since each end records the newest report seen.
  $effect(() => {
    if (followState !== "ended" || selectedId === null) return;
    const reported = byId.get(selectedId)?.timestampMs ?? 0;
    if (reported > endedSeenMs) untrack(() => startFollow(selectedId!, true));
  });

  function select(
    id: number | null,
    opts: { hint?: LiveFlight; fly?: boolean; follow?: boolean; fromBoard?: boolean } = {},
  ) {
    if (id === selectedId && !opts.fly) {
      // another click on a flight that stopped reporting asks again whether it is live
      if (id !== null && followState === "ended") startFollow(id);
      return;
    }
    selectedId = id;
    if (id !== null) {
      // one panel at a time; the focus (and the sidebar) stays where it is
      selectShip(null);
      selectTrain(null);
    }
    details = null;
    hint = opts.hint ?? null;
    flyPending = false;
    following = opts.follow ?? false;
    relockOnResume = false;
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
    selectShip(null);
    selectTrain(null);
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

  /** Fly to a place and mark it; nothing is selected or followed. */
  function pickPlace(p: Place) {
    // a camera locked on an aircraft can't fly anywhere else
    following = false;
    place = p;
    const target = placeView(p, window.innerWidth, window.innerHeight);
    globe.flyTo(target.lon, target.lat, target.zoom);
  }

  async function handleSignIn(email: string, password: string) {
    session = await signIn(email, password);
    // fetch again with the session: the feed, and the open flight's details (flight plan)
    refreshSoon();
    if (selectedId !== null) startFollow(selectedId);
  }

  /** From a sign-in prompt in a panel: the keyboard lands in the form, and goes back to the panel when it closes. */
  function promptSignIn(returnFocus: () => HTMLElement | null) {
    keysPrompt = returnFocus;
    keysOpen = true;
  }

  /** A flight, train, ship or airport board is open: on a phone it takes the sheet, and the search steps aside. */
  const detailOpen = $derived(Boolean(selected || boardCode || selectedShip || selectedTrain));
  // on a phone the details open lowered, to their summary (a board half up:
  // its flights are what it's for); the list comes back as high as it was
  let listDetent: Detent = "peek";
  $effect.pre(() => {
    if (!detailOpen) return;
    untrack(() => {
      listDetent = sheetDetent;
      sheetDetent = boardCode && !selected ? "half" : "peek";
    });
    return () => {
      sheetDetent = listDetent;
    };
  });

  /** Closes whatever is on top (Escape, or Android's back gesture); false when nothing is open. */
  function closeTop(): boolean {
    if (keysOpen) closeKeys();
    else if (helpOpen) helpOpen = false;
    else if (menu) menu = null;
    else if (place) place = null;
    else if (selectedId !== null) select(null);
    else if (pickedShip) selectShip(null);
    else if (pickedTrain) selectTrain(null);
    else if (boardCode) boardCode = null;
    else if (filtersOpen) filtersOpen = false;
    else if (layout.phone && sheetDetent !== "peek") sheetDetent = "peek";
    else return false;
    return true;
  }

  // Android's back gesture goes back in the page's history, and leaves the app
  // when there is none. While something is open, a history entry is kept for
  // it to take instead, which closes that thing.
  const android = typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent);
  const closable = $derived(
    keysOpen ||
      helpOpen ||
      menu !== null ||
      place !== null ||
      selectedId !== null ||
      pickedShip !== null ||
      pickedTrain !== null ||
      boardCode !== null ||
      filtersOpen ||
      (layout.phone && sheetDetent !== "peek"),
  );
  /** Whether the entry on top is ours, and whether we took it away ourselves. */
  let backArmed = false;
  let unarming = false;
  $effect(() => {
    if (!android) return;
    const armed = Boolean(page.state.back);
    // the gesture took our entry: close the top thing (the next effect arms again if more is open)
    if (backArmed && !armed) {
      if (unarming) unarming = false;
      else closeTop();
    }
    backArmed = armed;
  });
  $effect(() => {
    if (!android) return;
    const armed = Boolean(page.state.back);
    if (closable && !armed) pushState("", { back: true });
    else if (!closable && armed) {
      unarming = true;
      history.back();
    }
  });

  function closeKeys() {
    keysOpen = false;
    checkTrafikverketKey();
    const back = keysPrompt;
    keysPrompt = null;
    if (back) requestAnimationFrame(() => back()?.focus());
  }

  async function handleSignOut() {
    session = await signOut();
    refreshSoon();
    if (selectedId !== null) startFollow(selectedId);
  }

  const toggles: Record<string, () => void> = {
    "/": () => searchBox?.focus(),
    "?": () => (helpOpen = !helpOpen),
    f: () => (filtersOpen = !filtersOpen),
    c: () => {
      if (selectedId !== null) toggleFollow();
      else if (selectedTrain || selectedShip) followingVehicle = !followingVehicle;
    },
    t: () => globe.setPitch(view.pitch > 5 ? 0 : TILT),
    n: () => globe.resetOrientation(),
    "+": () => globe.zoomBy(1),
    "=": () => globe.zoomBy(1),
    "-": () => globe.zoomBy(-1),
    "1": () => (settings.basemap = "dark"),
    "2": () => (settings.basemap = "satellite"),
    "3": () => (settings.basemap = "realistic"),
    l: () => (settings.labels = !settings.labels),
    b: () => (settings.buildings = !settings.buildings),
    // the focus: Planes, traIns (T tilts), Ships; with Shift, that kind also shown on the map or not
    p: () => (settings.mode = "aircraft"),
    i: () => trainsOffered && (settings.mode = "trains"),
    s: () => (settings.mode = "ships"),
    P: () => toggleShown("aircraft"),
    I: () => trainsOffered && toggleShown("trains"),
    S: () => toggleShown("ships"),
    k: () => aircraftOn && (settings.aircraftLabels = !settings.aircraftLabels),
    d: () => (settings.daylight = !settings.daylight),
    w: () => (settings.weather = !settings.weather),
    r: () => (settings.railMap = !settings.railMap),
    e: () => (settings.terrain = !settings.terrain),
  };

  /** Dev builds: the API calls and the selected train's data (Ctrl/Cmd+Shift+D). */
  let devOpen = $state(false);

  function onkeydown(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    if (import.meta.env.DEV && (e.ctrlKey || e.metaKey) && e.shiftKey && e.code === "KeyD") {
      e.preventDefault();
      devOpen = !devOpen;
      return;
    }
    const typing = t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement || !!t?.isContentEditable;
    // Cmd/Ctrl+Alt+I: the web inspector, for bug reports
    if ((e.ctrlKey || e.metaKey) && e.altKey && e.code === "KeyI" && isTauri()) {
      e.preventDefault();
      invoke("open_devtools").catch(() => {});
      return;
    }
    // the keys page is modal: it handles its own keys, Escape included
    if (typing || keysOpen || e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === "Escape") {
      closeTop();
      return;
    }
    if (helpOpen && e.key !== "?") return;
    // a letter with Shift is its own shortcut (Shift+P, I, S), never the plain one; Caps Lock alone counts as plain
    let key = e.key;
    if (key.length === 1 && key.toLowerCase() !== key.toUpperCase()) key = e.shiftKey ? key.toUpperCase() : key.toLowerCase();
    const action = toggles[key];
    if (!action) return;
    e.preventDefault();
    action();
  }

  // quiet while the feed keeps its schedule; past about two refreshes, a warning with the data's age
  const freshness = $derived(shownFeed && focus === "aircraft" ? feedFreshness(Math.max(0, now - shownFeed.at), shownFeed.interval) : null);
  /** The credits for the trains on show; the sources whose licences ask for a link get one. */
  const trainCredit = $derived(trainsOn ? trainCredits(trainsOnMap) : []);
  const attribution = $derived(
    [
      aircraftOn && `Flight data: Flightradar24 (unofficial API${authenticated ? ", signed in" : ""})`,
      // DWD's open data licence asks to be named wherever its radar is shown
      settings.weather && "Radar: NOAA MRMS, Deutscher Wetterdienst, RainViewer",
      settings.basemap === "satellite" && "Imagery: Esri, Maxar, Earthstar Geographics",
      merged.added > 0 && "Track: OpenSky Network",
      // ODbL asks for attribution wherever the data is shown
      measured && "Aircraft data: © adsb.lol contributors, ODbL",
      shipsOn && "Ships © VesselFinder",
      // the rail map in the app is OpenRailwayMap's; elsewhere the OpenStreetMap data the map credit below covers
      settings.railMap && isTauri() && RAIL_SPEED_ATTRIBUTION,
      settings.terrain && TERRAIN_ATTRIBUTION,
      settings.basemap === "realistic" && "Textures: Streets GL (MIT)",
      settings.basemap === "realistic" && view.zoom >= SGL_ZOOM - 0.5 && SGL_ATTRIBUTION,
      settings.basemap === "realistic" && view.zoom >= SGL_ZOOM - 0.5 && OVERTURE_ATTRIBUTION,
      // the ground where the map leaves it unmapped (the flat map's), and what plants the trees close in (Globe's TREES_ZOOM)
      settings.basemap === "realistic" && view.zoom >= 12 && LANDCOVER_ATTRIBUTION,
      settings.basemap === "realistic" && view.zoom >= 14.5 && `${CANOPY_ATTRIBUTION} · ${STREET_TREES_ATTRIBUTION}`,
      // (Globe's ROAD_WIDTHS_ZOOM; the app asks with the Trafikverket key)
      settings.basemap === "realistic" && view.zoom >= 14 && isTauri() && "Road widths: Trafikverket NVDB",
      settings.basemap === "realistic" && "Earth: NASA Blue Marble",
      // (imagery.ts: from zoom 5 until the drawn ground takes over)
      settings.basemap === "realistic" && view.zoom >= 5 && view.zoom < 15.5 && IMAGERY_ATTRIBUTION,
      weatherLive && liveWeather && "Weather: MET Norway",
      "Map, names and buildings: OpenFreeMap, © OpenMapTiles, © OpenStreetMap contributors",
    ]
      .filter((c): c is string => Boolean(c)),
  );
  /** The trains' or ships' age, as the aircraft's: quiet while on schedule, a warning past about two refreshes. */
  const listFreshness = $derived(
    focus === "trains" && trainsAt
      ? feedFreshness(Math.max(0, now - trainsAt), TRAINS_REFRESH_MS)
      : focus === "ships" && shipsAt
        ? feedFreshness(Math.max(0, now - shipsAt), SHIPS_REFRESH_MS)
        : null,
  );
  const partial = $derived(snapshot?.errors.length ?? 0);
  const retryIn = $derived(Math.max(0, Math.ceil((nextRefreshAt - now) / 1000)));
  const count = (n: number) => n.toLocaleString();

  let timers: ReturnType<typeof setInterval>[] = [];
  onMount(() => {
    checkTrafikverketKey();
    // dev builds: `#@lat,lon,zoom,pitch,bearing` opens the map there
    const at = import.meta.env.DEV ? location.hash.match(/^#@(-?[\d.]+),(-?[\d.]+),([\d.]+)(?:,([\d.]+))?(?:,(-?[\d.]+))?/) : null;
    if (at) {
      const [lat, lon, zoom, pitch, bearing] = at.slice(1).map((v) => (v === undefined ? undefined : Number(v)));
      globe.jumpTo({ latitude: lat, longitude: lon, zoom, pitch: pitch ?? 0, bearing: bearing ?? 0 });
    }
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
    timers = [setInterval(() => focus === "aircraft" && refreshTop(), TOP_REFRESH_MS), setInterval(() => (now = Date.now()), 1000)];
  });
  onDestroy(() => {
    timers.forEach(clearInterval);
    clearTimeout(refreshTimer);
    clearTimeout(viewTimer);
    clearTimeout(keyRetry);
    stopShips();
    stopTrains();
  });
</script>

<svelte:window {onkeydown} />

<main class:phone={layout.phone} class:detail={detailOpen} style:--sheet-h={layout.phone ? `${sheetHeight}px` : null}>
  <Globe
    bind:this={globe}
    flights={shown}
    ships={shipsOnMap}
    {selected}
    details={shownDetails}
    {route}
    airport={boardAirport ?? null}
    {place}
    exaggeration={settings.exaggeration}
    basemap={settings.basemap}
    labels={settings.labels}
    labelScale={settings.labelSize}
    buildings={settings.buildings}
    sky={skyConditions}
    aircraft={aircraftOn}
    aircraftLabels={settings.aircraftLabels}
    daylight={settings.daylight}
    weather={settings.weather}
    trains={trainsOnMap}
    {hoveredTrainId}
    {hoveredShipMmsi}
    railMap={settings.railMap}
    trainBlocks={settings.railMap && settings.railShows === "blocks" && isTauri()}
    terrain={settings.terrain}
    tiltShift={settings.tiltShift}
    follow={following}
    followVehicle={followingVehicle}
    {selectedShip}
    {selectedTrain}
    trainRoute={shownRoute}
    ontrainrouteoff={() => reloadRoute?.()}
    onselect={(f) => {
      place = null;
      // a click on empty map lets go of a ship or a train too
      if (!f) {
        selectShip(null);
        selectTrain(null);
      }
      select(f?.id ?? null);
    }}
    onshipselect={(s) => {
      place = null;
      selectShip(s);
    }}
    ontrainselect={(t) => {
      place = null;
      selectTrain(t);
    }}
    onairport={openAirport}
    onviewchange={(v) => (view = v)}
  />

  {#snippet modeBar()}
    <ModeBar
      bind:mode={settings.mode}
      offered={{ aircraft: true, trains: trainsOffered || settings.mode === "trains", ships: true }}
      shown={kinds}
      ontoggle={toggleShown}
      counts={{
        aircraft: aircraftOn && snapshot ? flights.length : null,
        trains: trainsOn && trainsAt ? trains.length : null,
        ships: shipsOn && shipsAt ? ships.length : null,
      }}
    />
  {/snippet}

  {#if !layout.phone}{@render modeBar()}{/if}

  <div class="sidebar">
    <section class="surface primary">
      <header class="brand">
        <h1><Radar size={20} strokeWidth={1.75} /> RustRadar</h1>
        {#if focus === "aircraft"}
          {#if snapshot && freshness?.stale}
            <p class="status" title="Last update at {clockAt(Math.floor((shownFeed?.at ?? 0) / 1000))}{loading ? '; updating now' : ''}">
              <span class="badge warn">{freshness.label}</span>
            </p>
          {:else if snapshot}
            <p class="status" title="{snapshot.requests} requests in {snapshot.elapsedMs} ms">
              {#if loading}Updating…{:else}{freshness?.label}{/if}
            </p>
          {:else if !feedError}
            <p class="status"><span class="skeleton" style="width: 90px; height: 12px"></span></p>
          {/if}
        {:else if listFreshness}
          {#if listFreshness.stale}
            <p class="status"><span class="badge warn">{listFreshness.label}</span></p>
          {:else}
            <p class="status">{listFreshness.label}</p>
          {/if}
        {:else if isTauri()}
          <p class="status"><span class="skeleton" style="width: 90px; height: 12px"></span></p>
        {/if}
        <!-- the age ticks every second: announce only the change to stale -->
        <span class="visually-hidden" role="status">{freshness?.stale ? "Flight data is out of date" : ""}</span>
      </header>

      {#if focus === "aircraft"}
        <SearchBox bind:this={searchBox} onpick={pickSearch} onplace={pickPlace} onquery={() => (place = null)} {near} />
      {:else if focus === "trains"}
        <SearchBox
          bind:this={searchBox}
          local={trainSearch}
          bind:query={trainQuery}
          onpick={pickSearch}
          onplace={pickPlace}
          onquery={() => (place = null)}
          {near}
        />
      {:else}
        <SearchBox
          bind:this={searchBox}
          local={shipSearch}
          bind:query={shipQuery}
          onpick={pickSearch}
          onplace={pickPlace}
          onquery={() => (place = null)}
          {near}
        />
      {/if}

      {#if layout.phone}{@render modeBar()}{:else}{@render toolbar()}{/if}

      {#if !isTauri()}
        <div class="alert" role="alert">
          <TriangleAlert size={16} strokeWidth={1.75} />
          <div>
            <p>No live {focus === "aircraft" ? "flight" : focus === "trains" ? "train" : "ship"} data in a plain browser.</p>
            <p class="detail">{feedError}</p>
          </div>
        </div>
      {:else if focus !== "aircraft"}
        <!-- the trains' and ships' failures are told in their lists -->
      {:else if rateLimited}
        <!-- expected throttling, not a failure: the warning tone, still in plain sight -->
        <div class="alert warn" role="status">
          <TriangleAlert size={16} strokeWidth={1.75} />
          <div>
            <p>Flightradar24 is limiting requests. Slowing down; next update in {retryIn} s.</p>
          </div>
        </div>
      {:else if feedError}
        <div class="alert" role="alert">
          <TriangleAlert size={16} strokeWidth={1.75} />
          <div>
            <p>Can't reach Flightradar24. Trying again in {retryIn} s.</p>
            <p class="detail">{feedError}</p>
            <button class="button" onclick={refresh} disabled={loading}>
              <RefreshCw size={14} strokeWidth={1.75} /> Retry now
            </button>
          </div>
        </div>
      {:else if partial}
        <p class="note">{partial} region{partial === 1 ? "" : "s"} didn't load this round; some aircraft may be missing.</p>
      {/if}
    </section>

    {#if !layout.phone}{@render sheetContent()}{/if}
  </div>

  {#snippet toolbar()}
    <div class="toolbar">
      {@render filterToggle(focus === "aircraft" ? active : focus === "trains" ? trainFilterActive : shipFilterActive)}
      {#if focus === "aircraft" && snapshot}
        <p class="counts">
          <span class="count">{count(filtered.length)}</span>
          {#if filtered.length !== flights.length}of {count(flights.length)}{/if}
          <span class="noun">flights</span>
          {regional ? "in view" : "worldwide"}
        </p>
      {:else if focus === "trains" && trainsAt}
        <p class="counts">
          <span class="count">{count(trainsListed.length)}</span>
          {#if trainsListed.length !== trains.length}of {count(trains.length)}{/if}
          <span class="noun">trains</span> in view
        </p>
      {:else if focus === "ships" && shipsAt}
        <p class="counts">
          <span class="count">{count(shipsListed.length)}</span>
          {#if shipsListed.length !== ships.length}of {count(ships.length)}{/if}
          <span class="noun">ships</span> in view
        </p>
      {/if}
    </div>
  {/snippet}

  {#snippet sheetContent()}
  {#if filtersOpen}
    {#if focus === "aircraft"}
      <FilterPanel bind:filters {flights} shown={filtered.length} onclose={() => (filtersOpen = false)} />
    {:else if focus === "trains"}
      <TrainFilterPanel bind:filters={trainFilters} {trains} shown={trainsFiltered.length} onclose={() => (filtersOpen = false)} />
    {:else}
      <ShipFilterPanel
        bind:filters={shipFilters}
        {ships}
        shown={shipsFiltered.length}
        sizesKnown={shipSizesKnown}
        onclose={() => (filtersOpen = false)}
      />
    {/if}
  {/if}

  <section class="surface list">
    {#if focus === "aircraft"}
      <TopFlights
        flights={top}
        failed={topFailed}
        {selectedId}
        onpick={(f) => select(f.flight_id, { fly: true })}
        onretry={refreshTop}
      />
    {:else if focus === "trains"}
      <TrainList
        trains={trainsListed}
        total={trains.length}
        loaded={trainsAt > 0}
        failed={trainsFailed || !isTauri()}
        bind:sort={trainFilters.sort}
        centre={listCentre}
        selectedId={selectedTrain?.id ?? null}
        onpick={pickTrain}
        onhover={(id) => (hoveredTrainId = id)}
        onshowall={() => {
          trainQuery = "";
          Object.assign(trainFilters, { ...structuredClone(DEFAULT_TRAIN_FILTERS), sort: trainFilters.sort });
        }}
      />
    {:else}
      <ShipList
        ships={shipsListed}
        total={ships.length}
        loaded={shipsAt > 0}
        failed={shipsFailed || !isTauri()}
        bind:sort={shipFilters.sort}
        centre={listCentre}
        sizesKnown={shipSizesKnown}
        selectedMmsi={selectedShip?.mmsi ?? null}
        onpick={pickShip}
        onhover={(mmsi) => (hoveredShipMmsi = mmsi)}
        onshowall={() => {
          shipQuery = "";
          Object.assign(shipFilters, { ...structuredClone(DEFAULT_SHIP_FILTERS), sort: shipFilters.sort });
        }}
      />
    {/if}
  </section>
  {/snippet}

  <!-- On a phone, one sheet: the list, or in its place the details of what is picked, lowered to their summary so the map stays in view -->
  {#if layout.phone}
    <BottomSheet
      bind:detent={sheetDetent}
      bind:height={sheetHeight}
      label={detailOpen ? "Details" : focus === "aircraft" ? "Flights" : focus === "trains" ? "Trains" : "Ships"}
      clear={detailOpen ? 12 : 64}
      head={detailOpen ? undefined : listHead}
    >
      {#if detailOpen}{@render panels()}{:else}{@render sheetContent()}{/if}
    </BottomSheet>
  {/if}
  {#snippet listHead()}{@render toolbar()}{/snippet}

  {#snippet filterToggle(n: number)}
    <button
      class="button filter-toggle"
      aria-pressed={filtersOpen}
      aria-expanded={filtersOpen}
      title="Filters (F)"
      onclick={() => (filtersOpen = !filtersOpen)}
    >
      <Funnel size={14} strokeWidth={1.75} />
      Filters
      {#if n}<span class="active-count" aria-label="{n} active">{n}</span>{/if}
    </button>
  {/snippet}

  {#snippet panels()}
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
      {measured}
      {route}
      status={followState}
      lastSeenMs={followState === "ended" ? endedSeenMs : 0}
      {authenticated}
      {following}
      back={fromBoard && boardCode ? { label: `${boardCode} departures and arrivals`, onclick: () => select(null) } : null}
      onfollow={toggleFollow}
      onselect={(id) => select(id, { fly: true })}
      onairport={openAirport}
      onsignin={promptSignIn}
      onclose={() => select(null)}
    />
  {/if}

  {#if selectedShip}
    <ShipPanel
      ship={selectedShip}
      details={shipCard?.mmsi === selectedShip.mmsi ? shipCard.details : null}
      error={shipCard?.mmsi === selectedShip.mmsi ? shipCard.error : null}
      {now}
      onretry={() => loadShipCard(selectedShip.mmsi)}
      following={followingVehicle}
      onfollow={() => (followingVehicle = !followingVehicle)}
      onclose={() => selectShip(null)}
    />
  {/if}

  {#if selectedTrain}
    <TrainPanel
      train={selectedTrain}
      {now}
      following={followingVehicle}
      onfollow={() => (followingVehicle = !followingVehicle)}
      onclose={() => selectTrain(null)}
      {liveStops}
    />
  {/if}
  {/snippet}

  {#if !layout.phone}{@render panels()}{/if}

  <MapControls
    zoom={view.zoom}
    bearing={view.bearing}
    pitch={view.pitch}
    bind:basemap={settings.basemap}
    bind:labels={settings.labels}
    bind:labelSize={settings.labelSize}
    bind:buildings={settings.buildings}
    aircraftShown={aircraftOn}
    bind:aircraftLabels={settings.aircraftLabels}
    bind:daylight={settings.daylight}
    bind:weather={settings.weather}
    bind:adsb={settings.adsb}
    bind:railMap={settings.railMap}
    bind:railShows={settings.railShows}
    bind:sky={settings.sky}
    bind:tiltShift={settings.tiltShift}
    bind:terrain={settings.terrain}
    bind:exaggeration={settings.exaggeration}
    {session}
    inset={!layout.phone && (selected || boardCode || selectedShip || selectedTrain) ? PANEL_INSET : 0}
    onzoom={(d) => globe.zoomBy(d)}
    onreset={() => globe.resetOrientation()}
    ontilt={(p) => globe.setPitch(p)}
    bind:menu
    onkeys={() => (keysOpen = true)}
    onhelp={() => (helpOpen = true)}
  />

  <ShortcutsHelp open={helpOpen} onclose={() => (helpOpen = false)} />
  {#if import.meta.env.DEV}
    <DevPanel bind:open={devOpen} train={selectedTrain} stops={liveStops} route={shownRoute} />
  {/if}
  <KeysDialog
    open={keysOpen}
    {session}
    focusLogin={keysPrompt !== null}
    onclose={closeKeys}
    onsignin={handleSignIn}
    onsignout={handleSignOut}
  />

  <!-- the train credits come first, so the line's ellipsis can't hide them -->
  <p class="attribution" title={[...trainCredit.map((c) => c.text), ...attribution].join(" · ")}>
    {#each trainCredit as c, i (c.url)}
      {i ? " · " : ""}<a href={c.url} onclick={(e) => (e.preventDefault(), openUrl(c.url))}>{c.text}</a>
    {/each}{#each attribution as text, i (i)}{i || trainCredit.length ? " · " : ""}{text}{/each}
  </p>
</main>

<style>
  main {
    position: fixed;
    inset: 0;
    background: var(--ink);
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
  /* stale: the status's own size, so the escalation reads as the same line raised */
  .status .badge {
    font-size: var(--text-sm);
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
  /* beside the mode buttons it goes without saying; in a phone's sheet, under icons alone, it doesn't */
  .noun {
    display: none;
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
  .alert.warn {
    background: var(--warn-soft);
    color: var(--warn);
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
    background: oklch(0.15 0 0 / 0.85);
    font-size: var(--text-xs);
    color: var(--text-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .attribution a {
    color: var(--text-2);
    text-decoration: underline;
    text-decoration-color: oklch(1 0 0 / 0.25);
    text-underline-offset: 2px;
  }
  .attribution a:hover {
    color: var(--text-1);
  }
  /* Phones (the query in phone.svelte.ts): the search and mode buttons in one
     row along the top, the lists and filters in the bottom sheet. With
     something picked, its details take the sheet and the search steps aside. */
  @media (max-width: 899px), (max-height: 500px) {
    .sidebar {
      top: max(var(--space-2), env(safe-area-inset-top, 0px));
      left: max(var(--space-2), env(safe-area-inset-left, 0px));
      right: max(var(--space-2), env(safe-area-inset-right, 0px));
      bottom: auto;
      width: auto;
      z-index: 3;
      transition:
        transform 220ms var(--ease-out),
        opacity 220ms var(--ease-out);
    }
    .detail .sidebar {
      transform: translateY(calc(-100% - var(--space-2)));
      opacity: 0;
      visibility: hidden;
      transition:
        transform 220ms var(--ease-out),
        opacity 220ms var(--ease-out),
        visibility 0s 220ms;
    }
    .primary {
      flex-flow: row wrap;
      align-items: center;
      padding: var(--space-1);
      gap: var(--space-1);
    }
    .primary > :global(.search) {
      flex: 1;
      min-width: 0;
    }
    .primary > .alert,
    .primary > .note {
      flex-basis: 100%;
    }
    .brand {
      display: none;
    }
    /* the sheet's head is its title: the count on the left, the filters on the right */
    .toolbar {
      flex-direction: row-reverse;
    }
    .counts .count {
      font-size: var(--text-base);
    }
    .noun {
      display: inline;
    }
    .filter-toggle {
      height: 32px;
      padding: 0 var(--space-3);
    }
    .list {
      flex: 1 1 auto;
      border: 0;
      border-radius: 0;
      background: none;
      box-shadow: none;
      overflow: visible;
    }
    :global(.sheet) :global(.filters) {
      flex: none;
      margin: 0 var(--space-2) var(--space-2);
      box-shadow: none;
    }
    .attribution {
      bottom: var(--sheet-h, 0px);
      left: 0;
      max-width: none;
      border-radius: 0;
      font-size: 10px;
      z-index: 2;
    }
  }
  /* a phone on its side: the search and list down the left, the map beside them */
  @media (max-height: 500px) and (orientation: landscape) {
    .sidebar {
      right: auto;
      width: 320px;
    }
    .attribution {
      bottom: 0;
      left: auto;
      max-width: calc(100% - 360px);
    }
  }
</style>
