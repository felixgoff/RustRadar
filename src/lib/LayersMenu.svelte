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
    liveries: boolean;
    ships: boolean;
    trains: boolean;
    weather: boolean;
    daylight: boolean;
  }

  let {
    basemap = $bindable(),
    terrain = $bindable(),
    buildings = $bindable(),
    labels = $bindable(),
    railMap = $bindable(),
    liveries = $bindable(),
    ships = $bindable(),
    trains = $bindable(),
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
</section>

<section aria-labelledby="layers-traffic">
  <h3 id="layers-traffic">Traffic</h3>
  <ToggleRow label="Airline liveries" hint="Aircraft in their airline's colours" key="P" bind:checked={liveries} />
  <ToggleRow label="Ships" hint="Live from VesselFinder" key="S" bind:checked={ships} />
  <ToggleRow label="Trains" hint="Live in Finland, Norway, Ireland, USA and Canada" key="I" bind:checked={trains} />
</section>

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
