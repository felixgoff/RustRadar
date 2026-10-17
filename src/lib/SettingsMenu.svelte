<script lang="ts">
  import { KeyRound } from "@lucide/svelte";
  import Segmented from "./Segmented.svelte";
  import ToggleRow from "./ToggleRow.svelte";

  // How the app behaves. What is drawn on the globe lives in LayersMenu.
  interface Props {
    /** Whether place names are on, which the size applies to. */
    labels: boolean;
    labelSize: number;
    exaggeration: number;
    adsb: boolean;
    /** Opens the keys page: listed here on a phone, where the map's rail has no room for its own button. */
    onkeys?: () => void;
  }

  let { labels, labelSize = $bindable(), exaggeration = $bindable(), adsb = $bindable(), onkeys }: Props = $props();

  /** Place-name sizes on offer, as multiples of the default. */
  const LABEL_SIZES = [
    [0.85, "Small"],
    [1, "Medium"],
    [1.25, "Large"],
    [1.5, "Extra large"],
  ] as const;
</script>

<section aria-labelledby="settings-display">
  <h3 id="settings-display">Display</h3>
  <div class="field">
    <Segmented
      legend="Place name size"
      name="label-size"
      options={LABEL_SIZES}
      columns={2}
      disabled={!labels}
      bind:value={labelSize}
    />
    {#if !labels}<small>Place names are off in Layers</small>{/if}
  </div>
  <label class="range">
    <span>Altitude exaggeration <output class="data">×{exaggeration}</output></span>
    <input type="range" min="1" max="40" step="1" bind:value={exaggeration} />
    <small>Applies when zoomed out; true scale up close</small>
  </label>
</section>

<section aria-labelledby="settings-data">
  <h3 id="settings-data">Data</h3>
  <ToggleRow label="Aircraft-reported data" hint="Heading, bank and autopilot, from adsb.lol" bind:checked={adsb} />
  {#if onkeys}
    <button class="button keys" onclick={onkeys}><KeyRound size={14} strokeWidth={1.75} /> API keys and Flightradar24 login</button>
  {/if}
</section>

<style>
  section {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  section + section {
    padding-top: var(--space-3);
    border-top: 1px solid var(--line);
  }
  h3 {
    margin-bottom: calc(var(--space-1) * -1);
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-3);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  small {
    font-size: var(--text-sm);
    color: var(--text-3);
  }

  .range {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    font-size: var(--text-md);
  }
  .range > span {
    display: flex;
    justify-content: space-between;
  }
  .range output {
    color: var(--amber);
  }
  .range input {
    width: 100%;
    margin: 0;
  }
  .range small {
    margin-top: calc(var(--space-1) * -1);
  }
  .keys {
    display: none;
  }
  @media (max-width: 899px), (max-height: 500px) {
    .keys {
      display: inline-flex;
      align-self: flex-start;
    }
  }
</style>
