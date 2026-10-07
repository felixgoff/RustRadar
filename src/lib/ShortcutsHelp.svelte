<script lang="ts">
  import { X } from "@lucide/svelte";

  interface Props {
    open: boolean;
    onclose: () => void;
  }

  let { open, onclose }: Props = $props();

  const SHORTCUTS: [string[], string][] = [
    [["/"], "Search flights and airports"],
    [["F"], "Show or hide filters"],
    [["C"], "Follow the selected aircraft with the camera"],
    [["Esc"], "Close the open panel"],
    [["+", "−"], "Zoom in or out"],
    [["T"], "Tilt to 3D, or look straight down"],
    [["N"], "North up"],
    [["1", "2"], "Dark map or satellite imagery"],
    [["L"], "Place names"],
    [["B"], "3D buildings"],
    [["P"], "Airline liveries"],
    [["D"], "Day and night"],
    [["W"], "Weather radar"],
    [["?"], "This list"],
  ];

  let dialog: HTMLDialogElement;

  $effect(() => {
    if (open && !dialog.open) dialog.showModal();
    else if (!open && dialog.open) dialog.close();
  });
</script>

<!-- a click on the backdrop lands on the dialog itself; the content fills it otherwise -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<dialog bind:this={dialog} class="surface" aria-labelledby="shortcuts-heading" {onclose} onclick={(e) => e.target === dialog && dialog.close()}>
  <div class="content">
    <header>
      <h2 id="shortcuts-heading">Keyboard shortcuts</h2>
      <button class="icon-button" aria-label="Close" onclick={() => dialog.close()}><X size={16} strokeWidth={1.75} /></button>
    </header>
    <dl>
      {#each SHORTCUTS as [keys, what] (what)}
        <div>
          <dt>
            {#each keys as key (key)}<kbd>{key}</kbd>{/each}
          </dt>
          <dd>{what}</dd>
        </div>
      {/each}
    </dl>
  </div>
</dialog>

<style>
  dialog {
    width: 360px;
    max-width: calc(100vw - 32px);
    padding: 0;
    color: var(--text-1);
  }
  dialog::backdrop {
    background: oklch(0.1 0.02 255 / 0.5);
  }
  dialog[open] {
    animation: pop 180ms var(--ease-out);
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }
  .content {
    padding: var(--space-3) var(--space-4) var(--space-4);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: var(--space-2);
  }
  h2 {
    font-size: var(--text-base);
    font-weight: 650;
  }
  dl > div {
    display: grid;
    grid-template-columns: 72px 1fr;
    align-items: center;
    gap: var(--space-3);
    padding: 5px 0;
    border-top: 1px solid var(--line);
  }
  dt {
    display: flex;
    gap: var(--space-1);
  }
  dd {
    font-size: var(--text-md);
    color: var(--text-2);
  }
  kbd {
    display: inline-grid;
    place-items: center;
    min-width: 22px;
    height: 22px;
    padding: 0 5px;
    border: 1px solid var(--line-control);
    border-bottom-width: 2px;
    border-radius: 4px;
    font: 600 var(--text-sm) var(--font-data);
    color: var(--text-1);
  }
</style>
