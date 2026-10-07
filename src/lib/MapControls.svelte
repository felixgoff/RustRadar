<script lang="ts">
  import { CircleUserRound, Keyboard, Layers, LoaderCircle, Minus, Navigation2, Plus, Rotate3d } from "@lucide/svelte";
  import type { SessionInfo } from "./api";
  import type { Basemap } from "./geo";

  interface Props {
    zoom: number;
    bearing: number;
    pitch: number;
    basemap: Basemap;
    labels: boolean;
    buildings: boolean;
    liveries: boolean;
    daylight: boolean;
    weather: boolean;
    exaggeration: number;
    session: SessionInfo | null;
    /** Extra space on the right, e.g. for an open side panel. */
    inset?: number;
    onzoom: (delta: number) => void;
    onreset: () => void;
    ontilt: (pitch: number) => void;
    onsignin: (email: string, password: string) => Promise<void>;
    onsignout: () => Promise<void>;
    onhelp: () => void;
  }

  let {
    zoom,
    bearing,
    pitch,
    basemap = $bindable(),
    labels = $bindable(),
    buildings = $bindable(),
    liveries = $bindable(),
    daylight = $bindable(),
    weather = $bindable(),
    exaggeration = $bindable(),
    session,
    inset = 0,
    onzoom,
    onreset,
    ontilt,
    onsignin,
    onsignout,
    onhelp,
  }: Props = $props();

  const TILT = 55;
  let open = $state<"layers" | "account" | null>(null);
  let root: HTMLDivElement;

  let email = $state("");
  let password = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);

  const tilted = $derived(pitch > 5);
  const rotated = $derived(Math.abs(bearing) > 0.5 || tilted);
  const signedIn = $derived(session?.authenticated ?? false);

  /** Opens the account popover, e.g. from a "sign in" prompt elsewhere. */
  export function openAccount() {
    open = "account";
    error = null;
  }

  function onpointerdown(e: PointerEvent) {
    if (open && !root.contains(e.target as Node)) open = null;
  }

  // in the capture phase, so Escape closes the popover before it closes anything else
  function onkeydowncapture(e: KeyboardEvent) {
    if (e.key !== "Escape" || !open) return;
    e.stopPropagation();
    open = null;
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    error = null;
    try {
      await onsignin(email, password);
      password = "";
    } catch (err) {
      error = String(err);
    } finally {
      busy = false;
    }
  }

  async function signOut() {
    busy = true;
    try {
      await onsignout();
    } finally {
      busy = false;
    }
  }

  const until = (unix: number) =>
    new Date(unix * 1000).toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" });
</script>

<svelte:window {onpointerdown} {onkeydowncapture} />

<div class="controls" bind:this={root} style:--inset="{inset}px">
  {#if open === "layers"}
    <section class="popover surface" aria-label="Map layer settings">
      <fieldset class="segmented">
        <legend>Map</legend>
        {#each [["dark", "Dark"], ["satellite", "Satellite"]] as const as [value, label]}
          <label>
            <input type="radio" name="basemap" {value} bind:group={basemap} />
            <span>{label}</span>
          </label>
        {/each}
      </fieldset>

      <div class="group">
        <label class="switch">
          <span>Place names</span>
          <input type="checkbox" role="switch" bind:checked={labels} />
        </label>
        <label class="switch">
          <span>
            3D buildings
            <small>Appear when you zoom in to street level</small>
          </span>
          <input type="checkbox" role="switch" bind:checked={buildings} />
        </label>
        <label class="switch">
          <span>
            Airline liveries
            <small>Aircraft take their airline's colours up close</small>
          </span>
          <input type="checkbox" role="switch" bind:checked={liveries} />
        </label>
        <label class="switch">
          <span>
            Day and night
            <small>Shades where the sun has set, with twilight</small>
          </span>
          <input type="checkbox" role="switch" bind:checked={daylight} />
        </label>
        <label class="switch">
          <span>
            Weather radar
            <small>Rain and snow from RainViewer, updated every 10 min</small>
          </span>
          <input type="checkbox" role="switch" bind:checked={weather} />
        </label>
      </div>

      <label class="range">
        <span>Altitude exaggeration <output class="data">×{exaggeration}</output></span>
        <input type="range" min="1" max="40" step="1" bind:value={exaggeration} />
        <small>Applies when zoomed out; true scale up close</small>
      </label>
    </section>
  {:else if open === "account"}
    <section class="popover surface account" aria-labelledby="account-heading">
      <h2 id="account-heading">Flightradar24 account</h2>
      {#if signedIn}
        <p class="state"><span class="badge good">Signed in</span></p>
        <p class="note">
          Data your subscription includes, such as filed flight plans, now shows up.{session?.expires
            ? ` The session lasts until ${until(session.expires)}.`
            : ""}
        </p>
        <button class="button" onclick={signOut} disabled={busy}>Sign out</button>
      {:else}
        <p class="note">
          Optional. A subscription unlocks filed flight plans (the real route), vertical speed, squawk and airspace,
          depending on your plan.
        </p>
        <form onsubmit={submit}>
          <label class="field">
            <span>Email</span>
            <input type="email" autocomplete="username" required bind:value={email} disabled={busy} />
          </label>
          <label class="field">
            <span>Password</span>
            <input type="password" autocomplete="current-password" required bind:value={password} disabled={busy} />
          </label>
          {#if error}<p class="error" role="alert">{error}</p>{/if}
          <button class="button primary" type="submit" disabled={busy || !email || !password}>
            {#if busy}<span class="spin"><LoaderCircle size={14} strokeWidth={2} /></span>{/if}
            Sign in
          </button>
        </form>
        <p class="fine">
          Sent only to Flightradar24. RustRadar keeps the session in memory and forgets it when you quit.
        </p>
      {/if}
    </section>
  {/if}

  <div class="stack surface">
    <button
      class="icon-button"
      aria-label="Map layers"
      aria-expanded={open === "layers"}
      aria-pressed={open === "layers"}
      title="Map layers"
      onclick={() => (open = open === "layers" ? null : "layers")}
    >
      <Layers size={18} strokeWidth={1.75} />
    </button>
    <button
      class="icon-button"
      class:signed-in={signedIn}
      aria-label="Flightradar24 account"
      aria-expanded={open === "account"}
      aria-pressed={open === "account"}
      title={signedIn ? "Signed in to Flightradar24" : "Sign in to Flightradar24"}
      onclick={() => (open === "account" ? (open = null) : openAccount())}
    >
      <CircleUserRound size={18} strokeWidth={1.75} />
    </button>
    <button class="icon-button" aria-label="Keyboard shortcuts" title="Keyboard shortcuts (?)" onclick={onhelp}>
      <Keyboard size={18} strokeWidth={1.75} />
    </button>
  </div>

  <div class="stack surface">
    <button class="icon-button" aria-label="Zoom in" title="Zoom in (+)" disabled={zoom >= 18} onclick={() => onzoom(1)}>
      <Plus size={18} strokeWidth={1.75} />
    </button>
    <button class="icon-button" aria-label="Zoom out" title="Zoom out (−)" disabled={zoom <= 0.5} onclick={() => onzoom(-1)}>
      <Minus size={18} strokeWidth={1.75} />
    </button>
  </div>

  <div class="stack surface">
    <button
      class="icon-button"
      aria-pressed={tilted}
      aria-label={tilted ? "Look straight down" : "Tilt to 3D"}
      title={tilted ? "Look straight down (T)" : "Tilt to 3D (T, or drag with the right mouse button)"}
      onclick={() => ontilt(tilted ? 0 : TILT)}
    >
      <Rotate3d size={18} strokeWidth={1.75} />
    </button>
    <button
      class="icon-button compass"
      aria-label="Reset to north up"
      title="Reset to north up (N)"
      disabled={!rotated}
      onclick={onreset}
    >
      <span style="transform: rotate({-bearing}deg)"><Navigation2 size={18} strokeWidth={1.75} /></span>
    </button>
  </div>
</div>

<style>
  .controls {
    position: absolute;
    right: calc(var(--space-4) + var(--inset));
    transition: right 200ms var(--ease-out);
    bottom: 28px;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: var(--space-2);
  }
  .stack {
    display: flex;
    flex-direction: column;
    padding: 2px;
  }
  .compass span {
    display: inline-grid;
    transition: transform 150ms var(--ease-out);
  }
  .compass :global(svg) {
    color: var(--amber);
  }
  .compass:disabled :global(svg) {
    color: inherit;
  }
  .signed-in {
    color: var(--live);
  }

  .popover {
    position: absolute;
    right: calc(100% + var(--space-2));
    bottom: 0;
    width: 272px;
    max-height: calc(100vh - 60px);
    overflow-y: auto;
    padding: var(--space-4);
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    animation: pop 180ms var(--ease-out);
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: translateX(6px);
    }
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  .segmented {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 2px;
    margin: 0;
    padding: 2px;
    border: 1px solid var(--line-control);
    border-radius: var(--radius-sm);
  }
  .segmented legend {
    margin-bottom: var(--space-2);
    padding: 0;
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-2);
  }
  .segmented label {
    position: relative;
  }
  .segmented input {
    position: absolute;
    opacity: 0;
  }
  .segmented span {
    display: block;
    padding: 5px 0;
    border-radius: 4px;
    font-size: var(--text-md);
    text-align: center;
    color: var(--text-2);
    cursor: pointer;
    transition: background-color 150ms var(--ease-out);
  }
  .segmented label:hover span {
    background: var(--surface-hover);
  }
  .segmented input:checked + span {
    background: var(--amber);
    color: var(--amber-ink);
    font-weight: 650;
  }
  .segmented input:focus-visible + span {
    outline: 2px solid var(--amber);
    outline-offset: 1px;
  }

  .switch {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    font-size: var(--text-md);
    cursor: pointer;
  }
  .switch > span,
  .range > span {
    display: flex;
    flex-direction: column;
  }
  small {
    color: var(--text-3);
    font-size: var(--text-sm);
  }
  .switch input {
    appearance: none;
    flex: none;
    position: relative;
    width: 34px;
    height: 20px;
    margin: 0;
    border-radius: 10px;
    background: var(--line-control);
    cursor: pointer;
    transition: background-color 150ms var(--ease-out);
  }
  .switch input::after {
    content: "";
    position: absolute;
    top: 3px;
    left: 3px;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--text-1);
    transition: transform 150ms var(--ease-out);
  }
  .switch input:checked {
    background: var(--amber);
  }
  .switch input:checked::after {
    transform: translateX(14px);
    background: var(--amber-ink);
  }

  .range {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    font-size: var(--text-md);
  }
  .range > span {
    flex-direction: row;
    justify-content: space-between;
  }
  .range output {
    color: var(--amber);
  }
  .range input {
    width: 100%;
    margin: 0;
  }

  .account {
    gap: var(--space-3);
  }
  .account h2 {
    font-size: var(--text-base);
    font-weight: 650;
  }
  .note {
    font-size: var(--text-md);
    color: var(--text-2);
  }
  .fine {
    font-size: var(--text-xs);
    color: var(--text-3);
  }
  form {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-2);
  }
  .field input {
    height: 34px;
    padding: 0 var(--space-3);
    border: 1px solid var(--line-control);
    border-radius: var(--radius-sm);
    background: var(--ink);
    font-size: var(--text-md);
    font-weight: 400;
    color: var(--text-1);
    outline: none;
  }
  .field input:focus {
    border-color: var(--amber);
  }
  .error {
    font-size: var(--text-sm);
    color: var(--danger);
    overflow-wrap: anywhere;
  }
  .primary {
    justify-content: center;
    border-color: transparent;
    background: var(--amber);
    color: var(--amber-ink);
  }
  .primary:hover {
    background: oklch(0.86 0.15 75);
  }
  .primary:disabled {
    opacity: 0.5;
    cursor: default;
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
