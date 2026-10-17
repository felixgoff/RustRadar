<script lang="ts" module>
  export type Detent = "peek" | "half" | "full";
</script>

<script lang="ts">
  import type { Snippet } from "svelte";

  // On a phone, the lists and filters in a sheet over the bottom of the map,
  // and the details of whatever is picked in their place. Drag the handle
  // between three heights, or tap it to step through them.

  interface Props {
    detent?: Detent;
    /** The sheet's height on screen now, in px, home-bar padding included: what sits above it moves with it. */
    height?: number;
    label: string;
    /** What the full sheet leaves clear at the top, px: the search and mode buttons, when they're on show. */
    clear?: number;
    /** Always on show, under the handle. */
    head?: Snippet;
    children: Snippet;
  }

  let { detent = $bindable("peek"), height = $bindable(0), label, clear = 64, head, children }: Props = $props();

  const PEEK = 90;
  const ORDER: Detent[] = ["peek", "half", "full"];

  let vh = $state(typeof innerHeight === "number" ? innerHeight : 800);
  let drag = $state<{ y: number; from: number; now: number; moved: boolean } | null>(null);

  // Content that marks its summary with `data-peek` (a panel's name strip, its
  // key line and its sticky actions) is lowered to just that: the map stays in view.
  let grab = $state(0);
  let headHeight = $state(0);
  let body: HTMLDivElement;
  let summary = $state(0);
  $effect(() => {
    const marked = () => [...body.querySelectorAll<HTMLElement>("[data-peek]")];
    const measure = () => (summary = marked().reduce((sum, el) => sum + el.offsetHeight, 0));
    const sizes = new ResizeObserver(measure);
    const watch = () => {
      sizes.disconnect();
      marked().forEach((el) => sizes.observe(el));
      measure();
    };
    const changes = new MutationObserver(watch);
    changes.observe(body, { childList: true, subtree: true });
    watch();
    return () => {
      sizes.disconnect();
      changes.disconnect();
    };
  });
  const peek = $derived(summary ? grab + (head ? headHeight : 0) + summary : PEEK);
  const heights = $derived({ peek, half: Math.max(peek, Math.round(vh * 0.5)), full: Math.max(peek, vh - clear) });
  const shown = $derived(drag ? drag.now : heights[detent]);


  function down(e: PointerEvent) {
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    drag = { y: e.clientY, from: heights[detent], now: heights[detent], moved: false };
  }

  function move(e: PointerEvent) {
    if (!drag) return;
    const dy = drag.y - e.clientY;
    drag.now = Math.min(heights.full, Math.max(heights.peek - 40, drag.from + dy));
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

<!-- `shown` is the content's height; the home-bar padding goes under it, and the measured whole is reported -->
<section
  class="sheet surface"
  class:dragging={drag}
  style:height="{shown}px"
  style:--clear="{clear}px"
  bind:offsetHeight={height}
  aria-label={label}
>
  <button
    class="grab"
    bind:offsetHeight={grab}
    aria-label="{label}: {detent === 'full' ? 'full height' : detent === 'half' ? 'half height' : 'lowered'}. Drag, tap or use the arrow keys to resize"
    onpointerdown={down}
    onpointermove={move}
    onpointerup={up}
    onpointercancel={up}
    {onkeydown}
  >
    <span></span>
  </button>
  {#if head}<div class="head" bind:offsetHeight={headHeight}>{@render head()}</div>{/if}
  <div class="body" bind:this={body}>{@render children()}</div>
</section>

<style>
  .sheet {
    box-sizing: content-box;
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
    /* below a notch, the full height still leaves the search clear */
    max-height: calc(100dvh - var(--clear) - env(safe-area-inset-top, 0px) - env(safe-area-inset-bottom, 0px));
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
    padding: 0 var(--space-4) var(--space-1);
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
