// See https://svelte.dev/docs/kit/types#app.d.ts
declare global {
  namespace App {
    interface PageState {
      /** A history entry for Android's back gesture to close something with, rather than leave the app. */
      back?: boolean;
    }
  }
}

export {};
