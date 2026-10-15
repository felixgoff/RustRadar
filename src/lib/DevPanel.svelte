<!--
  Dev builds only (`tauri dev`): what the app asked of each API and what came
  back, and the selected train as the backend made it. Ctrl/Cmd+Shift+D.
-->
<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { isTauri, type Train, type TrainRoute, type TrainStop } from "./api";

  type Entry = {
    id: number;
    atMs: number;
    source: string;
    method: string;
    url: string;
    request: string | null;
    status: number | null;
    ms: number;
    size: number;
    body: string;
    error: string | null;
  };

  interface Props {
    open: boolean;
    train?: Train | null;
    stops?: TrainStop[] | null;
    route?: TrainRoute | null;
  }
  let { open = $bindable(), train = null, stops = null, route = null }: Props = $props();

  /** Calls kept here, newest last. */
  const KEEP = 400;
  const POLL_MS = 1000;
  let entries = $state.raw<Entry[]>([]);
  let tab = $state<"calls" | "train">("calls");
  let source = $state("all");
  let search = $state("");
  let paused = $state(false);
  let expanded = $state<number | null>(null);

  $effect(() => {
    if (!open || !isTauri()) return;
    let since = entries.at(-1)?.id ?? 0;
    const poll = async () => {
      if (paused) return;
      const fresh = await invoke<Entry[]>("dev_log", { since }).catch(() => []);
      if (!fresh.length) return;
      since = fresh[fresh.length - 1].id;
      entries = [...entries, ...fresh].slice(-KEEP);
    };
    poll();
    const timer = setInterval(poll, POLL_MS);
    return () => clearInterval(timer);
  });

  const sources = $derived([...new Set(entries.map((e) => e.source))].sort());
  const shown = $derived.by(() => {
    const q = search.trim().toLowerCase();
    return entries
      .filter((e) => source === "all" || e.source === source)
      .filter((e) => !q || e.url.toLowerCase().includes(q) || e.body.toLowerCase().includes(q) || e.request?.toLowerCase().includes(q))
      .reverse();
  });

  /** JSON laid out, XML a tag per line; anything else as it came. */
  function pretty(text: string): string {
    const t = text.trim();
    if (t.startsWith("{") || t.startsWith("[")) {
      try {
        return JSON.stringify(JSON.parse(t), null, 2);
      } catch {
        return text;
      }
    }
    if (t.startsWith("<")) return t.replace(/>\s*</g, ">\n<");
    return text;
  }

  const time = (ms: number) => new Date(ms).toLocaleTimeString(undefined, { hour12: false });
  const size = (b: number) => (b >= 1024 * 1024 ? `${(b / 1024 / 1024).toFixed(1)} MB` : b >= 1024 ? `${(b / 1024).toFixed(1)} kB` : `${b} B`);
  const copy = (text: string) => navigator.clipboard.writeText(text).catch(() => {});

  async function clear() {
    entries = [];
    expanded = null;
    await invoke("dev_log_clear").catch(() => {});
  }

  /** The selected train, its stops and its route, as the panel shows them. */
  const trainDump = $derived(
    JSON.stringify(
      {
        train: train && { ...train, frames: `${train.frames.length} frames`, framesList: train.frames },
        stops,
        route: route && {
          complete: route.complete,
          segments: route.segments.map((s) => `${s.length} points, from ${s[0]} to ${s[s.length - 1]}`),
          stops: route.stops,
        },
      },
      null,
      2,
    ),
  );
</script>

{#if open}
  <aside class="dev surface" aria-label="Developer panel">
    <header>
      <strong>Dev</strong>
      <div class="tabs" role="tablist">
        <button role="tab" aria-selected={tab === "calls"} class:on={tab === "calls"} onclick={() => (tab = "calls")}>API calls</button>
        <button role="tab" aria-selected={tab === "train"} class:on={tab === "train"} onclick={() => (tab = "train")}>Selected train</button>
      </div>
      <button class="close" aria-label="Close" onclick={() => (open = false)}>×</button>
    </header>

    {#if tab === "calls"}
      <div class="tools">
        <select bind:value={source} aria-label="Source">
          <option value="all">All sources</option>
          {#each sources as s (s)}<option value={s}>{s}</option>{/each}
        </select>
        <input placeholder="Search URL or data" bind:value={search} spellcheck="false" />
        <button onclick={() => (paused = !paused)}>{paused ? "Resume" : "Pause"}</button>
        <button onclick={clear}>Clear</button>
      </div>
      {#if !isTauri()}
        <p class="empty">Only in the app (`tauri dev`).</p>
      {:else if !shown.length}
        <p class="empty">No calls yet. They show up as the app makes them (dev builds only).</p>
      {/if}
      <ol class="calls">
        {#each shown as e (e.id)}
          <li class:failed={e.error || (e.status ?? 200) >= 400}>
            <button class="row" onclick={() => (expanded = expanded === e.id ? null : e.id)}>
              <span class="t">{time(e.atMs)}</span>
              <span class="src">{e.source}</span>
              <span class="st">{e.error ? "failed" : e.status}</span>
              <span class="sz">{size(e.size)} · {e.ms} ms</span>
              <span class="url" title={e.url}>{e.method} {e.url}</span>
            </button>
            {#if expanded === e.id}
              <div class="detail">
                {#if e.error}<p class="err">{e.error}</p>{/if}
                {#if e.request}
                  <div class="label">Request <button onclick={() => copy(e.request ?? "")}>Copy</button></div>
                  <pre>{pretty(e.request)}</pre>
                {/if}
                <div class="label">Response <button onclick={() => copy(e.body)}>Copy</button></div>
                <pre>{pretty(e.body)}</pre>
              </div>
            {/if}
          </li>
        {/each}
      </ol>
    {:else}
      <div class="tools">
        <button onclick={() => copy(trainDump)}>Copy</button>
        <span class="hint">{train ? train.id : "Click a train on the map"}</span>
      </div>
      <pre class="dump">{trainDump}</pre>
    {/if}
  </aside>
{/if}

<style>
  .dev {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    z-index: 50;
    display: flex;
    flex-direction: column;
    width: min(560px, 100%);
    border-left: 1px solid var(--line);
    font: 12px / 1.4 var(--font-data);
    color: var(--text-1);
  }
  header,
  .tools {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    border-bottom: 1px solid var(--line);
  }
  .tabs {
    display: flex;
    gap: 2px;
    margin-left: 8px;
  }
  button,
  select,
  input {
    font: inherit;
    color: inherit;
    background: var(--surface-raised);
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 2px 8px;
  }
  .tabs button.on {
    background: var(--text-1);
    color: var(--surface);
  }
  .close {
    margin-left: auto;
    font-size: 16px;
    line-height: 1;
  }
  input {
    flex: 1;
    min-width: 0;
  }
  .empty,
  .hint {
    color: var(--text-3);
    padding: 8px;
  }
  .calls {
    flex: 1;
    overflow: auto;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .calls li {
    border-bottom: 1px solid var(--line);
  }
  .row {
    display: grid;
    grid-template-columns: auto auto auto 1fr;
    gap: 2px 8px;
    width: 100%;
    padding: 4px 8px;
    border: 0;
    border-radius: 0;
    background: none;
    text-align: left;
    cursor: pointer;
  }
  .row:hover {
    background: var(--surface-raised);
  }
  .t,
  .sz {
    color: var(--text-3);
  }
  .src {
    font-weight: 600;
  }
  .failed .st,
  .err {
    color: var(--warn);
  }
  .url {
    grid-column: 1 / -1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-2);
  }
  .detail {
    padding: 0 8px 8px;
  }
  .label {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 6px 0 2px;
    color: var(--text-3);
  }
  pre {
    max-height: 50vh;
    overflow: auto;
    margin: 0;
    padding: 6px;
    background: oklch(0.12 0 0);
    border-radius: 4px;
    white-space: pre-wrap;
    word-break: break-all;
    user-select: text;
  }
  .dump {
    flex: 1;
    max-height: none;
    margin: 8px;
  }
</style>
