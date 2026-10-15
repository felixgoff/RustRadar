<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
  import { LoaderCircle, X } from "@lucide/svelte";
  import { tick } from "svelte";
  import { isTauri, type SessionInfo } from "./api";
  import { EMPTY_KEYS, keysFile, loadKeys, readKeysFile, saveKeys, type Keys } from "./keys";

  interface Props {
    open: boolean;
    session: SessionInfo | null;
    /** Put the caret in the Flightradar24 email field on opening (a "sign in" prompt). */
    focusLogin?: boolean;
    onclose: () => void;
    onsignin: (email: string, password: string) => Promise<void>;
    onsignout: () => Promise<void>;
  }

  let { open, session, focusLogin = false, onclose, onsignin, onsignout }: Props = $props();

  let dialog: HTMLDialogElement;
  let emailField = $state<HTMLInputElement>();
  let fileInput = $state<HTMLInputElement>();
  /** As saved; the form is compared against it. */
  let saved = $state<Keys>({ ...EMPTY_KEYS });
  let form = $state<Keys>({ ...EMPTY_KEYS });
  let withLogin = $state(false);
  let busy = $state(false);
  let status = $state<{ text: string; error: boolean } | null>(null);

  const signedIn = $derived(session?.authenticated ?? false);
  const changed = $derived((Object.keys(form) as (keyof Keys)[]).some((k) => form[k] !== saved[k]));
  const hasLogin = $derived(!!form.fr24Email && !!form.fr24Password);

  $effect(() => {
    if (open && !dialog.open) {
      dialog.showModal();
      status = null;
      loadKeys().then(async (k) => {
        saved = k;
        form = { ...k };
        if (focusLogin) {
          await tick();
          emailField?.focus();
        }
      });
    }
    if (!open && dialog.open) dialog.close();
  });

  const message = (e: unknown) => String(e instanceof Error ? e.message : e).replace(/^Error:\s*/, "");

  /** Saves the form, then signs in to Flightradar24 if its login changed. */
  async function save(next: Keys = form, done = "Saved") {
    busy = true;
    status = null;
    const loginChanged = next.fr24Email !== saved.fr24Email || next.fr24Password !== saved.fr24Password;
    try {
      saved = await saveKeys(next);
      form = { ...saved };
      if (loginChanged && saved.fr24Email && saved.fr24Password) {
        await onsignin(saved.fr24Email, saved.fr24Password);
        done += ", signed in to Flightradar24";
      }
      status = { text: done, error: false };
    } catch (e) {
      status = { text: message(e), error: true };
    } finally {
      busy = false;
    }
  }

  async function signOut() {
    busy = true;
    try {
      await onsignout();
      // signing out forgets the saved password too, or the next launch signs in again
      saved = await saveKeys({ ...saved, fr24Password: "" });
      form = { ...form, fr24Password: "" };
      status = { text: "Signed out", error: false };
    } catch (e) {
      status = { text: message(e), error: true };
    } finally {
      busy = false;
    }
  }

  async function exportKeys() {
    const name = "rustradar-keys.json";
    const contents = keysFile(form, withLogin);
    try {
      if (isTauri()) {
        const path = await invoke<string>("save_export", { fileName: name, contents });
        revealItemInDir(path).catch(() => {});
        status = { text: `Saved ${path.split(/[\\/]/).pop()} to Downloads`, error: false };
      } else {
        const url = URL.createObjectURL(new Blob([contents], { type: "application/json" }));
        Object.assign(document.createElement("a"), { href: url, download: name }).click();
        setTimeout(() => URL.revokeObjectURL(url), 10_000);
        status = { text: `Saved ${name}`, error: false };
      }
    } catch (e) {
      status = { text: `Couldn't export: ${message(e)}`, error: true };
    }
  }

  async function importKeys(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    try {
      const { keys, count } = readKeysFile(await file.text(), form);
      await save(keys, `Imported ${count} ${count === 1 ? "key" : "keys"}`);
    } catch (err) {
      status = { text: message(err), error: true };
    }
  }

  /** In the app, a link opens in the browser rather than in place of the map. */
  function external(e: MouseEvent) {
    if (!isTauri()) return;
    e.preventDefault();
    openUrl((e.currentTarget as HTMLAnchorElement).href).catch(() => {});
  }

  function submit(e: SubmitEvent) {
    e.preventDefault();
    save();
  }
</script>

<!-- a click on the backdrop lands on the dialog itself; the content fills it otherwise -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<dialog bind:this={dialog} class="surface" aria-labelledby="keys-heading" {onclose} onclick={(e) => e.target === dialog && dialog.close()}>
  <form class="content" onsubmit={submit}>
    <header>
      <h2 id="keys-heading">API keys</h2>
      <button type="button" class="icon-button" aria-label="Close" onclick={() => dialog.close()}><X size={16} strokeWidth={1.75} /></button>
    </header>

    <fieldset>
      <legend>
        Flightradar24
        {#if signedIn}<span class="badge good">Signed in</span>{/if}
      </legend>
      <p class="note">
        Optional. A subscription unlocks filed flight plans, vertical speed, squawk and airspace, depending on your plan.
      </p>
      <label class="field">
        <span>Email</span>
        <input type="email" autocomplete="username" bind:value={form.fr24Email} disabled={busy} bind:this={emailField} />
      </label>
      <label class="field">
        <span>Password</span>
        <input type="password" autocomplete="current-password" bind:value={form.fr24Password} disabled={busy} />
      </label>
      {#if signedIn}
        <button type="button" class="button small" onclick={signOut} disabled={busy}>Sign out</button>
      {/if}
    </fieldset>

    <fieldset>
      <legend>Trafikverket</legend>
      <p class="note">
        Every train in Sweden, SJ included. Free key from
        <a href="https://api.trafikinfo.trafikverket.se/" target="_blank" rel="noreferrer" onclick={external}>api.trafikinfo.trafikverket.se</a>.
      </p>
      <label class="field">
        <span>API key</span>
        <input autocomplete="off" spellcheck="false" bind:value={form.trafikverket} disabled={busy} />
      </label>
    </fieldset>

    <p class="status" class:error={status?.error} role="status">{status?.text ?? ""}</p>

    <div class="actions">
      <button class="button primary" type="submit" disabled={busy || !changed}>
        {#if busy}<span class="spin"><LoaderCircle size={14} strokeWidth={2} /></span>{/if}
        {hasLogin && (form.fr24Email !== saved.fr24Email || form.fr24Password !== saved.fr24Password) ? "Save and sign in" : "Save"}
      </button>
      <span class="spacer"></span>
      <button type="button" class="button" onclick={() => fileInput?.click()} disabled={busy}>Import…</button>
      <button type="button" class="button" onclick={exportKeys} disabled={busy}>Export</button>
    </div>
    <label class="check">
      <input type="checkbox" bind:checked={withLogin} />
      Include the Flightradar24 login in the export
    </label>
    <p class="fine">
      Kept on this device in plain text. An exported file holds the keys as they are: share it only with people you'd
      give the keys to.
    </p>
    <input type="file" accept=".json,application/json" hidden bind:this={fileInput} onchange={importKeys} />
  </form>
</dialog>

<style>
  dialog {
    width: 440px;
    max-width: calc(100vw - 32px);
    max-height: calc(100dvh - 32px);
    padding: 0;
    color: var(--text-1);
  }
  dialog::backdrop {
    background: oklch(0.1 0 0 / 0.5);
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
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4) var(--space-4);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  h2 {
    font-size: var(--text-base);
    font-weight: 650;
  }
  fieldset {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin: 0;
    padding: var(--space-3) 0 0;
    border: 0;
    border-top: 1px solid var(--line);
  }
  legend {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: 0 var(--space-2) 0 0;
    font-size: var(--text-md);
    font-weight: 650;
  }
  .note {
    font-size: var(--text-sm);
    color: var(--text-2);
  }
  .note a {
    color: var(--amber);
  }
  .fine {
    font-size: var(--text-xs);
    color: var(--text-3);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-2);
  }
  .field input {
    height: 34px;
    padding: 0 var(--space-3);
    border: 1px solid var(--line-control);
    border-radius: var(--radius-sm);
    background: var(--ink);
    font-family: inherit;
    font-size: var(--text-md);
    font-weight: 400;
    color: var(--text-1);
    outline: none;
  }
  .field input:focus {
    border-color: var(--amber);
  }
  .small {
    align-self: flex-start;
  }
  .status {
    min-height: 1.2em;
    font-size: var(--text-sm);
    color: var(--live);
    overflow-wrap: anywhere;
  }
  .status.error {
    color: var(--danger);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .spacer {
    flex: 1;
  }
  .check {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-sm);
    color: var(--text-2);
  }
  .primary {
    justify-content: center;
    border-color: transparent;
    background: var(--amber);
    color: var(--amber-ink);
  }
  .primary:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .spin {
    display: inline-grid;
    animation: spin 900ms linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
