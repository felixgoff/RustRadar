<script lang="ts">
  interface Props {
    label: string;
    /** One quiet line under the label. */
    hint?: string;
    /** Its single-key shortcut, as shown in the keyboard help. */
    key?: string;
    checked: boolean;
  }

  let { label, hint, key, checked = $bindable() }: Props = $props();
</script>

<label class="row">
  <span class="text">
    <span class="label">{label}</span>
    {#if hint}<small>{hint}</small>{/if}
  </span>
  {#if key}<kbd aria-hidden="true">{key}</kbd>{/if}
  <input type="checkbox" role="switch" aria-keyshortcuts={key} bind:checked />
</label>

<style>
  .row {
    display: grid;
    grid-template-columns: 1fr auto auto;
    align-items: center;
    gap: var(--space-3);
    min-height: 32px;
    font-size: var(--text-md);
    cursor: pointer;
  }
  .text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  small {
    overflow: hidden;
    font-size: var(--text-sm);
    color: var(--text-3);
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  kbd {
    display: inline-grid;
    place-items: center;
    min-width: 20px;
    height: 20px;
    padding: 0 4px;
    border: 1px solid var(--line);
    border-bottom-width: 2px;
    border-radius: 4px;
    font: 600 var(--text-xs) var(--font-data);
    color: var(--text-3);
  }

  /* the switch, as across the app's popovers; always in the last column, with or without a shortcut */
  input {
    grid-column: 3;
    appearance: none;
    flex: none;
    position: relative;
    width: 34px;
    height: 20px;
    margin: 0;
    border-radius: 10px;
    background: var(--line-control);
    cursor: pointer;
    transition: background-color 150ms var(--ease-out);
  }
  input::after {
    content: "";
    position: absolute;
    top: 3px;
    left: 3px;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--text-1);
    transition: transform 150ms var(--ease-out);
  }
  input:checked {
    background: var(--amber);
  }
  input:checked::after {
    transform: translateX(14px);
    background: var(--amber-ink);
  }
</style>
