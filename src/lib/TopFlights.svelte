<script lang="ts">
  import { ArrowRight, Eye, RefreshCw } from "@lucide/svelte";
  import type { TopFlight } from "./api";

  interface Props {
    /** `null` while the first request is in flight. */
    flights: TopFlight[] | null;
    failed: boolean;
    selectedId: number | null;
    onpick: (flight: TopFlight) => void;
    onretry: () => void;
  }

  let { flights, failed, selectedId, onpick, onretry }: Props = $props();

  // group digits only from five up: "1443", "12 345"
  const count = new Intl.NumberFormat(undefined, { useGrouping: "min2" });
</script>

<section aria-labelledby="top-heading">
  <header>
    <h2 id="top-heading">Most tracked</h2>
    <span class="hint" title="People watching each flight on Flightradar24 right now">
      <Eye size={13} strokeWidth={1.75} /> Watching
    </span>
  </header>

  {#if failed && !flights?.length}
    <div class="message">
      <p>Couldn't load the most tracked flights.</p>
      <button class="button" onclick={onretry}><RefreshCw size={14} strokeWidth={1.75} /> Try again</button>
    </div>
  {:else if flights === null}
    <ol aria-busy="true">
      {#each Array(6) as _, i (i)}
        <li class="row placeholder">
          <span class="skeleton" style="width: {58 + ((i * 17) % 30)}px"></span>
          <span class="skeleton" style="width: 34px"></span>
          <span class="skeleton wide" style="width: {120 + ((i * 29) % 70)}px"></span>
        </li>
      {/each}
    </ol>
  {:else}
    <ol>
      {#each flights as f (f.flight_id)}
        <li>
          <button class="row" class:active={f.flight_id === selectedId} onclick={() => onpick(f)}>
            <span class="callsign data">{f.callsign || f.flight_number || "No callsign"}</span>
            <span class="watching data">{count.format(f.live_clicks)}</span>
            <span class="sub">
              {#if f.from_iata || f.to_iata}
                <span class="data">{f.from_iata || "?"}</span>
                <ArrowRight size={11} strokeWidth={2} />
                <span class="data">{f.to_iata || "?"}</span>
              {:else}
                <span>No route filed</span>
              {/if}
              <span class="type">{f.full_description || f.type}</span>
            </span>
          </button>
        </li>
      {/each}
    </ol>
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
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    font-size: var(--text-sm);
    color: var(--text-3);
  }
  .row {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 2px var(--space-2);
    box-sizing: border-box;
    width: 100%;
    padding: var(--space-2);
    border: 0;
    border-radius: var(--radius-sm);
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
  button.row.active .callsign {
    color: var(--amber);
  }
  .callsign {
    font-size: var(--text-md);
    font-weight: 650;
  }
  .watching {
    font-size: var(--text-md);
    color: var(--text-2);
    text-align: right;
  }
  .sub {
    grid-column: 1 / -1;
    display: flex;
    align-items: center;
    gap: var(--space-1);
    min-width: 0;
    font-size: var(--text-sm);
    color: var(--text-2);
  }
  .sub :global(svg) {
    flex: none;
    color: var(--text-3);
  }
  .type {
    margin-left: var(--space-1);
    color: var(--text-3);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
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
</style>
