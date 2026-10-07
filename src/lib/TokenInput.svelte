<script lang="ts" module>
  export interface Suggestion {
    value: string;
    label: string;
    detail?: string;
  }
</script>

<script lang="ts">
  import { X } from "@lucide/svelte";

  interface Props {
    label: string;
    values: string[];
    /** Suggestions for a query (possibly empty), best first. */
    suggest: (query: string) => Suggestion[];
    /** A value typed in full, when no suggestion is picked; `null` if it isn't valid. */
    free?: (query: string) => string | null;
    /** What a chip says for a value. */
    chip?: (value: string) => string;
    placeholder?: string;
    hint?: string;
  }

  let { label, values = $bindable(), suggest, free, chip = (v) => v, placeholder = "", hint }: Props = $props();

  const uid = $props.id();
  let query = $state("");
  let open = $state(false);
  let active = $state(0);

  const suggestions = $derived(open ? suggest(query) : []);

  function add(value: string) {
    if (!values.includes(value)) values = [...values, value];
    query = "";
    active = 0;
  }

  function remove(value: string) {
    values = values.filter((v) => v !== value);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "ArrowDown" && suggestions.length) {
      e.preventDefault();
      active = (active + 1) % suggestions.length;
    } else if (e.key === "ArrowUp" && suggestions.length) {
      e.preventDefault();
      active = (active - 1 + suggestions.length) % suggestions.length;
    } else if (e.key === "Enter") {
      e.preventDefault();
      const typed = free?.(query.trim());
      if (suggestions[active]) add(suggestions[active].value);
      else if (typed) add(typed);
    } else if (e.key === "Backspace" && !query && values.length) {
      remove(values[values.length - 1]);
    } else if (e.key === "Escape" && open) {
      e.stopPropagation();
      open = false;
    }
  }
</script>

<div class="token-input">
  <label class="label" for="{uid}-input">{label}</label>
  <div class="field">
    {#each values as value (value)}
      <span class="chip" title={chip(value)}>
        <span class="text">{chip(value)}</span>
        <button aria-label="Remove {chip(value)}" onclick={() => remove(value)}><X size={12} strokeWidth={2} /></button>
      </span>
    {/each}
    <input
      id="{uid}-input"
      type="text"
      bind:value={query}
      {placeholder}
      {onkeydown}
      oninput={() => {
        open = true;
        active = 0;
      }}
      onfocus={() => (open = true)}
      onblur={() => (open = false)}
      spellcheck="false"
      autocomplete="off"
      role="combobox"
      aria-expanded={suggestions.length > 0}
      aria-controls="{uid}-list"
      aria-activedescendant={suggestions[active] ? `${uid}-option-${active}` : undefined}
      aria-describedby={hint ? `${uid}-hint` : undefined}
    />
  </div>
  {#if suggestions.length}
    <ul class="suggestions surface" id="{uid}-list" role="listbox" aria-label={label}>
      {#each suggestions as s, i (s.value)}
        <li
          id="{uid}-option-{i}"
          role="option"
          aria-selected={i === active}
          class:active={i === active}
          onmouseenter={() => (active = i)}
          onmousedown={(e) => {
            e.preventDefault();
            add(s.value);
          }}
        >
          <span class="option-label">{s.label}</span>
          {#if s.detail}<span class="detail data">{s.detail}</span>{/if}
        </li>
      {/each}
    </ul>
  {/if}
  {#if hint}<p class="hint" id="{uid}-hint">{hint}</p>{/if}
</div>

<style>
  .token-input {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .label {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-2);
  }
  .field {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-1);
    min-height: 34px;
    padding: 3px var(--space-2);
    border: 1px solid var(--line-control);
    border-radius: var(--radius-sm);
    background: var(--ink);
    transition: border-color 150ms var(--ease-out);
  }
  .field:focus-within {
    border-color: var(--amber);
  }
  input {
    flex: 1;
    min-width: 80px;
    height: 26px;
    border: 0;
    outline: none;
    background: none;
    font-size: var(--text-md);
  }
  input::placeholder {
    color: var(--text-3);
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    max-width: 100%;
    height: 24px;
    padding-left: var(--space-2);
    border-radius: 4px;
    background: var(--amber-soft);
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--amber);
  }
  .chip .text {
    max-width: 150px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .chip button {
    display: inline-grid;
    place-items: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: none;
    color: inherit;
    cursor: pointer;
  }
  .chip button:hover {
    background: var(--amber-soft);
  }
  /* in the flow, not floating: the filter panel scrolls and would clip a popup */
  .suggestions {
    padding: var(--space-1);
    background: var(--surface-raised);
    box-shadow: none;
  }
  [role="option"] {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-3);
    padding: 6px var(--space-2);
    border-radius: 4px;
    font-size: var(--text-md);
    cursor: pointer;
  }
  [role="option"].active {
    background: var(--surface-hover);
  }
  .option-label {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .detail {
    flex: none;
    font-size: var(--text-xs);
    color: var(--text-3);
  }
  .hint {
    font-size: var(--text-xs);
    color: var(--text-3);
  }
</style>
