<script lang="ts">
  import { ExternalLink, LocateFixed, RefreshCw, X } from "@lucide/svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { shipPageUrl, shipPhotoUrl, type Ship, type ShipDetails } from "./api";
  import { degrees } from "./aircraft-readings";
  import { shipKindLabel } from "./ships";
  import { dimensions, draught, isLocode, latLon, navTone, positionAge } from "./ship-details";
  import { clockWithZone, dayAt } from "./time";

  interface Props {
    /** The ship as the map last showed it: its position follows the map's refreshes. */
    ship: Ship;
    /** VesselFinder's card for it; null while loading or after a failure. */
    details: ShipDetails | null;
    /** Why the card didn't load. */
    error?: string | null;
    /** The clock the report's age is measured against (Unix ms), ticking. */
    now: number;
    onretry: () => void;
    /** Whether the camera follows the ship. */
    following: boolean;
    /** Follow it with the camera, or stop. */
    onfollow: () => void;
    onclose: () => void;
  }

  let { ship, details, error = null, now, onretry, following, onfollow, onclose }: Props = $props();

  const loading = $derived(!details && !error);
  const name = $derived(details?.name || (ship.name !== String(ship.mmsi) ? ship.name : "") || "Unnamed ship");
  /** VesselFinder's own wording, else the map's colour class (none for "unknown"). */
  const typeLabel = $derived(details?.typeLabel || (ship.kind ? shipKindLabel(ship.kind) : null));
  const subtitle = $derived([typeLabel, details?.flag].filter(Boolean).join(" · "));
  const moving = $derived(details?.speedKn != null || details?.courseDeg != null);
  const pageUrl = $derived(shipPageUrl(ship.mmsi));

  /** The photo, unless it failed to load. */
  let failedPhoto = $state<string | null>(null);
  const photo = $derived(details?.photoId && details.photoId !== failedPhoto ? details.photoId : null);

  const status = $derived(details?.navStatus ?? null);
  const destination = $derived(details?.destination ?? null);
  const eta = $derived(details?.etaUnix ?? null);
  /** An ETA the ship has already passed: the crew hasn't updated it. */
  const etaPassed = $derived(eta !== null && eta * 1000 < now);

  /** What on show the crew typed in, so it isn't read as measured: "Status and destination are". */
  const crewEntered = $derived.by(() => {
    const items = [status && "status", destination && "destination", eta && "ETA"].filter(Boolean) as string[];
    if (!items.length) return "";
    const list = items.length > 1 ? `${items.slice(0, -1).join(", ")} and ${items.at(-1)}` : items[0];
    return `${list.charAt(0).toUpperCase()}${list.slice(1)} ${items.length > 1 ? "are" : "is"}`;
  });

  const age = $derived(details?.positionUnix ? positionAge(details.positionUnix, now) : null);
  const count = (n: number) => n.toLocaleString("en-GB");

  type Fact = { label: string; value: string; data?: boolean; title?: string; wide?: boolean };
  const facts = $derived.by<Fact[]>(() => {
    const d = details;
    const list: Fact[] = [];
    if (d?.imo) list.push({ label: "IMO", value: String(d.imo), data: true });
    list.push({ label: "MMSI", value: String(ship.mmsi), data: true });
    if (d?.flag) list.push({ label: "Flag", value: d.flag, title: d.flagCode ?? undefined });
    const size = d ? dimensions(d.lengthM, d.beamM) : null;
    if (size) list.push({ label: "Length × beam", value: size, data: true });
    const depth = d ? draught(d.draughtM, d.maxDraughtM) : null;
    if (depth) {
      list.push({
        label: d?.draughtM && d.maxDraughtM ? "Draught / max" : "Draught",
        value: depth,
        data: true,
        title: "The current draught as entered by the crew, and the ship's maximum",
      });
    }
    if (d?.yearBuilt) list.push({ label: "Built", value: String(d.yearBuilt), data: true });
    if (d?.grossTonnage) list.push({ label: "Gross tonnage", value: count(d.grossTonnage), data: true });
    if (d?.deadweight) list.push({ label: "Deadweight", value: `${count(d.deadweight)} t`, data: true });
    list.push({ label: "Position", value: latLon(ship.lat, ship.lon), data: true, wide: true, title: "As the map last showed it" });
    return list;
  });
</script>

<aside class="panel surface" aria-label="Selected ship">
  <header class="strip" class:bare={!moving && !loading}>
    <div class="cell id">
      <h2 class="data" class:long={name.length > 13} title={name}>{name}</h2>
      {#if subtitle}<p title={subtitle}>{subtitle}</p>{:else if loading}<p><span class="skeleton line short"></span></p>{/if}
    </div>
    {#if moving}
      <div class="cell" title="Speed and course over ground">
        <span class="data value speed">{details?.speedKn != null ? `${details.speedKn.toFixed(1)} kn` : "—"}</span>
        <span class="data sub">{details?.courseDeg != null ? degrees(details.courseDeg) : ""}</span>
      </div>
    {:else if loading}
      <div class="cell" aria-hidden="true">
        <span class="skeleton line value-line"></span>
        <span class="skeleton line sub-line"></span>
      </div>
    {/if}
    <button class="icon-button close" aria-label="Close ship details" title="Close (Esc)" onclick={onclose}>
      <X size={18} strokeWidth={1.75} />
    </button>
  </header>

  {#if loading}
    <div class="photo skeleton"></div>
  {:else if photo}
    <div class="photo">
      <button class="image" onclick={() => openUrl(pageUrl)} title="View this ship on VesselFinder">
        <img src={shipPhotoUrl(photo)} alt={name} onerror={() => (failedPhoto = photo)} />
      </button>
      <span class="credit">Photo: VesselFinder</span>
    </div>
  {/if}

  {#if loading}
    <section class="voyage" aria-busy="true" aria-label="Voyage">
      <div class="legs">
        <div><span class="skeleton line code-line"></span></div>
        <div class="end"><span class="skeleton line code-line"></span></div>
      </div>
      <span class="skeleton line"></span>
    </section>
  {:else if destination || eta || status || details?.sar}
    <section class="voyage" aria-label="Voyage">
      {#if destination || eta}
        <div class="legs">
          <div>
            <span class="label">Destination</span>
            {#if destination}
              <span
                class="code data"
                class:free={!isLocode(destination)}
                title={isLocode(destination) ? `UN/LOCODE ${destination}, as entered by the crew` : "As entered by the crew"}
              >
                {destination}
              </span>
            {:else}
              <span class="unknown">Not given</span>
            {/if}
          </div>
          {#if eta}
            <!-- the port's time zone isn't known: UTC, named -->
            <div
              class="end"
              class:dim={etaPassed}
              title={etaPassed
                ? "This ETA has passed; the crew hasn't updated it"
                : "As entered by the crew; the port's time zone isn't known, so in UTC"}
            >
              <span class="label">ETA{etaPassed ? ", passed" : ""}</span>
              <span class="code data">{clockWithZone(eta, "UTC")}</span>
              <span class="when">{dayAt(eta, "UTC", now)}</span>
            </div>
          {/if}
        </div>
      {/if}
      {#if status || details?.sar}
        <div class="badges">
          {#if status}
            <span class="badge {navTone(status)}" title="AIS navigational status, as set by the crew">{status.label}</span>
          {/if}
          {#if details?.sar}<span class="badge neutral">Search and rescue aircraft</span>{/if}
        </div>
      {/if}
    </section>
  {/if}

  <section class="body" aria-label="Ship">
    {#if error}
      <div class="stack">
        <p class="explain">Couldn't load this ship's details from VesselFinder.</p>
        <p class="note">{error}</p>
        <button class="button" onclick={onretry}><RefreshCw size={14} strokeWidth={1.75} /> Try again</button>
      </div>
    {/if}
    <dl class="facts">
      {#if loading}
        {#each Array(6) as _, i (i)}
          <div aria-hidden="true">
            <dt><span class="skeleton line label-line"></span></dt>
            <dd><span class="skeleton line" style="width: {44 + ((i * 23) % 40)}px"></span></dd>
          </div>
        {/each}
      {:else}
        {#each facts as f (f.label)}
          <div class:wide={f.wide} title={f.title}>
            <dt>{f.label}</dt>
            <dd class:data={f.data}>{f.value}</dd>
          </div>
        {/each}
      {/if}
    </dl>
    {#if details}
      <div class="notes">
        {#if crewEntered}<p class="note">{crewEntered} entered by the crew.</p>{/if}
        <p class="note">Data: VesselFinder</p>
      </div>
    {/if}
  </section>

  <footer>
    <div class="report">
      {#if age}
        {#if age.stale}
          <span class="badge warn" title="Reported at {clockWithZone(details?.positionUnix)}">{age.label}</span>
        {:else}
          <span class="status" title="Reported at {clockWithZone(details?.positionUnix)}">{age.label}</span>
        {/if}
        {#if details?.satellite}<span class="via">via satellite AIS</span>{/if}
      {:else if loading}
        <span class="skeleton line"></span>
      {:else if ship.old}
        <span class="badge warn">Old position</span>
      {/if}
    </div>
    <div class="actions">
      <button
        class="button"
        aria-pressed={following}
        title={following ? "Stop following (C)" : "Follow with the camera (C)"}
        onclick={onfollow}
      >
        <LocateFixed size={14} strokeWidth={1.75} /> Follow
      </button>
      <button class="button" onclick={() => openUrl(pageUrl)}>
        VesselFinder <ExternalLink size={14} strokeWidth={1.75} />
      </button>
    </div>
  </footer>
</aside>

<style>
  /* placed like the flight panel: anchored to the bottom, clear of the attribution line, growing upwards */
  .panel {
    position: absolute;
    bottom: 28px;
    right: var(--space-4);
    width: 360px;
    max-height: calc(100% - var(--space-4) - 28px);
    overflow-x: hidden;
    overflow-y: auto;
    scroll-padding-bottom: calc(57px + var(--space-4));
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

  /* the flight panel's strip: name and type, then speed and course */
  .strip {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto 40px;
    border-bottom: 1px solid var(--line);
  }
  .strip.bare {
    grid-template-columns: minmax(0, 1fr) 40px;
  }
  .cell {
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 2px;
    padding: var(--space-3);
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
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* ship names run longer than callsigns: a step down keeps most of them whole */
  .strip h2.long {
    font-size: var(--text-lg);
    line-height: 1.3;
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
  .speed {
    color: var(--amber);
  }
  .close {
    align-self: center;
    justify-self: center;
  }

  .photo {
    position: relative;
    flex: none;
    aspect-ratio: 2 / 1;
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
  .credit {
    position: absolute;
    right: var(--space-2);
    bottom: var(--space-1);
    font-size: var(--text-xs);
    color: var(--text-1);
    text-shadow: 0 1px 2px oklch(0 0 0 / 0.9);
    pointer-events: none;
  }

  /* the flight panel's route block, for the voyage: where to, and when */
  .voyage {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-4);
    border-bottom: 1px solid var(--line);
  }
  .legs {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: var(--space-3);
  }
  .legs > div {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    min-width: 0;
  }
  .legs .end {
    align-items: flex-end;
    text-align: right;
  }
  .label {
    font-size: var(--text-sm);
    color: var(--text-3);
  }
  .code {
    max-width: 100%;
    font-size: var(--text-xl);
    font-weight: 700;
    line-height: 1.1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* the crew's free text, not a port code: a size that fits more of it */
  .code.free {
    font-size: var(--text-base);
    line-height: 1.5;
  }
  .end .code {
    font-size: var(--text-base);
    line-height: 1.5;
  }
  .when,
  .unknown {
    font-size: var(--text-sm);
    color: var(--text-2);
  }
  .dim .code,
  .dim .when {
    color: var(--text-3);
  }
  .badges {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-4);
  }
  .stack {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-2);
  }
  .explain {
    font-size: var(--text-md);
    color: var(--text-2);
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
  dt {
    font-size: var(--text-sm);
    color: var(--text-3);
  }
  dd {
    margin-top: 2px;
    font-size: var(--text-md);
    overflow-wrap: anywhere;
  }
  .note {
    font-size: var(--text-xs);
    color: var(--text-3);
  }
  .notes {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .skeleton.line {
    display: block;
    height: 12px;
    width: 70%;
  }
  .skeleton.short {
    width: 140px;
  }
  .skeleton.value-line {
    width: 56px;
    height: 14px;
  }
  .skeleton.sub-line {
    width: 36px;
  }
  .skeleton.code-line {
    width: 84px;
    height: 22px;
  }
  .skeleton.label-line {
    width: 52px;
    height: 10px;
    margin-bottom: 4px;
  }

  footer {
    position: sticky;
    bottom: 0;
    z-index: 1;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    margin-top: auto;
    padding: var(--space-3) var(--space-4);
    border-top: 1px solid var(--line);
    background: var(--surface);
  }
  .report {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }
  .report .skeleton.line {
    width: 120px;
  }
  .status {
    font-size: var(--text-md);
    font-variant-numeric: tabular-nums;
    color: var(--text-2);
  }
  /* stale: the status's own size, so the escalation reads as the same line raised */
  .report .badge {
    font-size: var(--text-sm);
  }
  .via {
    font-size: var(--text-xs);
    color: var(--text-3);
  }
  .actions {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .button.compact {
    justify-content: center;
    width: 32px;
    padding: 0;
  }
  /* Phones (the query in phone.svelte.ts): a sheet along the bottom */
  @media (max-width: 899px), (max-height: 500px) {
    .panel {
      position: fixed;
      left: 0;
      right: 0;
      bottom: 0;
      z-index: 5;
      width: auto;
      max-height: 72dvh;
      border-bottom: 0;
      border-left: 0;
      border-right: 0;
      border-radius: 12px 12px 0 0;
      padding-bottom: env(safe-area-inset-bottom, 0px);
      padding-left: env(safe-area-inset-left, 0px);
      padding-right: env(safe-area-inset-right, 0px);
      animation-name: rise;
    }
    @keyframes rise {
      from {
        transform: translateY(24px);
        opacity: 0;
      }
    }
  }
  /* a phone on its side: down the right, beside the list */
  @media (max-height: 500px) and (orientation: landscape) {
    .panel {
      top: 0;
      left: auto;
      width: min(360px, 50vw);
      max-height: none;
      border-radius: 12px 0 0 0;
    }
  }
</style>
