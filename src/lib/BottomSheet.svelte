<script lang="ts" module>
  export type Detent = "peek" | "half" | "full";
</script>

<script lang="ts">
  import type { Snippet } from "svelte";

  // On a phone, the lists and filters in a sheet over the bottom of the map.
  // Drag the handle between three heights, or tap it to step through them.

  interface Props {
    detent?: Detent;
    /** The sheet's height now, in px: what sits above it moves with it. */
    height?: number;
    label: string;
    /** Always on show, under the handle. */
    head?: Snippet;
    children: Snippet;
  }

  let { detent = $bindable("peek"), height = $bindable(0), label, head, children }: Props = $props();

  const PEEK = 132;
  /** What the full sheet leaves clear at the top: the search and mode buttons. */
  const TOP_CLEAR = 104;
  const ORDER: Detent[] = ["peek", "half", "full"];

  let vh = $state(typeof innerHeight === "number" ? innerHeight : 800);
  let drag = $state<{ y: number; from: number; now: number; moved: boolean } | null>(null);

  const heights = $derived({ peek: PEEK, half: Math.round(vh * 0.5), full: Math.max(PEEK, vh - TOP_CLEAR) });
  const shown = $derived(drag ? drag.now : heights[detent]);

  $effect(() => {
    height = shown;
  });

  function down(e: PointerEvent) {
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    drag = { y: e.clientY, from: heights[detent], now: heights[detent], moved: false };
  }

  function move(e: PointerEvent) {
    if (!drag) return;
    const dy = drag.y - e.clientY;
    drag.now = Math.min(heights.full, Math.max(PEEK - 40, drag.from + dy));
    if (Math.abs(dy) > 6) drag.moved = true;
  }

  function up() {
    if (!drag) return;
    if (!drag.moved) {
      detent = ORDER[(ORDER.indexOf(detent) + 1) % ORDER.length];
    } else {
      const now = drag.now;
      detent = ORDER.reduce((best, d) => (Math.abs(heights[d] - now) < Math.abs(heights[best] - now) ? d : best), detent);
    }
    drag = null;
  }

  function onkeydown(e: KeyboardEvent) {
    const at = ORDER.indexOf(detent);
    if (e.key === "ArrowUp" && at < ORDER.length - 1) detent = ORDER[at + 1];
    else if (e.key === "ArrowDown" && at > 0) detent = ORDER[at - 1];
    else return;
    e.preventDefault();
  }
</script>

<svelte:window bind:innerHeight={vh} />

<section class="sheet surface" class:dragging={drag} style:height="{shown}px" aria-label={label}>
  <button
    class="grab"
    aria-label="{label}: {detent === 'full' ? 'full height' : detent === 'half' ? 'half height' : 'lowered'}. Drag, tap or use the arrow keys to resize"
    onpointerdown={down}
    onpointermove={move}
    onpointerup={up}
    onpointercancel={up}
    {onkeydown}
  >
    <span></span>
  </button>
  {#if head}<div class="head">{@render head()}</div>{/if}
  <div class="body">{@render children()}</div>
</section>

<style>
  .sheet {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 4;
    display: flex;
    flex-direction: column;
    border-bottom: 0;
    border-left: 0;
    border-right: 0;
    border-radius: 12px 12px 0 0;
    padding-bottom: env(safe-area-inset-bottom, 0px);
    transition: height 220ms var(--ease-out);
    overflow: hidden;
  }
  .sheet.dragging {
    transition: none;
  }
  .grab {
    flex: none;
    display: grid;
    place-items: center;
    height: 22px;
    padding: 0;
    border: 0;
    background: none;
    cursor: grab;
    touch-action: none;
  }
  .grab span {
    width: 36px;
    height: 4px;
    border-radius: 2px;
    background: var(--line-control);
  }
  .grab:focus-visible span {
    background: var(--amber);
  }
  .head {
    flex: none;
    padding: 0 var(--space-4) var(--space-2);
    padding-left: max(var(--space-4), env(safe-area-inset-left, 0px));
    padding-right: max(var(--space-4), env(safe-area-inset-right, 0px));
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding-left: env(safe-area-inset-left, 0px);
    padding-right: env(safe-area-inset-right, 0px);
  }
  @media (prefers-reduced-motion: reduce) {
    .sheet {
      transition: none;
    }
  }
  /* a phone on its side: the sheet runs down the left, under the search */
  @media (max-height: 500px) and (orientation: landscape) {
    .sheet {
      right: auto;
      left: max(var(--space-2), env(safe-area-inset-left, 0px));
      width: 320px;
    }
  }
</style>
