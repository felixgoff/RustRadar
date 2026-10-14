<script lang="ts">
  import type { TrailPoint } from "./api";
  import { EXPORT_FORMATS, exportFileName, exportPath, exportPoints, saveExport, type ExportFormat, type ExportMeta } from "./export";

  interface Props {
    trail: TrailPoint[];
    meta: ExportMeta;
  }

  let { trail, meta }: Props = $props();

  let open = $state(false);
  let busy = $state(false);
  /** The last outcome, announced and shown briefly under the menu button. */
  let status = $state<{ text: string; error: boolean } | null>(null);
  let root = $state<HTMLElement>();
  let trigger = $state<HTMLButtonElement>();
  let clearTimer: ReturnType<typeof setTimeout> | undefined;

  const count = $derived(exportPoints(trail).length);

  function close(focusTrigger = false) {
    open = false;
    if (focusTrigger) trigger?.focus();
  }

  async function save(format: ExportFormat) {
    busy = true;
    try {
      const contents = exportPath(trail, meta, format);
      const name = exportFileName(meta, exportPoints(trail), format);
      const where = await saveExport(name, contents, format);
      const file = where.split(/[\\/]/).pop() || name;
      show(`Saved ${file}`, false);
    } catch (e) {
      show(`Couldn't save the path: ${String(e).replace(/^Error:\s*/, "")}`, true);
    } finally {
      busy = false;
      close(true);
    }
  }

  function show(text: string, error: boolean) {
    status = { text, error };
    clearTimeout(clearTimer);
    clearTimer = setTimeout(() => (status = null), error ? 8000 : 5000);
  }

  function onkeydown(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === "Escape") {
      e.stopPropagation();
      close(true);
      return;
    }
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const items = [...(root?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [])];
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = e.key === "ArrowDown" ? (at + 1) % items.length : (at - 1 + items.length) % items.length;
    items[next]?.focus();
    e.preventDefault();
  }

  function onfocusout(e: FocusEvent) {
    if (open && !root?.contains(e.relatedTarget as Node | null)) open = false;
  }

  function toggle() {
    open = !open;
    if (open) queueMicrotask(() => root?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus());
  }
</script>

<svelte:window onclick={(e) => open && !root?.contains(e.target as Node) && close()} />

<span class="export" bind:this={root} {onfocusout}>
  <button
    bind:this={trigger}
    class="toggle"
    class:error={status?.error}
    aria-haspopup="menu"
    aria-expanded={open}
    disabled={count < 2 || busy}
    title={status?.text ??
      (count < 2 ? "Nothing to export until the aircraft has reported a few positions" : "Save this flight's path as a file")}
    onclick={toggle}
  >
    {busy ? "Saving…" : status ? (status.error ? "Not saved" : "Saved") : "Export"}
  </button>
  {#if open}
    <div class="menu surface" role="menu" tabindex="-1" aria-label="Export the flight path" {onkeydown}>
      {#each EXPORT_FORMATS as f (f.format)}
        <button role="menuitem" onclick={() => save(f.format)}>
          <span class="label">{f.label}</span>
          <span class="hint">{f.hint}</span>
        </button>
      {/each}
      <p class="note">{count} positions, saved to Downloads</p>
    </div>
  {/if}
  <span class="visually-hidden" role="status">{status?.text ?? ""}</span>
</span>

<style>
  .export {
    position: relative;
  }
  .toggle {
    padding: 0;
    border: 0;
    background: none;
    font-size: var(--text-sm);
    color: var(--text-3);
    cursor: pointer;
  }
  .toggle:hover:not(:disabled),
  .toggle[aria-expanded="true"] {
    color: var(--text-1);
  }
  .toggle:disabled {
    cursor: default;
    opacity: 0.5;
  }
  .menu {
    position: absolute;
    top: calc(100% + var(--space-2));
    right: 0;
    z-index: 5;
    display: flex;
    flex-direction: column;
    width: 240px;
    padding: var(--space-1);
    border-radius: 0;
    background: var(--surface-raised);
  }
  [role="menuitem"] {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--space-2);
    border: 0;
    border-radius: 0;
    background: none;
    color: var(--text-1);
    text-align: left;
    cursor: pointer;
  }
  [role="menuitem"]:hover,
  [role="menuitem"]:focus-visible {
    background: var(--surface-hover);
  }
  .label {
    font-size: var(--text-md);
    font-weight: 600;
  }
  .hint {
    font-size: var(--text-sm);
    color: var(--text-3);
  }
  .note {
    margin: var(--space-1) var(--space-2) var(--space-1);
    padding-top: var(--space-2);
    border-top: 1px solid var(--line);
    font-size: var(--text-sm);
    color: var(--text-3);
  }
  .toggle.error {
    color: var(--danger);
  }

</style>
