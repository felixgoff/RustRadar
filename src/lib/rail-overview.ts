// The railway network zoomed out. The map's vector tiles carry railways only
// from zoom 8, so a whole country (Sweden, say) showed no rail at all. Below
// that the network comes from Natural Earth's 1:10m railroads (public domain),
// simplified to about a kilometre and shipped with the app as
// static/rail/railroads.bin; see scripts/build-rail-overview.py for the format.

import { PathLayer } from "@deck.gl/layers";

type RGBA = [number, number, number, number];

export interface RailOverview {
  length: number;
  startIndices: Uint32Array;
  /** lon, lat pairs. */
  positions: Float32Array;
  /** Natural Earth's category per path: 1 main lines, 2 and 3 lesser ones. */
  categories: Uint8Array;
}

/** Parses the packed file; throws on anything that isn't one. */
export function parseRailOverview(buffer: ArrayBuffer): RailOverview {
  const view = new DataView(buffer);
  const magic = String.fromCharCode(...new Uint8Array(buffer, 0, 4));
  if (magic !== "RRO1") throw new Error("not a rail overview file");
  const paths = view.getUint32(4, true);
  const points = view.getUint32(8, true);
  const headerEnd = 12 + paths * 3;
  if (buffer.byteLength !== headerEnd + points * 4) throw new Error("rail overview file is truncated");
  const startIndices = new Uint32Array(paths + 1);
  const categories = new Uint8Array(paths);
  let at = 0;
  for (let i = 0; i < paths; i++) {
    categories[i] = view.getUint8(12 + i * 3);
    startIndices[i] = at;
    at += view.getUint16(12 + i * 3 + 1, true);
  }
  startIndices[paths] = at;
  if (at !== points) throw new Error("rail overview point count disagrees");
  const positions = new Float32Array(points * 2);
  for (let i = 0; i < points * 2; i++) positions[i] = view.getInt16(headerEnd + i * 2, true) / 100;
  return { length: paths, startIndices, positions, categories };
}

let overview: RailOverview | null = null;
let loading: Promise<void> | null = null;

/** The overview once loaded; starts loading it on first call. Null meanwhile (or if it can't be had). */
export function railOverview(): RailOverview | null {
  loading ??= fetch("/rail/railroads.bin")
    .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error(`HTTP ${r.status}`))))
    .then((b) => {
      overview = parseRailOverview(b);
    })
    .catch(() => {
      // drawn without it: the vector tiles still have rail from zoom 8
    });
  return overview;
}

const SURFACE_NO_CULL = { cullMode: "none", depthWriteEnabled: false } as const;

/** Zoomed out, the network from the overview, in the given colours by category. */
export function railOverviewLayer(data: RailOverview, colors: { main: RGBA; minor: RGBA }, flat: boolean) {
  const color = (i: number): RGBA => (data.categories[i] === 1 ? colors.main : colors.minor);
  return new PathLayer<number>({
    id: flat ? "rail-overview-flat" : "rail-overview-globe",
    data: {
      length: data.length,
      startIndices: data.startIndices,
      attributes: { getPath: { value: data.positions, size: 2 } },
    } as never,
    _pathType: "open",
    getColor: (_: number, { index }: { index: number }) => color(index),
    getWidth: (_: number, { index }: { index: number }) => (data.categories[index] === 1 ? 1.5 : 1),
    widthUnits: "pixels",
    parameters: SURFACE_NO_CULL,
  });
}
