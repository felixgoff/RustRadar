<script lang="ts">
  import { clockAt, zoneName } from "./time";

  interface Props {
    /** Unix seconds; a dash without one. */
    unix: number | null | undefined;
    /** IANA zone; the viewer's own without one. */
    timeZone?: string;
  }

  let { unix, timeZone }: Props = $props();
</script>

<!-- `13:25 BST`: the clock in the data face, its zone in the UI face, so the
     space between them is a word space rather than a monospaced cell -->
{#if unix}
  <span class="clock"><span class="data">{clockAt(unix, timeZone)}</span> <span class="zone">{zoneName(unix, timeZone)}</span></span>
{:else}
  <span class="data">—</span>
{/if}

<style>
  .clock {
    white-space: nowrap;
  }
  .zone {
    font-family: var(--font-ui);
    letter-spacing: normal;
    color: var(--text-3);
  }
</style>
