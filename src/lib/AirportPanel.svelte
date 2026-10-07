<script lang="ts">
  import { untrack } from "svelte";
  import { LocateFixed, PlaneLanding, PlaneTakeoff, RefreshCw, X } from "@lucide/svelte";
  import { airportBoard, type Airport, type BoardMode, type BoardRow } from "./api";
  import { clockAt, dayAt, toneOf, zoneName } from "./time";

  interface Props {
    /** IATA code. */
    code: string;
    /** From the reference data; may still be loading. */
    airport: Airport | undefined;
    selectedId: number | null;
    onselect: (flightId: number) => void;
    onlocate: () => void;
    onclose: () => void;
  }

  let { code, airport, selectedId, onselect, onlocate, onclose }: Props = $props();

  const REFRESH_MS = 60_000;

  let mode = $state<BoardMode>("departures");
  let board = $state.raw<{ key: string; rows: BoardRow[]; page: number; pages: number | null } | null>(null);
  let failed = $state(false);
  let busy = $state(false);
  let updatedAt = $state(0);
  let now = $state(Date.now());

  const key = $derived(`${code}:${mode}`);
  const timeZone = $derived(airport?.timezone);

  /** (Re)loads the first `pages` pages; later pages are kept on refresh. */
  async function load(k: string, pages: number) {
    const [c, m] = k.split(":") as [string, BoardMode];
    busy = true;
    try {
      const results = [];
      for (let p = 1; p <= pages; p++) results.push(await airportBoard(c, m, p));
      if (k !== key) return;
      const seen = new Set<string>();
      const rows = results
        .flatMap((r) => r.rows)
        .filter((r) => {
          const id = `${r.flightId ?? r.number}-${r.scheduled}`;
          return !seen.has(id) && !!seen.add(id);
        });
      board = { key: k, rows, page: pages, pages: results[0]?.pages ?? null };
      failed = false;
      updatedAt = Date.now();
    } catch {
      if (k === key) failed = true;
    } finally {
      busy = false;
    }
  }

  $effect(() => {
    const k = key;
    untrack(() => {
      board = null;
      failed = false;
      load(k, 1);
    });
    const timer = setInterval(() => load(k, untrack(() => board?.page ?? 1)), REFRESH_MS);
    const clock = setInterval(() => (now = Date.now()), 15_000);
    return () => {
      clearInterval(timer);
      clearInterval(clock);
    };
  });

  const rows = $derived(board?.key === key ? board.rows : null);
  const more = $derived(!!board && board.pages !== null && board.page < board.pages);

  function tabKeys(e: KeyboardEvent) {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    e.preventDefault();
    mode = mode === "departures" ? "arrivals" : "departures";
    document.getElementById(`board-${mode}`)?.focus();
  }
</script>

<aside class="panel surface" aria-label="{airport?.name ?? code} departures and arrivals">
  <header class="strip">
    <div class="cell id">
      <h2 class="data">{code}</h2>
      <p class="data">{airport?.icao ?? ""}</p>
    </div>
    <div class="cell name">
      <span class="value" title={airport?.name}>{airport?.name ?? "Airport"}</span>
      <span class="sub">{[airport?.city, airport?.country].filter(Boolean).join(", ")}</span>
    </div>
    <div class="cell">
      <span class="data value">{clockAt(now / 1000, timeZone)}</span>
      <span class="data sub">{timeZone ? zoneName(now / 1000, timeZone) : "local"}</span>
    </div>
    <button class="icon-button close" aria-label="Close airport" title="Close (Esc)" onclick={onclose}>
      <X size={18} strokeWidth={1.75} />
    </button>
  </header>

  <div class="tabs" role="tablist" aria-label="Board">
    {#each [["departures", "Departures", PlaneTakeoff], ["arrivals", "Arrivals", PlaneLanding]] as const as [id, label, Icon] (id)}
      <button
        id="board-{id}"
        role="tab"
        aria-selected={mode === id}
        aria-controls="board"
        tabindex={mode === id ? 0 : -1}
        onclick={() => (mode = id)}
        onkeydown={tabKeys}
      >
        <Icon size={15} strokeWidth={1.75} />
        {label}
      </button>
    {/each}
  </div>

  <div class="board" id="board" role="tabpanel" aria-labelledby="board-{mode}" aria-busy={busy}>
    {#if failed && !rows}
      <div class="message">
        <p>Couldn't load the {mode} board.</p>
        <button class="button" onclick={() => load(key, 1)}><RefreshCw size={14} strokeWidth={1.75} /> Try again</button>
      </div>
    {:else if !rows}
      <ol>
        {#each Array(7) as _, i (i)}
          <li class="row placeholder">
            <span class="skeleton" style="width: 38px"></span>
            <span class="skeleton" style="width: {90 + ((i * 31) % 60)}px"></span>
            <span class="skeleton" style="width: {60 + ((i * 17) % 30)}px"></span>
          </li>
        {/each}
      </ol>
    {:else if rows.length === 0}
      <p class="message">No {mode} listed right now.</p>
    {:else}
      <ol>
        {#each rows as r, i (`${r.flightId ?? r.number}-${r.scheduled}`)}
          {@const day = r.scheduled ? dayAt(r.scheduled, timeZone, now) : ""}
          {@const tone = toneOf(r.statusColor)}
          {#if i === 0 ? day !== "Today" : day !== (rows[i - 1].scheduled ? dayAt(rows[i - 1].scheduled!, timeZone, now) : "")}
            <li class="day">{day}</li>
          {/if}
          <li>
            <button
              class="row"
              class:selected={r.flightId !== null && r.flightId === selectedId}
              disabled={!r.live || !r.flightId}
              title={r.live ? "Show this flight" : "Not airborne or not tracked right now"}
              onclick={() => r.flightId && onselect(r.flightId)}
            >
              <span class="time data">{clockAt(r.scheduled, timeZone)}</span>
              <span class="flight">
                <span class="number data">{r.number || r.callsign || "—"}</span>
                <span class="airline">{r.airline ?? ""}</span>
              </span>
              <span class="status" class:good={tone === "good"} class:warn={tone === "warn"} class:bad={tone === "bad"}>
                {#if r.live}<span class="live-dot" aria-label="Live"></span>{/if}
                {r.status ?? ""}
              </span>
              <span class="place">
                {mode === "departures" ? "To" : "From"}
                {r.airportCity || r.airportName || "?"}
                {#if r.airportIata}<span class="data">({r.airportIata})</span>{/if}
              </span>
              <span class="extra data">
                {[r.aircraft, r.registration, r.terminal && `T${r.terminal}`, r.gate && `Gate ${r.gate}`]
                  .filter(Boolean)
                  .join(" · ")}
              </span>
            </button>
          </li>
        {/each}
      </ol>
      {#if more}
        <button class="button more" disabled={busy} onclick={() => board && load(key, board.page + 1)}>
          Show more {mode}
        </button>
      {/if}
    {/if}
  </div>

  <footer>
    <span class="updated">
      {#if updatedAt}Updated {clockAt(updatedAt / 1000, timeZone)} · times at {code}{/if}
    </span>
    <button class="button" onclick={onlocate} disabled={!airport}>
      <LocateFixed size={14} strokeWidth={1.75} /> Show on map
    </button>
  </footer>
</aside>

<style>
  .panel {
    position: absolute;
    top: var(--space-4);
    right: var(--space-4);
    width: 360px;
    max-height: calc(100% - var(--space-4) * 2);
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

  .strip {
    display: grid;
    grid-template-columns: auto 1fr auto 40px;
    border-bottom: 1px solid var(--line);
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
  h2 {
    font-size: var(--text-xl);
    font-weight: 700;
    line-height: 1.1;
  }
  .cell p,
  .sub {
    font-size: var(--text-sm);
    color: var(--text-2);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .value {
    font-size: var(--text-md);
    font-weight: 650;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .close {
    align-self: center;
    justify-self: center;
  }

  .tabs {
    display: flex;
    gap: var(--space-1);
    padding: 0 var(--space-3);
    border-bottom: 1px solid var(--line);
  }
  [role="tab"] {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-3) var(--space-2) var(--space-2);
    border: 0;
    background: none;
    font-size: var(--text-md);
    font-weight: 550;
    color: var(--text-3);
    cursor: pointer;
  }
  [role="tab"]:hover,
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

  .board {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: var(--space-2);
    transition: opacity 150ms var(--ease-out);
  }
  .board[aria-busy="true"] {
    opacity: 0.75;
  }
  .day {
    padding: var(--space-3) var(--space-2) var(--space-1);
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-2);
  }
  .row {
    display: grid;
    grid-template-columns: 44px 1fr auto;
    grid-template-areas:
      "time flight status"
      ". place place"
      ". extra extra";
    gap: 1px var(--space-2);
    width: 100%;
    padding: var(--space-2);
    border: 0;
    border-radius: var(--radius-sm);
    background: none;
    font-size: var(--text-sm);
    text-align: left;
    color: var(--text-1);
  }
  button.row:not(:disabled) {
    cursor: pointer;
  }
  button.row:not(:disabled):hover {
    background: var(--surface-hover);
  }
  .row.selected {
    background: var(--amber-soft);
  }
  .time {
    grid-area: time;
    font-size: var(--text-md);
    font-weight: 650;
  }
  .flight {
    grid-area: flight;
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    min-width: 0;
  }
  .number {
    font-size: var(--text-md);
    font-weight: 650;
    white-space: nowrap;
  }
  .airline {
    color: var(--text-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .status {
    grid-area: status;
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    color: var(--text-2);
    white-space: nowrap;
  }
  .status.good {
    color: var(--live);
  }
  .status.warn {
    color: var(--warn);
  }
  .status.bad {
    color: var(--danger);
  }
  .live-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--live);
  }
  .place {
    grid-area: place;
    color: var(--text-2);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .extra {
    grid-area: extra;
    font-size: var(--text-xs);
    color: var(--text-3);
  }
  .extra:empty {
    display: none;
  }
  .placeholder {
    display: flex;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-2);
  }
  .placeholder .skeleton {
    height: 12px;
  }
  .message {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-2);
    font-size: var(--text-md);
    color: var(--text-2);
  }
  .more {
    margin: var(--space-2);
  }

  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    border-top: 1px solid var(--line);
  }
  .updated {
    font-size: var(--text-xs);
    color: var(--text-3);
  }
</style>
