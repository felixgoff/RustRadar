<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { RefreshCw, X } from "@lucide/svelte";
  import { onMount } from "svelte";
  import { isTauri } from "./api";

  // A newer version found while the app is open is downloaded in the
  // background (updater.rs); this offers to restart into it. Launching the
  // app installs updates anyway, so "Later" only hides the offer for now.
  let version = $state<string | null>(null);
  let dismissed = $state<string | null>(null);
  let restarting = $state(false);
  let error = $state<string | null>(null);
  /** Android installs the new APK itself: the button opens its download rather than restarting. */
  const android = typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent);

  onMount(() => {
    if (!isTauri()) return;
    invoke<string | null>("pending_update").then((v) => v && (version = v), () => {});
    const unlisten = listen<string>("update-ready", (e) => (version = e.payload));
    return () => void unlisten.then((stop) => stop());
  });

  async function restart() {
    restarting = !android;
    error = null;
    try {
      await invoke("restart_to_update");
    } catch (e) {
      error = String(e).replace(/^Error:\s*/, "");
      restarting = false;
    }
  }
</script>

{#if version && dismissed !== version}
  <div class="toast surface" role="status" aria-live="polite">
    <span class="icon"><RefreshCw size={16} strokeWidth={1.75} /></span>
    <div class="text">
      <p class="title">RustRadar {version} is ready</p>
      <p class="sub">
        {error ?? (android ? "Download it, then open the file to install." : "Restart to start using it. It takes a few seconds.")}
      </p>
    </div>
    <button class="button primary" onclick={restart} disabled={restarting}>
      {#if android}Download{:else}{restarting ? "Restarting…" : "Restart"}{/if}
    </button>
    <button class="icon-button" aria-label="Later" title={android ? "Later" : "Later: it installs the next time RustRadar opens"} onclick={() => (dismissed = version)}>
      <X size={16} strokeWidth={1.75} />
    </button>
  </div>
{/if}

<style>
  .toast {
    position: fixed;
    left: var(--space-4);
    bottom: 40px;
    z-index: 30;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    max-width: min(440px, calc(100vw - 2 * var(--space-4)));
    padding: var(--space-3) var(--space-3) var(--space-3) var(--space-4);
    border-radius: var(--radius);
  }
  .icon {
    display: flex;
    color: var(--amber);
  }
  .text {
    min-width: 0;
  }
  .title {
    font-size: var(--text-md);
    font-weight: 600;
    color: var(--text-1);
  }
  .sub {
    font-size: var(--text-sm);
    color: var(--text-3);
  }
  .button.primary {
    border-color: var(--amber);
    background: var(--amber);
    color: var(--amber-ink);
  }
  .button.primary:hover:not(:disabled) {
    background: oklch(from var(--amber) calc(l + 0.05) c h);
  }
  .button:disabled {
    opacity: 0.7;
    cursor: default;
  }
</style>
