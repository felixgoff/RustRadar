<script lang="ts">
  import "@fontsource-variable/overpass";
  import "@fontsource-variable/overpass-mono";
  import "../app.css";

  let { children } = $props();

  // This is an app, not a page: no browser context menu ("Save image as" on
  // the map, "Inspect"...), except in text fields where copy and paste help.
  // Right-dragging the map rotates and tilts it.
  function oncontextmenu(e: MouseEvent) {
    const target = e.target as HTMLElement | null;
    if (!target?.closest("input, textarea, [contenteditable='true']")) e.preventDefault();
  }
</script>

<svelte:window {oncontextmenu} />

{@render children()}
