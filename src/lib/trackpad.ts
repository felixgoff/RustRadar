/**
 * Trackpad gestures, as Apple Maps reads them: two fingers moved together
 * pan the map (a mouse wheel still zooms), pinching zooms about the
 * fingers and twisting turns it.
 */

/**
 * Whether a wheel event comes from a trackpad's two-finger scroll rather
 * than a mouse wheel's notches: sideways movement, WebKit's `wheelDeltaY`
 * at three times the pixel delta (a wheel's are multiples of 120), or pixel
 * deltas that aren't whole lines.
 */
export function isTrackpadScroll(e: Pick<WheelEvent, "deltaX" | "deltaY" | "deltaMode" | "ctrlKey">): boolean {
  if (e.ctrlKey) return false;
  if (e.deltaX !== 0) return true;
  const legacy = (e as { wheelDeltaY?: number }).wheelDeltaY;
  if (legacy !== undefined && legacy !== 0) return legacy === -3 * e.deltaY;
  return e.deltaMode === 0 && !Number.isInteger(e.deltaY);
}

/** A wheel event's movement in pixels (line and page deltas scaled to them). */
export function wheelPixels(e: Pick<WheelEvent, "deltaX" | "deltaY" | "deltaMode">, pageHeight: number): [number, number] {
  const k = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? pageHeight : 1;
  return [e.deltaX * k, e.deltaY * k];
}

/** A bearing in deck.gl's range, -180 to 180 degrees. */
export function wrapBearing(b: number): number {
  return (((b % 360) + 540) % 360) - 180;
}

/** WebKit's pinch-and-twist events (Safari, and the app's web view on macOS). */
export type PinchEvent = UIEvent & { scale: number; rotation: number; clientX: number; clientY: number };

/** The zoom and bearing a pinch has reached from where it started. */
export function pinched(start: { zoom: number; bearing: number }, e: { scale: number; rotation: number }) {
  // fingers apart doubles the scale for each zoom level; turned clockwise, the map turns with them
  return { zoom: start.zoom + Math.log2(Math.max(0.01, e.scale)), bearing: wrapBearing(start.bearing - e.rotation) };
}
