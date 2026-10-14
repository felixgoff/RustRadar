<!--
  The card that follows the pointer over an aircraft: its photo, when one is
  known, over the flight's essentials. Non-interactive, positioned by transform
  only, so following the pointer never relays the card out.
-->
<script lang="ts">
  import type { LiveFlight } from "./api";
  import { airlineLogoUrl } from "./api";
  import type { HoverInfo, HoverStatus } from "./hover-details";

  interface Props {
    flight: LiveFlight;
    /** Display names of the route's ends ("Vilnius"), or "" when unknown. */
    from: string;
    to: string;
    /**
     * 0–1. `measured` comes from Flightradar24; otherwise it is a great-circle
     * estimate and drawn quieter. Null draws no bar.
     */
    progress: { value: number; measured: boolean } | null;
    info: HoverInfo | null;
    status: HoverStatus;
    /** Pointer position and the size of the area the card must stay inside. */
    x: number;
    y: number;
    width: number;
    height: number;
  }

  let { flight, from, to, progress, info, status, x, y, width, height }: Props = $props();

  const CARD_WIDTH = 272;
  const OFFSET = 16;
  const MARGIN = 8;

  let cardHeight = $state(0);
  let photoLoaded = $state("");
  let photoFailed = $state("");
  let logoFailed = $state(0);

  const image = $derived(info?.images[0] ?? null);
  const src = $derived(image ? image.thumbnail || image.medium : "");
  const hasPhoto = $derived(!!src && photoFailed !== src);
  /** Reserve the photo's space while it may still come; give it back once it won't. */
  const photoOpen = $derived(status === "loading" || (status === "ready" && hasPhoto));
  const logoId = $derived(info && info.logoId !== logoFailed ? info.logoId : 0);

  const callsign = $derived(flight.callsign || flight.flight || "No callsign");
  const route = $derived(from && to ? `${from} to ${to}` : from ? `From ${from}` : to ? `To ${to}` : "");
  const altitude = $derived(flight.onGround ? "On ground" : `${flight.alt.toLocaleString()} ft`);
  const pct = $derived(progress ? Math.min(1, Math.max(0, progress.value)) : 0);

  // right and below the pointer, flipped to the other side near an edge
  const left = $derived.by(() => {
    let l = x + OFFSET;
    if (l + CARD_WIDTH > width - MARGIN) l = x - OFFSET - CARD_WIDTH;
    return Math.max(MARGIN, l);
  });
  const top = $derived.by(() => {
    let t = y + OFFSET;
    if (t + cardHeight > height - MARGIN) t = y - OFFSET - cardHeight;
    return Math.max(MARGIN, t);
  });
</script>

<div
  class="hover-card"
  style:width="{CARD_WIDTH}px"
  style:transform="translate({left}px, {top}px)"
  bind:offsetHeight={cardHeight}
  aria-hidden="true"
>
  <div class="photo-slot" class:open={photoOpen}>
    <div class="photo-clip">
      <div class="photo" class:skeleton={!(hasPhoto && photoLoaded === src)}>
        {#if hasPhoto}
          {#key src}
            <img
              {src}
              alt=""
              class:shown={photoLoaded === src}
              onload={() => (photoLoaded = src)}
              onerror={() => (photoFailed = src)}
            />
          {/key}
          {#if image?.copyright && photoLoaded === src}<span class="credit">© {image.copyright}</span>{/if}
        {/if}
      </div>
    </div>
  </div>

  <div class="body surface">
    <div class="head">
      <span class="callsign data">{callsign}</span>
      {#if flight.typecode}<span class="type data">{flight.typecode}</span>{/if}
      {#if flight.reg}<span class="reg data">{flight.reg}</span>{/if}
      {#if logoId}
        <span class="logo"><img src={airlineLogoUrl(logoId)} alt="" onerror={() => (logoFailed = logoId)} /></span>
      {/if}
    </div>
    {#if route}<p class="route">{route}</p>{/if}
    {#if progress}
      <div class="track" class:estimate={!progress.measured}>
        <div class="fill" style:transform="scaleX({pct})"></div>
      </div>
    {/if}
    <p class="numbers data">{altitude} · {flight.speed} kt</p>
  </div>
</div>

<style>
  .hover-card {
    position: absolute;
    top: 0;
    left: 0;
    z-index: 2;
    display: flex;
    flex-direction: column;
    pointer-events: none;
    will-change: transform;
  }

  /* the photo's row eases shut when no photo is coming, instead of a box left
     empty or a jump */
  .photo-slot {
    display: grid;
    grid-template-rows: 0fr;
    transition: grid-template-rows 180ms var(--ease-out);
  }
  .photo-slot.open {
    grid-template-rows: 1fr;
  }
  .photo-clip {
    min-height: 0;
    overflow: hidden;
  }
  .photo {
    position: relative;
    aspect-ratio: 16 / 9;
    margin-bottom: 6px;
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--surface-raised);
    box-shadow: var(--shadow);
  }
  .photo img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
    opacity: 0;
    transition: opacity 160ms var(--ease-out);
  }
  .photo img.shown {
    opacity: 1;
  }
  .credit {
    position: absolute;
    left: var(--space-2);
    bottom: var(--space-1);
    max-width: calc(100% - var(--space-4));
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--text-xs);
    color: var(--text-1);
    text-shadow: 0 1px 2px oklch(0 0 0 / 0.9);
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--space-2) var(--space-3) var(--space-3);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-height: 26px;
  }
  .callsign {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--text-base);
    font-weight: 700;
  }
  .type {
    flex: none;
    padding: 0 5px;
    border: 1px solid var(--line-control);
    border-radius: 4px;
    font-size: var(--text-xs);
    font-weight: 600;
    line-height: 16px;
    color: var(--text-2);
  }
  .reg {
    flex: none;
    font-size: var(--text-xs);
    color: var(--text-3);
  }
  /* logos are drawn for light backgrounds */
  .logo {
    flex: none;
    display: inline-grid;
    place-items: center;
    height: 22px;
    margin-left: auto;
    padding: 2px 5px;
    border-radius: 4px;
    background: var(--text-1);
  }
  .logo img {
    max-height: 18px;
    max-width: 72px;
  }
  .route {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--text-md);
    color: var(--text-2);
  }
  .track {
    height: 2px;
    margin: var(--space-2) 0 var(--space-1);
    border-radius: 1px;
    background: var(--line);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--amber);
    transform-origin: left;
    transition: transform 400ms var(--ease-out);
  }
  /* a great-circle guess, not Flightradar24's figure: quieter */
  .track.estimate .fill {
    background: var(--text-3);
  }
  .numbers {
    font-size: var(--text-sm);
    color: var(--text-2);
  }
</style>
