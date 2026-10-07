<script lang="ts">
  import { LoaderCircle, MapPin, Plane, Search } from "@lucide/svelte";
  import { search, type FindEntry } from "./api";

  interface Props {
    onpick: (entry: FindEntry) => void;
  }

  let { onpick }: Props = $props();

  type Phase = { kind: "idle" } | { kind: "searching" } | { kind: "done"; results: FindEntry[] } | { kind: "error" };

  let input: HTMLInputElement;
  let query = $state("");
  let phase = $state.raw<Phase>({ kind: "idle" });
  let open = $state(false);
  let active = $state(0);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let latest = 0;

  // only these can be shown on the globe
  const selectable = (e: FindEntry) => e.type === "live" || e.type === "airport";
  const results = $derived(phase.kind === "done" ? phase.results : []);

  export function focus() {
    input.focus();
    input.select();
  }

  function oninput() {
    clearTimeout(timer);
    open = true;
    if (query.trim().length < 2) {
      latest++;
      phase = { kind: "idle" };
      return;
    }
    phase = { kind: "searching" };
    timer = setTimeout(run, 250);
  }

  async function run() {
    const id = ++latest;
    try {
      const found = (await search(query.trim())).filter(selectable);
      if (id !== latest) return;
      phase = { kind: "done", results: found };
      active = 0;
    } catch {
      if (id === latest) phase = { kind: "error" };
    }
  }

  function pick(entry: FindEntry) {
    open = false;
    query = "";
    phase = { kind: "idle" };
    input.blur();
    onpick(entry);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "ArrowDown" && results.length) {
      e.preventDefault();
      active = (active + 1) % results.length;
    } else if (e.key === "ArrowUp" && results.length) {
      e.preventDefault();
      active = (active - 1 + results.length) % results.length;
    } else if (e.key === "Enter" && results[active]) {
      pick(results[active]);
    } else if (e.key === "Escape") {
      e.stopPropagation();
      open = false;
      input.blur();
    }
  }

  const detailOf = (e: FindEntry) =>
    e.type === "live" ? (e.detail.route ?? e.detail.ac_type ?? "") : e.type === "airport" ? "Airport" : "";
</script>

<div class="search">
  <label class="field">
    <span class="visually-hidden">Search flights and airports</span>
    <Search size={16} strokeWidth={1.75} />
    <input
      bind:this={input}
      type="search"
      placeholder="Flight, callsign or airport"
      bind:value={query}
      {oninput}
      {onkeydown}
      onfocus={() => (open = true)}
      onblur={() => setTimeout(() => (open = false), 120)}
      spellcheck="false"
      autocomplete="off"
      role="combobox"
      aria-expanded={open && phase.kind !== "idle"}
      aria-controls="search-results"
      aria-activedescendant={results[active] ? `result-${active}` : undefined}
    />
    {#if phase.kind === "searching"}
      <span class="spin" aria-label="Searching"><LoaderCircle size={14} strokeWidth={2} /></span>
    {:else if !query}
      <kbd title="Press / to search">/</kbd>
    {/if}
  </label>

  {#if open && (phase.kind === "done" || phase.kind === "error")}
    <div class="results surface" id="search-results" role="listbox">
      {#if phase.kind === "error"}
        <p class="message">Search failed. Check your connection and try again.</p>
      {:else if results.length === 0}
        <p class="message">No live flights or airports match “{query.trim()}”.</p>
      {:else}
        {#each results as entry, i (entry.type + entry.id)}
          <button
            id="result-{i}"
            role="option"
            aria-selected={i === active}
            class:active={i === active}
            onmouseenter={() => (active = i)}
            onmousedown={(e) => e.preventDefault()}
            onclick={() => pick(entry)}
          >
            <span class="kind {entry.type}">
              {#if entry.type === "live"}<Plane size={14} strokeWidth={1.75} />{:else}<MapPin size={14} strokeWidth={1.75} />{/if}
            </span>
            <span class="text">
              <span class="label">{entry.label}</span>
              {#if detailOf(entry)}<span class="detail">{detailOf(entry)}</span>{/if}
            </span>
          </button>
        {/each}
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
    max-height: 340px;
    overflow-y: auto;
    padding: var(--space-1);
    background: var(--surface-raised);
  }
  .message {
    padding: var(--space-3);
    font-size: var(--text-md);
    color: var(--text-2);
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
  .kind.airport {
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
