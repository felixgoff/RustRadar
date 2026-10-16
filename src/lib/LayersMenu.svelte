<script lang="ts">
  import Segmented from "./Segmented.svelte";
  import ToggleRow from "./ToggleRow.svelte";
  import type { Basemap } from "./geo";

  // What is drawn on the globe. How the app behaves lives in SettingsMenu.
  interface Props {
    basemap: Basemap;
    terrain: boolean;
    buildings: boolean;
    labels: boolean;
    railMap: boolean;
    trainBlocks: boolean;
    realistic: boolean;
    /** The aircraft are on the map (the focus or shown with it): their own layers apply. */
    aircraftShown: boolean;
    aircraftLabels: boolean;
    weather: boolean;
    daylight: boolean;
  }

  let {
    basemap = $bindable(),
    terrain = $bindable(),
    buildings = $bindable(),
    labels = $bindable(),
    railMap = $bindable(),
    trainBlocks = $bindable(),
    realistic = $bindable(),
    aircraftShown,
    aircraftLabels = $bindable(),
    weather = $bindable(),
    daylight = $bindable(),
  }: Props = $props();

  const BASEMAPS = [
    ["dark", "Dark", "1"],
    ["satellite", "Satellite", "2"],
  ] as const;
</script>

<section aria-labelledby="layers-map">
  <h3 id="layers-map">Map</h3>
  <Segmented legend="Basemap" hideLegend name="basemap" options={BASEMAPS} bind:value={basemap} />
  <ToggleRow label="Terrain" hint="Mountains and valleys in relief" key="E" bind:checked={terrain} />
  <ToggleRow label="3D buildings" hint="Appear at street level" key="B" bind:checked={buildings} />
  <ToggleRow label="Place names" key="L" bind:checked={labels} />
  <ToggleRow label="Rail map" hint="Railway lines from OpenStreetMap" key="R" bind:checked={railMap} />
  <ToggleRow label="Signal blocks" hint="Free or taken, and by which train (zoom in)" bind:checked={trainBlocks} />
  <ToggleRow label="Realistic 3D" hint="Streets GL's city where the map is; no live traffic in it" bind:checked={realistic} />
</section>

<!-- which traffic is on the map is the mode bar's: only the aircraft have a layer of their own, offered while they are on it -->
{#if aircraftShown}
  <section aria-labelledby="layers-traffic">
    <h3 id="layers-traffic">Aircraft</h3>
    <ToggleRow label="Aircraft labels" hint="Callsign next to each aircraft" key="K" bind:checked={aircraftLabels} />
  </section>
{/if}

<section aria-labelledby="layers-sky">
  <h3 id="layers-sky">Sky</h3>
  <ToggleRow label="Weather radar" hint="Rain and snow, every 10 min" key="W" bind:checked={weather} />
  <ToggleRow label="Day and night" hint="Shades where the sun has set" key="D" bind:checked={daylight} />
</section>

<style>
  section {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  section + section {
    padding-top: var(--space-3);
    border-top: 1px solid var(--line);
  }
  h3 {
    margin-bottom: var(--space-1);
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-3);
  }
  /* the basemap switch sits a little apart from the rows under it */
  section > :global(.segmented) {
    margin-bottom: var(--space-1);
  }
</style>
