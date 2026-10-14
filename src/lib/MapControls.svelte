<script lang="ts">
  import { tick } from "svelte";
  import {
    CircleUserRound,
    Keyboard,
    Layers,
    LoaderCircle,
    Minus,
    Navigation2,
    Plus,
    Rotate3d,
    Settings,
  } from "@lucide/svelte";
  import LayersMenu from "./LayersMenu.svelte";
  import SettingsMenu from "./SettingsMenu.svelte";
  import type { SessionInfo } from "./api";
  import type { Basemap } from "./geo";

  interface Props {
    zoom: number;
    bearing: number;
    pitch: number;
    basemap: Basemap;
    labels: boolean;
    labelSize: number;
    buildings: boolean;
    /** The aircraft are on the map (the focus or shown with it): their own layers apply. */
    aircraftShown: boolean;
    aircraftLabels: boolean;
    daylight: boolean;
    weather: boolean;
    adsb: boolean;
    railMap: boolean;
    terrain: boolean;
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
    labelSize = $bindable(),
    buildings = $bindable(),
    aircraftShown,
    aircraftLabels = $bindable(),
    daylight = $bindable(),
    weather = $bindable(),
    adsb = $bindable(),
    railMap = $bindable(),
    terrain = $bindable(),
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

  type Popover = "layers" | "settings" | "account";

  const TILT = 55;
  /** One popover at a time. */
  let open = $state<Popover | null>(null);
  let root: HTMLDivElement;
  let firstField = $state<HTMLInputElement>();
  let signedInState = $state<HTMLElement>();
  /** The buttons that open each popover, where Escape sends the keyboard back to. */
  const triggers: Partial<Record<Popover, HTMLButtonElement>> = {};
  /** Where the keyboard goes back to when the account popover closes, if it was opened from elsewhere. */
  let returnFocus: (() => HTMLElement | null) | null = null;

  let email = $state("");
  let password = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);

  const tilted = $derived(pitch > 5);
  const rotated = $derived(Math.abs(bearing) > 0.5 || tilted);
  const signedIn = $derived(session?.authenticated ?? false);

  /**
   * Opens the account popover. From a "sign in" prompt elsewhere, `focusForm`
   * puts the caret in its first field, and `returnFocus` names where the
   * keyboard goes back to once it closes.
   */
  export async function openAccount(opts: { focusForm?: boolean; returnFocus?: () => HTMLElement | null } = {}) {
    open = "account";
    error = null;
    returnFocus = opts.returnFocus ?? null;
    if (opts.focusForm) {
      await tick();
      firstField?.focus();
    }
  }

  /**
   * Closes whichever popover is open. Focus left inside it would fall to the
   * page: it goes back where it came from. With `toTrigger` (Escape), the
   * keyboard goes back to the button that opened it.
   */
  function close(toTrigger = false) {
    if (!open) return;
    const trigger = triggers[open];
    open = null;
    const back = returnFocus ?? (toTrigger ? () => trigger ?? null : null);
    returnFocus = null;
    if (!back) return;
    if (toTrigger) {
      back()?.focus();
      return;
    }
    // after the click or key has moved focus on its own, e.g. to the control clicked
    requestAnimationFrame(() => {
      const active = document.activeElement;
      if (active && active !== document.body && active.isConnected) return;
      back()?.focus();
    });
  }

  function toggle(which: Popover) {
    if (open === which) return close();
    // moving on to another popover: nothing to go back to
    returnFocus = null;
    if (which === "account") error = null;
    open = which;
  }

  function onpointerdown(e: PointerEvent) {
    if (open && !root.contains(e.target as Node)) close();
  }

  // in the capture phase, so Escape closes the popover before it closes anything else
  function onkeydowncapture(e: KeyboardEvent) {
    if (e.key !== "Escape" || !open) return;
    e.stopPropagation();
    close(true);
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    error = null;
    try {
      await onsignin(email, password);
      password = "";
      // the form is gone: say so where the keyboard was, rather than dropping it to the page
      await tick();
      signedInState?.focus();
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
  <div class="stack surface">
    <button
      class="icon-button"
      aria-label="Map layers"
      aria-expanded={open === "layers"}
      aria-controls="layers-popover"
      aria-pressed={open === "layers"}
      title="Map layers"
      bind:this={triggers.layers}
      onclick={() => toggle("layers")}
    >
      <Layers size={18} strokeWidth={1.75} />
    </button>
    {#if open === "layers"}
      <section id="layers-popover" class="popover surface menu" aria-label="Map layers">
        <LayersMenu
          bind:basemap
          bind:terrain
          bind:buildings
          bind:labels
          bind:railMap
          {aircraftShown}
          bind:aircraftLabels
          bind:weather
          bind:daylight
        />
      </section>
    {/if}
    <button
      class="icon-button"
      aria-label="Settings"
      aria-expanded={open === "settings"}
      aria-controls="settings-popover"
      aria-pressed={open === "settings"}
      title="Settings"
      bind:this={triggers.settings}
      onclick={() => toggle("settings")}
    >
      <Settings size={18} strokeWidth={1.75} />
    </button>
    {#if open === "settings"}
      <section id="settings-popover" class="popover surface menu" aria-label="Settings">
        <SettingsMenu {labels} bind:labelSize bind:exaggeration bind:adsb />
      </section>
    {/if}
    <button
      class="icon-button"
      class:signed-in={signedIn}
      aria-label="Flightradar24 account"
      aria-expanded={open === "account"}
      aria-controls="account-popover"
      aria-pressed={open === "account"}
      title={signedIn ? "Signed in to Flightradar24" : "Sign in to Flightradar24"}
      bind:this={triggers.account}
      onclick={() => toggle("account")}
    >
      <CircleUserRound size={18} strokeWidth={1.75} />
    </button>
    {#if open === "account"}
      <section id="account-popover" class="popover surface account" aria-labelledby="account-heading">
        <h2 id="account-heading">Flightradar24 account</h2>
        {#if signedIn}
          <p class="state" tabindex="-1" bind:this={signedInState}><span class="badge good">Signed in</span></p>
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
              <input type="email" autocomplete="username" required bind:value={email} disabled={busy} bind:this={firstField} />
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
    <button
      class="icon-button"
      aria-label="Keyboard shortcuts"
      title="Keyboard shortcuts (?)"
      onclick={() => {
        close();
        onhelp();
      }}
    >
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
  .menu {
    width: 300px;
    gap: var(--space-3);
  }
  .account {
    gap: var(--space-3);
  }
  /* where the keyboard lands after signing in: read out, not a control, so no ring */
  .state:focus {
    outline: none;
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
