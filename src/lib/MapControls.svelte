<script lang="ts">
  import {
    KeyRound,
    Keyboard,
    Layers,
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
  import type { Sky } from "./weather";

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
    trainBlocks: boolean;
    sky: Sky;
    terrain: boolean;
    exaggeration: number;
    session: SessionInfo | null;
    /** Extra space on the right, e.g. for an open side panel. */
    inset?: number;
    onzoom: (delta: number) => void;
    onreset: () => void;
    ontilt: (pitch: number) => void;
    onkeys: () => void;
    /** The menu open, if any (bound, so the page can close it on Android's back gesture). */
    menu?: "layers" | "settings" | null;
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
    trainBlocks = $bindable(),
    sky = $bindable(),
    terrain = $bindable(),
    exaggeration = $bindable(),
    session,
    inset = 0,
    onzoom,
    onreset,
    ontilt,
    onkeys,
    onhelp,
    menu: open = $bindable(null),
  }: Props = $props();

  type Popover = "layers" | "settings";

  const TILT = 55;
  // one popover at a time: `open`, the bound `menu` prop
  let root: HTMLDivElement;
  /** The buttons that open each popover, where Escape sends the keyboard back to. */
  const triggers: Partial<Record<Popover, HTMLButtonElement>> = {};

  const tilted = $derived(pitch > 5);
  const rotated = $derived(Math.abs(bearing) > 0.5 || tilted);
  const signedIn = $derived(session?.authenticated ?? false);

  /** Closes whichever popover is open. With `toTrigger` (Escape), the keyboard goes back to the button that opened it. */
  function close(toTrigger = false) {
    if (!open) return;
    const trigger = triggers[open];
    open = null;
    if (toTrigger) trigger?.focus();
  }

  function toggle(which: Popover) {
    if (open === which) return close();
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
          bind:trainBlocks
          bind:sky
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
      aria-label="API keys"
      title={signedIn ? "API keys (signed in to Flightradar24)" : "API keys and Flightradar24 login"}
      onclick={() => {
        close();
        onkeys();
      }}
    >
      <KeyRound size={18} strokeWidth={1.75} />
    </button>
    <button
      class="icon-button desktop-only"
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

  <div class="stack surface zoom">
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
  /* Phones (the query in phone.svelte.ts): a slim rail above the bottom sheet;
     pinching zooms, and the menus open as sheets of their own. */
  @media (max-width: 899px), (max-height: 500px) {
    .controls {
      right: max(var(--space-2), env(safe-area-inset-right, 0px));
      bottom: calc(var(--sheet-h, 0px) + 24px);
      transition: bottom 220ms var(--ease-out);
    }
    .zoom,
    .desktop-only {
      display: none;
    }
    .popover {
      position: fixed;
      left: 0;
      right: 0;
      bottom: 0;
      z-index: 6;
      width: auto;
      max-height: 70dvh;
      border-radius: 12px 12px 0 0;
      padding-bottom: calc(var(--space-4) + env(safe-area-inset-bottom, 0px));
      padding-left: max(var(--space-4), env(safe-area-inset-left, 0px));
      padding-right: max(var(--space-4), env(safe-area-inset-right, 0px));
      animation-name: rise;
    }
    @keyframes rise {
      from {
        transform: translateY(24px);
        opacity: 0;
      }
    }
  }
  /* a phone on its side: the sheet is on the left, clear of the rail */
  @media (max-height: 500px) and (orientation: landscape) {
    .controls {
      bottom: var(--space-2);
    }
  }
</style>
