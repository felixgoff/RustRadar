// The clouds as they are now, the world over, for the realistic globe:
// EUMETSAT's world cloud map (EUMETView), its geostationary satellites'
// infrared at 10.8 µm stitched round the Earth every three hours: cold cloud
// tops bright, the warm ground dark (realistic.ts's CloudExtension turns the
// one into cloud and leaves the other clear). Asked for again every hour.

export const CLOUDS_ATTRIBUTION = "Clouds: © EUMETSAT";
/** The picture's edges: as far north and south as the flat map goes. */
export const CLOUD_BOUNDS: [number, number, number, number] = [-180, -85, 180, 85];
const URL =
  "https://view.eumetsat.int/geoserver/wms?service=WMS&version=1.3.0&request=GetMap&layers=mumi:worldcloudmap_ir108&styles=" +
  `&crs=EPSG:4326&bbox=${CLOUD_BOUNDS[1]},${CLOUD_BOUNDS[0]},${CLOUD_BOUNDS[3]},${CLOUD_BOUNDS[2]}&width=4096&height=1934&format=image/jpeg`;
const REFRESH_MS = 60 * 60_000;
/** After a failure, tried again this soon. */
const RETRY_MS = 5 * 60_000;

let image: ImageBitmap | null = null;
let askedAt = -Infinity;
let loading = false;

/** The latest cloud picture (null until one has come); fetches a newer one when it is due, and `onLoad` when it has. */
export function cloudPicture(onLoad: () => void): ImageBitmap | null {
  const now = Date.now();
  if (!loading && now - askedAt > (image ? REFRESH_MS : RETRY_MS)) {
    loading = true;
    askedAt = now;
    fetch(URL)
      .then((r) => (r.ok ? r.blob() : Promise.reject(new Error(`EUMETSAT answered ${r.status}`))))
      .then((b) => createImageBitmap(b))
      .then((next) => {
        image?.close();
        image = next;
        onLoad();
      })
      .catch(() => {})
      .finally(() => (loading = false));
  }
  return image;
}
