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

/** The clouds' height, metres: they are drawn there, faded out as the camera comes near them; the weather is the sun's above them. */
export const CLOUD_HEIGHT_M = 10_000;

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

type Strip = { bounds: [number, number, number][]; image: HTMLCanvasElement | OffscreenCanvas };
let stripsOf: { picture: ImageBitmap; strips: Map<number, Strip> } | null = null;

/**
 * The picture in strips a degree of latitude high, round `lat`, at the
 * clouds' height: on the flat map a picture is laid on as one quad and web
 * mercator would bend it out of place, a strip at a time only a little.
 */
export function cloudStrips(picture: ImageBitmap, lat: number, reach = 3): Strip[] {
  if (stripsOf?.picture !== picture) stripsOf = { picture, strips: new Map() };
  const [, south, , north] = CLOUD_BOUNDS;
  const rows = picture.height / (north - south);
  const out: Strip[] = [];
  for (let k = Math.floor(lat) - reach; k <= Math.floor(lat) + reach; k++) {
    const [a, b] = [Math.max(south, k), Math.min(north, k + 1)];
    if (b <= a) continue;
    let strip = stripsOf.strips.get(k);
    if (!strip) {
      const [y0, y1] = [Math.floor((north - b) * rows), Math.ceil((north - a) * rows)];
      const image = typeof OffscreenCanvas !== "undefined" ? new OffscreenCanvas(picture.width, y1 - y0) : Object.assign(document.createElement("canvas"), { width: picture.width, height: y1 - y0 });
      (image.getContext("2d") as CanvasRenderingContext2D | null)?.drawImage(picture, 0, y0, picture.width, y1 - y0, 0, 0, picture.width, y1 - y0);
      // (its edges where its rows are: a little past the degree)
      const [top, bottom] = [north - y0 / rows, north - y1 / rows];
      const h = CLOUD_HEIGHT_M;
      strip = { bounds: [[-180, bottom, h], [-180, top, h], [180, top, h], [180, bottom, h]], image };
      stripsOf.strips.set(k, strip);
    }
    out.push(strip);
  }
  return out;
}
