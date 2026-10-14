// Whether the window is phone-sized: the map fills the screen and the lists
// and panels move into bottom sheets. Narrow, or short as a phone on its side.
// The components' CSS uses the same query: keep them in step.
export const PHONE_QUERY = "(max-width: 899px), (max-height: 500px)";

export const layout = $state({ phone: false });

if (typeof matchMedia !== "undefined") {
  const mq = matchMedia(PHONE_QUERY);
  layout.phone = mq.matches;
  mq.addEventListener("change", () => (layout.phone = mq.matches));
}
