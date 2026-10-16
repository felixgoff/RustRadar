/**
 * Streets GL (streets.gl, OpenStreetMap in realistic 3D) at the map's view:
 * its address takes `#lat,lon,pitch,yaw,distance` — the camera's pitch up
 * from the ground and its compass heading in degrees, and how far it is from
 * the point it looks at, metres.
 */

/** Streets GL's camera's field of view, degrees (about the map's own). */
const FOV_DEG = 40;
/** Its camera keeps between these pitches, degrees. */
const MIN_PITCH = 5;
const MAX_PITCH = 89.99;

export function streetsGlUrl(
  view: { longitude: number; latitude: number; zoom: number; bearing: number; pitch: number },
  heightPx: number,
): string {
  // metres a pixel shows (deck.gl's 512-pixel tiles), the screen's height on the ground, and the camera's distance to see it
  const mpp = (40_075_016 * Math.cos((view.latitude * Math.PI) / 180)) / (512 * 2 ** view.zoom);
  const distance = (mpp * heightPx) / (2 * Math.tan(((FOV_DEG / 2) * Math.PI) / 180));
  // the map's pitch is from straight down; Streets GL's up from the ground
  const pitch = Math.max(MIN_PITCH, Math.min(MAX_PITCH, 90 - view.pitch));
  const yaw = ((view.bearing % 360) + 360) % 360;
  const parts = [view.latitude.toFixed(5), view.longitude.toFixed(5), pitch.toFixed(2), yaw.toFixed(2), distance.toFixed(2)];
  return `https://streets.gl/#${parts.join(",")}`;
}
