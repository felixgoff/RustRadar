// The trees towns keep registers of, in their streets and parks: Malmö's and
// Umeå's (both CC0), each where it stands, of its kind, Malmö's with their
// crowns' widths. static/street-trees/*.bin; see scripts/street-trees.py for
// the format.

/** A registered tree in a tile: where (0 to 1 across it, y down), its kind (Streets GL's tree, the format's), its crown's diameter (metres, 0 not known). */
export type RegisteredTree = { x: number; y: number; kind: number; crown: number };

/** The kinds, as the format has them. */
export const REGISTER_KINDS = { BROADLEAF: 0, CONIFER: 1, BEECH: 2, LINDEN: 3, OAK: 4 } as const;

export const STREET_TREES_ATTRIBUTION = "Street trees: Malmö stad, Umeå kommun (CC0)";

/** The towns, and round where their trees are (west, south, east, north). */
const TOWNS: { file: string; bounds: [number, number, number, number] }[] = [
  { file: "malmo", bounds: [12.8, 55.45, 13.3, 55.7] },
  { file: "umea", bounds: [19.6, 63.5, 20.9, 64.2] },
];

type Town = { lon: Int32Array; lat: Int32Array; kind: Uint8Array; crown: Uint8Array };

/** A town's file read, tree by tree (sorted by latitude). */
export function parseTown(buffer: ArrayBuffer): Town | null {
  const view = new DataView(buffer);
  if (buffer.byteLength < 8 || String.fromCharCode(...new Uint8Array(buffer, 0, 4)) !== "RST1") return null;
  const n = view.getUint32(4, true);
  if (buffer.byteLength < 8 + n * 10) return null;
  const town: Town = { lon: new Int32Array(n), lat: new Int32Array(n), kind: new Uint8Array(n), crown: new Uint8Array(n) };
  for (let i = 0, at = 8; i < n; i++, at += 10) {
    town.lon[i] = view.getInt32(at, true);
    town.lat[i] = view.getInt32(at + 4, true);
    town.kind[i] = view.getUint8(at + 8);
    town.crown[i] = view.getUint8(at + 9);
  }
  return town;
}

const towns = new Map<string, Promise<Town | null>>();
const townOf = (file: string) => {
  let hit = towns.get(file);
  if (!hit) {
    hit = fetch(`/street-trees/${file}.bin`)
      .then((r) => (r.ok ? r.arrayBuffer() : null))
      .then((b) => b && parseTown(b))
      .catch(() => null);
    towns.set(file, hit);
  }
  return hit;
};

/** The first index at or past `v` in a sorted list. */
function lowerBound(list: Int32Array, v: number): number {
  let [lo, hi] = [0, list.length];
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (list[mid] < v) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** A town's trees in a tile. */
export function treesInTile(town: Town, t: { x: number; y: number; z: number }): RegisteredTree[] {
  const n = 2 ** t.z;
  const latOf = (y: number) => (Math.atan(Math.sinh(Math.PI * (1 - (2 * y) / n))) * 180) / Math.PI;
  const [north, south] = [latOf(t.y), latOf(t.y + 1)];
  const [west, east] = [(t.x / n) * 360 - 180, ((t.x + 1) / n) * 360 - 180];
  const out: RegisteredTree[] = [];
  for (let i = lowerBound(town.lat, Math.floor(south * 1e6)); i < town.lat.length && town.lat[i] <= north * 1e6; i++) {
    const lon = town.lon[i] / 1e6;
    if (lon < west || lon >= east) continue;
    const r = ((town.lat[i] / 1e6) * Math.PI) / 180;
    const y = ((1 - Math.log(Math.tan(r) + 1 / Math.cos(r)) / Math.PI) / 2) * n - t.y;
    out.push({ x: ((lon + 180) / 360) * n - t.x, y, kind: town.kind[i], crown: town.crown[i] / 2 });
  }
  return out;
}

/** The registered trees in a tile (none where no town keeps a register). */
export async function streetTrees(t: { x: number; y: number; z: number }): Promise<RegisteredTree[]> {
  const n = 2 ** t.z;
  const [west, east] = [(t.x / n) * 360 - 180, ((t.x + 1) / n) * 360 - 180];
  const latOf = (y: number) => (Math.atan(Math.sinh(Math.PI * (1 - (2 * y) / n))) * 180) / Math.PI;
  const [north, south] = [latOf(t.y), latOf(t.y + 1)];
  const out: RegisteredTree[] = [];
  for (const town of TOWNS) {
    const [w, s, e, nn] = town.bounds;
    if (east < w || west > e || north < s || south > nn) continue;
    const trees = await townOf(town.file);
    if (trees) out.push(...treesInTile(trees, t));
  }
  return out;
}
