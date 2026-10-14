<script lang="ts">
  import { ExternalLink, LocateFixed, X } from "@lucide/svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import type { Train } from "./api";
  import { degrees } from "./aircraft-readings";
  import { latLon } from "./ship-details";
  import { clockWithZone } from "./time";
  import {
    delayText,
    delayTone,
    headingAt,
    positionAt,
    TRAIN_CREDITS,
    TRAIN_SOURCE_NAMES,
    trainColor,
    trainLabel,
    trainPositionAge,
  } from "./trains";

  interface Props {
    /** The train as the map last showed it: its report follows the map's refreshes. */
    train: Train;
    /** The clock the report's age is measured against (Unix ms), ticking. */
    now: number;
    /** Fly the camera to the train. */
    oncentre: () => void;
    onclose: () => void;
  }

  let { train, now, oncentre, onclose }: Props = $props();

  const kind = $derived(trainLabel(train.kind));
  /** Names run long ("Amtrak 171 Northeast Regional"): those wrap to a second line instead of losing their end. */
  const long = $derived(train.name.length > 16);
  const swatch = $derived(`rgb(${trainColor(train.kind).join(", ")})`);
  const heading = $derived(headingAt(train, now));
  const moving = $derived(train.speedKmh !== null || heading !== null);
  const delay = $derived(delayText(train.delayMinutes));
  const tone = $derived(delayTone(train.delayMinutes));
  const credit = $derived(TRAIN_CREDITS[train.source]);
  const sourceName = $derived(TRAIN_SOURCE_NAMES[train.source] ?? train.source);
  /** Where it is drawn now: along its expected positions when it has them. */
  const position = $derived(positionAt(train, now));
  const age = $derived(trainPositionAge(train.positionUnix, now));
  const reported = $derived(clockWithZone(train.positionUnix));
</script>

<aside class="panel surface" aria-label="Selected train">
  <header class="strip" class:bare={!moving}>
    <div class="cell id">
      <h2 class:long title={train.name}>{train.name}</h2>
      <p><span class="swatch" style:background={swatch} aria-hidden="true"></span>{kind} train</p>
    </div>
    {#if moving}
      <div class="cell" title="Speed and direction of travel">
        <span class="data value speed">{train.speedKmh !== null ? `${Math.round(train.speedKmh)} km/h` : "—"}</span>
        <span class="data sub">{heading !== null ? degrees(heading) : ""}</span>
      </div>
    {/if}
    <button class="icon-button close" aria-label="Close train details" title="Close (Esc)" onclick={onclose}>
      <X size={18} strokeWidth={1.75} />
    </button>
  </header>

  {#if train.destination || delay}
    <section class="voyage" aria-label="Journey">
      <div>
        <span class="label">Destination</span>
        {#if train.destination}
          <span class="code" class:free={train.destination.length > 14} title={train.destination}>{train.destination}</span>
        {:else}
          <span class="unknown">Not given</span>
        {/if}
      </div>
      {#if delay}
        <div class="end" title="At the last or next stop, as the operator reports it">
          <span class="label">Running</span>
          <span class="badge {tone ?? 'neutral'}">{delay.charAt(0).toUpperCase()}{delay.slice(1)}</span>
        </div>
      {/if}
    </section>
  {/if}

  <section class="body" aria-label="Train">
    <dl class="facts">
      <div>
        <dt>Operator</dt>
        <dd>{train.operator ?? "Not given"}</dd>
      </div>
      <div>
        <dt>Source</dt>
        <dd>{sourceName}</dd>
      </div>
      <div class="wide" title={train.frames.length > 1 ? "Moving along the source's expected positions" : "As last reported"}>
        <dt>Position</dt>
        <dd class="data">{latLon(position.lat, position.lon)}</dd>
      </div>
    </dl>
    {#if credit}
      <p class="note">
        <a href={credit.url} onclick={(e) => (e.preventDefault(), openUrl(credit.url))}>{credit.text}</a>
      </p>
    {/if}
  </section>

  <footer>
    <div class="report">
      {#if age.stale}
        <span class="badge warn" title="Reported at {reported}">{age.label}</span>
      {:else}
        <span class="status" title="Reported at {reported}">{age.label}</span>
      {/if}
    </div>
    <div class="actions">
      <button class="button compact" aria-label="Centre on the train" title="Centre the map on this train" onclick={oncentre}>
        <LocateFixed size={14} strokeWidth={1.75} />
      </button>
      {#if credit}
        <button class="button" onclick={() => openUrl(credit.url)} title={credit.url}>
          {train.source === "amtrak" ? "Amtraker" : sourceName.split(" (")[0]} <ExternalLink size={14} strokeWidth={1.75} />
        </button>
      {/if}
    </div>
  </footer>
</aside>

<style>
  /* placed like the ship and flight panels: anchored to the bottom, clear of the attribution line, growing upwards */
  .panel {
    position: absolute;
    bottom: 28px;
    right: var(--space-4);
    width: 360px;
    max-height: calc(100% - var(--space-4) - 28px);
    overflow-x: hidden;
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

  /* the ship panel's strip: name and kind, then speed and direction */
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
  .strip h2.long {
    font-size: var(--text-lg);
    line-height: 1.25;
    white-space: normal;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .strip p,
  .sub {
    font-size: var(--text-sm);
    color: var(--text-2);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* the train's colour on the map, as its key */
  .swatch {
    display: inline-block;
    width: 8px;
    height: 8px;
    margin-right: 6px;
    border-radius: 2px;
    vertical-align: 0;
  }
  .value {
    font-size: var(--text-base);
    font-weight: 650;
    white-space: nowrap;
  }
  .speed {
    color: var(--amber);
  }
  .close {
    align-self: center;
    justify-self: center;
  }

  .voyage {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: var(--space-3);
    padding: var(--space-4);
    border-bottom: 1px solid var(--line);
  }
  .voyage > div {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
    min-width: 0;
  }
  .voyage .end {
    align-items: flex-end;
  }
  .voyage .badge {
    font-size: var(--text-sm);
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
  .code.free {
    font-size: var(--text-base);
    line-height: 1.5;
  }
  .unknown {
    font-size: var(--text-sm);
    color: var(--text-2);
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-4);
  }
  .facts {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
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
  .note a {
    color: inherit;
    text-decoration: underline;
    text-decoration-color: var(--line);
    text-underline-offset: 2px;
  }
  .note a:hover {
    color: var(--text-1);
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
  .status {
    font-size: var(--text-md);
    font-variant-numeric: tabular-nums;
    color: var(--text-2);
  }
  .report .badge {
    font-size: var(--text-sm);
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
</style>
