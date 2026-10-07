<script lang="ts" module>
  export type FollowState = "connecting" | "live" | "reconnecting" | "ended";
</script>

<script lang="ts">
  import { untrack } from "svelte";
  import {
    ArrowLeft,
    ChevronLeft,
    ChevronRight,
    ExternalLink,
    LoaderCircle,
    LocateFixed,
    Plane,
    RefreshCw,
    X,
  } from "@lucide/svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { aircraftHistory, airlineLogoUrl, type FlightDetails, type HistoryRow, type LiveFlight } from "./api";
  import AltitudeChart from "./AltitudeChart.svelte";
  import { ALTITUDE_STOPS } from "./geo";
  import { airlineIcao, reference } from "./reference.svelte";
  import { airacCycle, type Route } from "./routes";
  import { clockAt, dayAt, delayMinutes, duration, punctuality, toneOf } from "./time";

  interface Props {
    flight: LiveFlight;
    details: FlightDetails | null;
    route: Route | null;
    status: FollowState;
    /** Squawk, vertical speed, airspace and flight plans are withheld from anonymous users. */
    authenticated: boolean;
    /** Whether the camera follows this aircraft. */
    following: boolean;
    /** Return to the airport board this flight was opened from. */
    back?: { label: string; onclick: () => void } | null;
    onfollow: () => void;
    /** Show another flight, e.g. from this aircraft's history. */
    onselect: (flightId: number) => void;
    /** Open an airport's board. */
    onairport: (code: string) => void;
    onsignin: () => void;
    onclose: () => void;
  }

  let {
    flight,
    details,
    route,
    status,
    authenticated,
    following,
    back = null,
    onfollow,
    onselect,
    onairport,
    onsignin,
    onclose,
  }: Props = $props();

  type Tab = "overview" | "route" | "aircraft" | "history";
  const TABS: [Tab, string][] = [
    ["overview", "Overview"],
    ["route", "Route"],
    ["aircraft", "Aircraft"],
    ["history", "History"],
  ];

  const LOGIN_HINT = "Shown to signed-in Flightradar24 users only";
  const info = $derived(details?.flight ?? null);
  const aircraft = $derived(details?.aircraft ?? null);
  const schedule = $derived(details?.schedule ?? null);
  const progress = $derived(details?.progress ?? null);
  const loading = $derived(!details && status !== "ended");
  const flightId = $derived(flight.id);

  let tab = $state<Tab>("overview");
  let photo = $state(0);
  let logoFailed = $state(0);
  $effect(() => {
    void flightId;
    photo = 0;
  });

  const images = $derived(aircraft?.images ?? []);
  const image = $derived(images.length ? images[photo % images.length] : null);

  const altitude = $derived(info?.altitude ?? flight.alt);
  const onGround = $derived(info?.on_ground ?? flight.onGround);

  /** Aviation's own notation: flight levels above the (US) transition altitude. */
  const level = $derived(
    onGround ? "GND" : altitude >= 18_000 ? `FL${String(Math.round(altitude / 100)).padStart(3, "0")}` : `${altitude.toLocaleString()} ft`,
  );

  const airline = $derived(reference.airline(airlineIcao(flight.callsign)));
  const logoId = $derived(schedule?.painted_as_id || schedule?.operated_by_id || 0);
  const origin = $derived(route?.origin);
  const destination = $derived(route?.destination);

  const km = (m: number) => `${Math.round(m / 1000).toLocaleString()} km`;
  // squawk codes are four octal digits, transmitted in base 10
  const squawk = (code: number) => (code ? code.toString(8).padStart(4, "0") : "—");
  const sentence = (s: string) => s.charAt(0) + s.slice(1).toLowerCase().replaceAll("_", " ");

  const departure = $derived.by(() => {
    if (!schedule) return { label: "Departure", time: 0, delay: null };
    const late = punctuality(delayMinutes(schedule.scheduled_departure, schedule.actual_departure));
    if (schedule.actual_departure) return { label: "Departed", time: schedule.actual_departure, delay: late };
    return { label: "Scheduled", time: schedule.scheduled_departure, delay: null };
  });
  const arrival = $derived.by(() => {
    if (!schedule) return { label: "Arrival", time: 0, delay: null };
    const late = punctuality(delayMinutes(schedule.scheduled_arrival, schedule.actual_arrival || progress?.eta));
    if (schedule.actual_arrival) return { label: "Arrived", time: schedule.actual_arrival, delay: late };
    if (progress?.eta) return { label: "Expected", time: progress.eta, delay: late };
    return { label: "Scheduled", time: schedule.scheduled_arrival, delay: null };
  });
  const pct = $derived(Math.min(100, Math.max(0, progress?.progress_pct ?? 0)));
  const diverted = $derived(!!schedule?.diverted_id && schedule.diverted_id !== schedule.destination_id);

  const trailGradient = ALTITUDE_STOPS.map(
    ([alt, c]) => `rgb(${c.join(" ")}) ${(alt / ALTITUDE_STOPS[ALTITUDE_STOPS.length - 1][0]) * 100}%`,
  ).join(", ");

  const airac = airacCycle();
  const day = (d: Date) => d.toLocaleDateString([], { day: "numeric", month: "short", timeZone: "UTC" });

  const fr24Url = $derived(
    `https://www.flightradar24.com/${encodeURIComponent(flight.callsign || "flight")}/${flight.id.toString(16)}`,
  );

  // the aircraft's other flights, fetched when the History tab first opens
  const registration = $derived(aircraft?.reg || flight.reg);
  let history = $state.raw<{ reg: string; rows: HistoryRow[] | null; failed: boolean } | null>(null);
  function loadHistory(reg: string) {
    history = { reg, rows: null, failed: false };
    aircraftHistory(reg)
      .then((rows) => {
        if (history?.reg === reg) history = { reg, rows, failed: false };
      })
      .catch(() => {
        if (history?.reg === reg) history = { reg, rows: null, failed: true };
      });
  }
  $effect(() => {
    if (tab !== "history" || !registration) return;
    const reg = registration;
    untrack(() => history?.reg !== reg && loadHistory(reg));
  });

  function tabKeys(e: KeyboardEvent) {
    const i = TABS.findIndex(([t]) => t === tab);
    const next = e.key === "ArrowRight" ? i + 1 : e.key === "ArrowLeft" ? i - 1 : null;
    if (next === null) return;
    e.preventDefault();
    tab = TABS[(next + TABS.length) % TABS.length][0];
    document.getElementById(`tab-${tab}`)?.focus();
  }

  const historyTimes = (r: HistoryRow) => {
    const from = reference.airport(r.origin_iata)?.timezone;
    const to = reference.airport(r.destination_iata)?.timezone;
    return {
      day: r.stod ? dayAt(r.stod, from) : "",
      dep: clockAt(r.atod || r.etod || r.stod, from),
      arr: clockAt(r.atoa || r.etoa || r.stoa, to),
    };
  };
</script>

<aside class="panel surface" aria-label="Selected flight">
  {#if back}
    <button class="back" onclick={back.onclick}><ArrowLeft size={14} strokeWidth={2} /> {back.label}</button>
  {/if}

  <header class="strip">
    <div class="cell id">
      <h2 class="data">{flight.callsign || flight.flight || "No callsign"}</h2>
      <p>{flight.flight || schedule?.flight_number || "No flight number"}</p>
    </div>
    <div class="cell">
      <span class="data value">{aircraft?.typecode || flight.typecode || "—"}</span>
      <span class="data sub">{aircraft?.reg || flight.reg || "—"}</span>
    </div>
    <div class="cell">
      <span class="data value level">{level}</span>
      <span class="data sub">{info?.ground_speed ?? flight.speed} kt</span>
    </div>
    <button class="icon-button close" aria-label="Close flight details" title="Close (Esc)" onclick={onclose}>
      <X size={18} strokeWidth={1.75} />
    </button>
  </header>

  {#if loading}
    <div class="photo skeleton"></div>
  {:else if image}
    <div class="photo">
      <button class="image" onclick={() => openUrl(image.url)} title="View this photo on JetPhotos">
        <img src={image.medium || image.thumbnail} alt="{aircraft?.full_description ?? 'Aircraft'} {aircraft?.reg ?? ''}" />
      </button>
      {#if image.copyright}<span class="credit">© {image.copyright}</span>{/if}
      {#if images.length > 1}
        <button class="nav previous" aria-label="Previous photo" onclick={() => (photo = (photo - 1 + images.length) % images.length)}>
          <ChevronLeft size={18} strokeWidth={2} />
        </button>
        <button class="nav next" aria-label="Next photo" onclick={() => (photo = (photo + 1) % images.length)}>
          <ChevronRight size={18} strokeWidth={2} />
        </button>
        <span class="count data">{(photo % images.length) + 1}/{images.length}</span>
      {/if}
    </div>
  {/if}

  {#if airline || logoId}
    <div class="airline">
      {#if logoId && logoFailed !== logoId}
        <span class="logo"><img src={airlineLogoUrl(logoId)} alt="" onerror={() => (logoFailed = logoId)} /></span>
      {/if}
      <span>{airline?.name ?? ""}</span>
    </div>
  {/if}

  <section class="route" aria-label="Route">
    <div class="airports">
      {#each [{ code: origin?.iata || flight.origin, airport: origin, time: departure, end: false }, { code: destination?.iata || flight.destination, airport: destination, time: arrival, end: true }] as side (side.end)}
        <div class:end={side.end}>
          {#if side.code}
            <button class="code data" title="Departures and arrivals at {side.airport?.name ?? side.code}" onclick={() => onairport(side.code)}>
              {side.code}
            </button>
          {:else}
            <span class="code data">—</span>
          {/if}
          <span class="city" title={side.airport?.name}>{side.airport?.city || side.airport?.name || ""}</span>
          <span class="when">
            {side.time.label} <span class="data">{clockAt(side.time.time, side.airport?.timezone)}</span>
          </span>
          {#if side.time.delay}<span class="badge {side.time.delay.tone}">{side.time.delay.label}</span>{/if}
        </div>
      {/each}
    </div>
    <div class="track" role="progressbar" aria-label="Flight progress" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
      <div class="fill" style="width: {pct}%"></div>
      <span class="marker" style="left: {pct}%"><Plane size={14} strokeWidth={2} /></span>
    </div>
    {#if progress}
      <p class="distances">
        {km(progress.traversed_distance)} flown · {km(progress.remaining_distance)} to go{progress.remaining_time > 0
          ? ` · ${duration(progress.remaining_time)} left`
          : ""}
      </p>
    {:else if loading}
      <div class="skeleton line"></div>
    {/if}
    {#if diverted}
      <p class="badge bad">Diverted to {destination?.name ?? "another airport"}</p>
    {/if}
    {#if schedule}<p class="note">Times are local to each airport.</p>{/if}
  </section>

  <div class="tabs" role="tablist" aria-label="Flight information">
    {#each TABS as [id, label] (id)}
      <button
        id="tab-{id}"
        role="tab"
        aria-selected={tab === id}
        aria-controls="tabpanel"
        tabindex={tab === id ? 0 : -1}
        onclick={() => (tab = id)}
        onkeydown={tabKeys}
      >
        {label}
      </button>
    {/each}
  </div>

  <div class="tabpanel" id="tabpanel" role="tabpanel" aria-labelledby="tab-{tab}">
    {#if tab === "overview"}
      <dl class="facts">
        <div><dt>Altitude</dt><dd class="data">{onGround ? "On ground" : `${altitude.toLocaleString()} ft`}</dd></div>
        <div><dt>Ground speed</dt><dd class="data">{info?.ground_speed ?? flight.speed} kt</dd></div>
        <div title={authenticated ? undefined : LOGIN_HINT}>
          <dt>Vertical speed</dt>
          <dd class="data">{info && authenticated ? `${info.vertical_speed > 0 ? "+" : ""}${info.vertical_speed} fpm` : "—"}</dd>
        </div>
        <div><dt>Track</dt><dd class="data">{info?.track ?? flight.track}°</dd></div>
        <div title={authenticated ? undefined : LOGIN_HINT}><dt>Squawk</dt><dd class="data">{squawk(info?.squawk ?? 0)}</dd></div>
        <div><dt>Phase</dt><dd>{progress ? sentence(progress.flight_stage) : "—"}</dd></div>
        <div class="wide" title={authenticated ? undefined : LOGIN_HINT}><dt>Airspace</dt><dd>{info?.airspace || "—"}</dd></div>
      </dl>
      <AltitudeChart trail={details?.trail ?? []} timeZone={origin?.timezone} place={origin?.iata || flight.origin || undefined} />
    {:else if tab === "route"}
      <div class="stack">
        <div class="kind">
          {#if route?.kind === "filed"}
            <span class="badge good">Filed flight plan</span>
          {:else if route?.kind === "estimated"}
            <span class="badge neutral">Estimated</span>
          {:else}
            <span class="badge neutral">No route known</span>
          {/if}
          <span class="airac" title="Effective {day(airac.effective)} to {day(airac.expires)}, 00:00 UTC">
            AIRAC <span class="data">{airac.ident}</span>
          </span>
        </div>

        {#if route?.kind === "filed" && details?.flight_plan}
          {@const plan = details.flight_plan}
          <p class="filed data">{plan.route || "Direct"}</p>
          <dl class="facts compact">
            <div><dt>Waypoints</dt><dd class="data">{plan.waypoints.length}</dd></div>
            {#if plan.alternates.length}<div class="wide"><dt>Alternates</dt><dd class="data">{plan.alternates.join(", ")}</dd></div>{/if}
          </dl>
          <p class="note">
            Flight plans are filed against the current AIRAC cycle, {airac.ident}, effective {day(airac.effective)} to
            {day(airac.expires)}.
          </p>
        {:else}
          <p class="explain">
            The map shows the track flown so far and the great-circle path to the destination. That's an estimate:
            the filed route follows airways and waypoints of AIRAC cycle {airac.ident}, and Flightradar24 shares flight
            plans only with signed-in subscribers.
          </p>
          {#if !authenticated}
            <button class="button" onclick={onsignin}>Sign in to Flightradar24</button>
          {/if}
        {/if}

        {#if progress?.great_circle_distance}
          <dl class="facts compact">
            <div><dt>Great circle</dt><dd class="data">{km(progress.great_circle_distance)}</dd></div>
            <div><dt>Flown</dt><dd class="data">{km(progress.traversed_distance)}</dd></div>
            <div><dt>Typical time</dt><dd class="data">{progress.mean_flight_time ? duration(progress.mean_flight_time) : "—"}</dd></div>
          </dl>
        {/if}

        <ul class="legend" aria-label="Map key">
          <li>
            <span class="swatch gradient" style="background: linear-gradient(to right, {trailGradient})"></span>
            <span>Track flown, coloured by altitude <span class="data scale">0 – 45,000 ft</span></span>
          </li>
          {#if route?.kind === "filed"}
            <li><span class="swatch solid"></span><span>Filed route</span></li>
          {:else}
            <li><span class="swatch ahead"></span><span>Great circle to the destination</span></li>
            <li><span class="swatch before"></span><span>Departure to the first tracked position</span></li>
          {/if}
        </ul>
      </div>
    {:else if tab === "aircraft"}
      <dl class="facts">
        <div class="wide"><dt>Type</dt><dd>{aircraft?.full_description || flight.typecode || "—"}</dd></div>
        <div><dt>Type code</dt><dd class="data">{aircraft?.typecode || flight.typecode || "—"}</dd></div>
        <div><dt>Registration</dt><dd class="data">{aircraft?.reg || flight.reg || "—"}</dd></div>
        <div>
          <dt>Mode S</dt>
          <dd class="data">{aircraft?.icao_address ? aircraft.icao_address.toString(16).toUpperCase().padStart(6, "0") : "—"}</dd>
        </div>
        <div><dt>Serial number</dt><dd class="data">{aircraft?.msn || "—"}</dd></div>
        <div>
          <dt>Age</dt>
          <dd class="data">{aircraft?.age != null ? `${aircraft.age} year${aircraft.age === 1 ? "" : "s"}` : "—"}</dd>
        </div>
        <div><dt>First flight</dt><dd class="data">{aircraft?.birth_date || "—"}</dd></div>
        <div class="wide"><dt>Owner</dt><dd>{aircraft?.registered_owners || "—"}</dd></div>
      </dl>
    {:else}
      <div class="history">
        {#if !registration}
          <p class="explain">This aircraft's registration isn't known, so its other flights can't be looked up.</p>
        {:else if history?.failed}
          <div class="stack">
            <p class="explain">Couldn't load the flights of {registration}.</p>
            <button class="button" onclick={() => loadHistory(registration)}>
              <RefreshCw size={14} strokeWidth={1.75} /> Try again
            </button>
          </div>
        {:else if !history?.rows}
          <ol aria-busy="true">
            {#each Array(5) as _, i (i)}
              <li class="row placeholder"><span class="skeleton" style="width: {150 + ((i * 37) % 90)}px"></span></li>
            {/each}
          </ol>
        {:else if history.rows.length === 0}
          <p class="explain">No recent flights recorded for {registration}.</p>
        {:else}
          <ol>
            {#each history.rows as r, i (r.flight_id ?? `${r.stod}-${i}`)}
              {@const t = historyTimes(r)}
              {@const tone = toneOf(r.status_color)}
              {@const current = r.flight_id === flight.id}
              <li>
                <button class="row" class:current disabled={!r.live || !r.flight_id || current} onclick={() => r.flight_id && onselect(r.flight_id)}>
                  <span class="day">{t.day}</span>
                  <span class="data number">{r.number || r.callsign || "—"}</span>
                  <span class="data legs">{r.origin_iata || "?"} – {r.destination_iata || "?"}</span>
                  <span class="data times">{t.dep} – {t.arr}</span>
                  <span class="status-text" class:good={tone === "good"} class:warn={tone === "warn"} class:bad={tone === "bad"}>
                    {current ? "This flight" : r.status || ""}
                  </span>
                </button>
              </li>
            {/each}
          </ol>
          <p class="note">Live flights open on the map. Times are local to each airport.</p>
        {/if}
      </div>
    {/if}
  </div>

  <footer>
    {#if status === "live"}
      <span class="status live"><span class="dot"></span>Live</span>
    {:else if status === "ended"}
      <span class="status ended">No longer live</span>
    {:else}
      <span class="status">
        <span class="spin"><LoaderCircle size={14} strokeWidth={2} /></span>
        {status === "connecting" ? "Connecting…" : "Reconnecting…"}
      </span>
    {/if}
    <div class="actions">
      <button
        class="icon-button"
        aria-pressed={following}
        aria-label={following ? "Stop following" : "Follow with the camera"}
        title={following ? "Stop following (C)" : "Follow with the camera (C)"}
        onclick={onfollow}
      >
        <LocateFixed size={18} strokeWidth={1.75} />
      </button>
      <button class="button" onclick={() => openUrl(fr24Url)}>
        Flightradar24 <ExternalLink size={14} strokeWidth={1.75} />
      </button>
    </div>
  </footer>
</aside>

<style>
  .panel {
    position: absolute;
    top: var(--space-4);
    right: var(--space-4);
    width: 360px;
    max-height: calc(100% - var(--space-4) * 2);
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    animation: enter 200ms var(--ease-out);
  }
  @keyframes enter {
    from {
      opacity: 0;
      transform: translateX(8px);
    }
  }

  .back {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    padding: var(--space-2) var(--space-4);
    border: 0;
    border-bottom: 1px solid var(--line);
    background: none;
    font-size: var(--text-sm);
    color: var(--text-2);
    text-align: left;
    cursor: pointer;
  }
  .back:hover {
    color: var(--text-1);
    background: var(--surface-hover);
  }

  /* the signature: an ATC flight-progress strip's column rhythm */
  .strip {
    display: grid;
    grid-template-columns: 1fr auto auto 40px;
    border-bottom: 1px solid var(--line);
  }
  .cell {
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 2px;
    padding: var(--space-3) var(--space-3);
    border-left: 1px solid var(--line);
    min-width: 0;
  }
  .cell.id {
    border-left: 0;
    padding-left: var(--space-4);
  }
  .strip h2 {
    font-size: var(--text-xl);
    font-weight: 700;
    line-height: 1.1;
    letter-spacing: 0;
  }
  .strip p,
  .sub {
    font-size: var(--text-sm);
    color: var(--text-2);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .value {
    font-size: var(--text-base);
    font-weight: 650;
  }
  .level {
    color: var(--amber);
  }
  .close {
    align-self: center;
    justify-self: center;
  }

  .photo {
    position: relative;
    flex: none;
    aspect-ratio: 16 / 9;
    background: var(--surface-raised);
    overflow: hidden;
  }
  .image {
    display: block;
    width: 100%;
    height: 100%;
    padding: 0;
    border: 0;
    background: none;
    cursor: pointer;
  }
  .photo img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .credit,
  .count {
    position: absolute;
    bottom: var(--space-1);
    font-size: var(--text-xs);
    color: var(--text-1);
    text-shadow: 0 1px 2px oklch(0 0 0 / 0.9);
    pointer-events: none;
  }
  .credit {
    right: var(--space-2);
  }
  .count {
    left: var(--space-2);
  }
  .nav {
    position: absolute;
    top: 50%;
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: oklch(0.15 0.02 255 / 0.7);
    color: var(--text-1);
    transform: translateY(-50%);
    opacity: 0;
    cursor: pointer;
    transition: opacity 150ms var(--ease-out);
  }
  .photo:hover .nav,
  .nav:focus-visible {
    opacity: 1;
  }
  .previous {
    left: var(--space-2);
  }
  .next {
    right: var(--space-2);
  }

  .airline {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4) 0;
    font-size: var(--text-md);
    font-weight: 600;
  }
  /* logos are drawn for light backgrounds */
  .logo {
    display: inline-grid;
    place-items: center;
    height: 26px;
    padding: 2px 6px;
    border-radius: 4px;
    background: var(--text-1);
  }
  .logo img {
    max-height: 22px;
    max-width: 110px;
  }

  .route {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-4);
    border-bottom: 1px solid var(--line);
  }
  .airports {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-3);
  }
  .airports > div {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    min-width: 0;
  }
  .airports .end {
    align-items: flex-end;
    text-align: right;
  }
  .code {
    padding: 0;
    border: 0;
    background: none;
    font-size: var(--text-xl);
    font-weight: 700;
    line-height: 1.1;
  }
  button.code {
    cursor: pointer;
    text-decoration: underline transparent;
    text-underline-offset: 3px;
    transition: text-decoration-color 150ms var(--ease-out);
  }
  button.code:hover {
    text-decoration-color: var(--amber);
  }
  .city {
    max-width: 100%;
    font-size: var(--text-sm);
    color: var(--text-1);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .when {
    font-size: var(--text-sm);
    color: var(--text-2);
  }
  .badge {
    margin-top: 2px;
  }
  .track {
    position: relative;
    height: 2px;
    margin: var(--space-2) 7px;
    background: var(--line-control);
  }
  .fill {
    height: 100%;
    background: var(--amber);
    transition: width 600ms var(--ease-out);
  }
  .marker {
    position: absolute;
    top: 50%;
    display: grid;
    color: var(--amber);
    transform: translate(-50%, -50%) rotate(45deg);
    transition: left 600ms var(--ease-out);
  }
  .distances {
    font-size: var(--text-sm);
    font-variant-numeric: tabular-nums;
    color: var(--text-2);
  }
  .note {
    font-size: var(--text-xs);
    color: var(--text-3);
  }
  .skeleton.line {
    height: 12px;
    width: 70%;
  }

  .tabs {
    display: flex;
    gap: var(--space-1);
    padding: 0 var(--space-3);
    border-bottom: 1px solid var(--line);
  }
  [role="tab"] {
    position: relative;
    padding: var(--space-3) var(--space-2) var(--space-2);
    border: 0;
    background: none;
    font-size: var(--text-md);
    font-weight: 550;
    color: var(--text-3);
    cursor: pointer;
  }
  [role="tab"]:hover {
    color: var(--text-1);
  }
  [role="tab"][aria-selected="true"] {
    color: var(--text-1);
  }
  [role="tab"][aria-selected="true"]::after {
    content: "";
    position: absolute;
    left: var(--space-2);
    right: var(--space-2);
    bottom: -1px;
    height: 2px;
    border-radius: 1px;
    background: var(--amber);
  }

  .tabpanel {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding: var(--space-4);
  }
  .stack {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-3);
  }
  .facts {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: var(--space-3);
    width: 100%;
  }
  .facts .wide {
    grid-column: 1 / -1;
  }
  .facts.compact {
    gap: var(--space-2) var(--space-3);
  }
  dt {
    font-size: var(--text-sm);
    color: var(--text-3);
  }
  dd {
    margin-top: 2px;
    font-size: var(--text-md);
    overflow-wrap: anywhere;
  }

  .kind {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
  }
  .airac {
    font-size: var(--text-sm);
    color: var(--text-3);
  }
  .airac .data {
    color: var(--text-1);
  }
  .filed {
    width: 100%;
    max-height: 132px;
    overflow-y: auto;
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-sm);
    background: var(--ink);
    font-size: var(--text-sm);
    line-height: 1.5;
    word-spacing: 0.2em;
  }
  .explain {
    font-size: var(--text-md);
    color: var(--text-2);
  }
  .legend {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    font-size: var(--text-sm);
    color: var(--text-2);
  }
  .legend li {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }
  .swatch {
    flex: none;
    width: 28px;
    height: 3px;
    border-radius: 2px;
  }
  .swatch.solid {
    background: oklch(0.95 0.01 85 / 0.6);
  }
  .swatch.ahead {
    background: repeating-linear-gradient(to right, var(--amber) 0 6px, transparent 6px 10px);
  }
  .swatch.before {
    background: repeating-linear-gradient(to right, oklch(0.95 0.01 85 / 0.45) 0 4px, transparent 4px 8px);
  }
  .scale {
    margin-left: var(--space-1);
    color: var(--text-3);
  }

  .history ol {
    display: flex;
    flex-direction: column;
  }
  .history .row {
    display: grid;
    grid-template-columns: 74px 1fr auto;
    grid-template-areas:
      "day number legs"
      "times times status";
    gap: 2px var(--space-2);
    width: 100%;
    padding: var(--space-2);
    border: 0;
    border-radius: var(--radius-sm);
    background: none;
    font-size: var(--text-sm);
    text-align: left;
    color: var(--text-1);
  }
  .history button.row:not(:disabled) {
    cursor: pointer;
  }
  .history button.row:not(:disabled):hover {
    background: var(--surface-hover);
  }
  .history .row.current {
    background: var(--amber-soft);
  }
  .day {
    grid-area: day;
    color: var(--text-2);
  }
  .number {
    grid-area: number;
    font-weight: 650;
  }
  .legs {
    grid-area: legs;
  }
  .times {
    grid-area: times;
    color: var(--text-3);
  }
  .status-text {
    grid-area: status;
    color: var(--text-2);
    text-align: right;
  }
  .status-text.good {
    color: var(--live);
  }
  .status-text.warn {
    color: var(--warn);
  }
  .status-text.bad {
    color: var(--danger);
  }
  .placeholder {
    padding: var(--space-2);
  }
  .placeholder .skeleton {
    display: block;
    height: 12px;
  }

  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-top: auto;
    padding: var(--space-3) var(--space-4);
    border-top: 1px solid var(--line);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .status {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-md);
    color: var(--text-2);
  }
  .status.live {
    color: var(--live);
    font-weight: 600;
  }
  .status.ended {
    color: var(--danger);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--live);
    animation: pulse 2s ease-out infinite;
  }
  @keyframes pulse {
    0% {
      box-shadow: 0 0 0 0 oklch(0.8 0.15 152 / 0.6);
    }
    100% {
      box-shadow: 0 0 0 6px oklch(0.8 0.15 152 / 0);
    }
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
</style>
