import type { Place, PlaceKind } from "./api";

/** How close to fly to each kind of place when it has no extent to fit. */
export const KIND_ZOOM: Record<PlaceKind, number> = {
  address: 17.5,
  poi: 16.5,
  street: 16,
  district: 14,
  postcode: 13,
  airport: 13,
  town: 12,
  city: 11,
  region: 7,
  country: 5,
};

/** Never closer than the globe's street level allows, nor further than the whole planet. */
const MIN_ZOOM = 2;
const MAX_ZOOM = 17.5;
/** An extent may move the zoom this far from its kind's (an overseas territory widens a country). */
const BELOW_KIND = 2;
const ABOVE_KIND = 1.5;
/** The share of the view a fitted extent fills. */
const FILL = 0.7;
/** deck.gl draws the world 512 px wide at zoom 0. */
const WORLD_PX = 512;

const clamp = (x: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, x));

/**
 * The zoom that shows `bbox` within a `width`×`height` view. deck.gl's globe
 * keeps Web Mercator's local scale (a degree of longitude is 512·2^zoom/360 px
 * at any latitude, a degree of latitude 1/cos(lat) times that), so this holds
 * on the globe and in the flat view above zoom 12 alike.
 */
export function fitZoom([west, south, east, north]: [number, number, number, number], width: number, height: number) {
  const lonSpan = Math.max(east >= west ? east - west : east + 360 - west, 1e-6); // across the antimeridian
  const midLat = Math.min(Math.abs((south + north) / 2), 85) * (Math.PI / 180);
  const latSpan = Math.max(north - south, 1e-6) / Math.cos(midLat);
  const scale = Math.min((width * FILL * 360) / (WORLD_PX * lonSpan), (height * FILL * 360) / (WORLD_PX * latSpan));
  return Math.log2(scale);
}

/** Where to fly for a place: its point, at a zoom fitted to its extent or set by its kind. */
export function placeView(place: Place, width: number, height: number) {
  const kindZoom = KIND_ZOOM[place.kind] ?? KIND_ZOOM.poi;
  const zoom = place.bbox
    ? clamp(fitZoom(place.bbox, width, height), kindZoom - BELOW_KIND, kindZoom + ABOVE_KIND)
    : kindZoom;
  return { lon: place.lon, lat: place.lat, zoom: clamp(zoom, MIN_ZOOM, MAX_ZOOM) };
}

const KIND_WORD: Record<PlaceKind, string> = {
  address: "Address",
  street: "Street",
  poi: "Place",
  airport: "Airport",
  district: "District",
  city: "City",
  town: "Town",
  postcode: "Postcode",
  region: "Region",
  country: "Country",
};

/** The muted second line: what it is, then where (the label without the name). */
export function placeDetail(place: Place) {
  const what = place.category ?? KIND_WORD[place.kind] ?? "Place";
  const prefix = `${place.name}, `;
  const where = place.label.startsWith(prefix) ? place.label.slice(prefix.length) : place.label === place.name ? "" : place.label;
  return where ? `${what} · ${where}` : what;
}

/** The interface language, if Photon has names in it; otherwise local names. */
export function placeLanguage(language = typeof navigator === "undefined" ? "" : navigator.language) {
  const primary = language.slice(0, 2).toLowerCase();
  return ["en", "de", "fr", "it"].includes(primary) ? primary : undefined;
}
