// Real aircraft shapes, built from the Flightradar24 3D models by
// `scripts/aircraft/build.mjs` and shipped in `static/aircraft/`.
//
// Each type comes at two levels of detail (a couple of hundred triangles for
// the wide view, a couple of thousand up close) plus a top-down silhouette
// used when the map is flat. Geometry is in metres in RustRadar's frame:
// nose along +y, right wing +x, up +z.
import { Geometry } from "@luma.gl/engine";
import type { Part } from "./liveries";

const BASE = "/aircraft";

interface PackedLod {
  /** Byte offsets into `meshes.bin`, each 4-byte aligned. */
  positions: number;
  normals: number;
  indices: number;
  vertices: number;
  triangles: number;
  /** Livery part to its [first triangle, count] run of the index buffer. */
  parts: Partial<Record<Part, [number, number]>>;
}

interface PackedModel {
  /** Largest dimension in metres. */
  size: number;
  span: number;
  length: number;
  lods: PackedLod[];
}

interface Index {
  models: Record<string, PackedModel>;
  /** ICAO type designator to model name. */
  types: Record<string, string>;
  /** Flightradar24 icon class to model name, for types not in `types`. */
  icons: Record<string, string>;
  lods: number[];
  icon: { cell: number; columns: number; names: string[] };
}

export interface ModelLod {
  /** Every triangle, for drawing an aircraft in one colour. */
  combined: Geometry;
  /** Triangles grouped by livery part. */
  parts: [Part, Geometry][];
}

export interface AircraftModel {
  name: string;
  size: number;
  lods: ModelLod[];
}

/** Levels of detail, coarsest first. */
export const LOD_COUNT = 2;

function geometries(buffer: ArrayBuffer, lod: PackedLod): ModelLod {
  const { vertices, triangles } = lod;
  const positions = new Float32Array(buffer, lod.positions, vertices * 3);
  const packed = new Int8Array(buffer, lod.normals, vertices * 3);
  const indices = new Uint16Array(buffer, lod.indices, triangles * 3);
  // normals ship as signed bytes; the GPU wants floats
  const normals = Float32Array.from(packed, (v) => v / 127);
  const attributes = { positions: { size: 3, value: positions }, normals: { size: 3, value: normals } };
  const build = (subset: Uint16Array) =>
    new Geometry({ topology: "triangle-list", indices: subset, attributes });
  return {
    combined: build(indices),
    parts: Object.entries(lod.parts).map(([part, [start, count]]) => [
      part as Part,
      build(indices.subarray(start * 3, (start + count) * 3)),
    ]),
  };
}

class AircraftModels {
  /** Resolved once the meshes and the silhouette atlas are ready to draw. */
  loaded = false;
  iconAtlas: string | null = null;
  iconMapping: Record<string, { x: number; y: number; width: number; height: number; mask: boolean }> = {};

  private models = new Map<string, AircraftModel>();
  private byType = new Map<string, string>();
  private byIcon = new Map<string, string>();
  private loading: Promise<void> | null = null;

  load(): Promise<void> {
    this.loading ??= this.read().catch((e) => {
      // the procedural models stay as the fallback; nothing else breaks
      console.warn("aircraft models unavailable:", e);
    });
    return this.loading;
  }

  private async read() {
    const [index, buffer] = await Promise.all([
      fetch(`${BASE}/index.json`).then((r) => r.json() as Promise<Index>),
      fetch(`${BASE}/meshes.bin`).then((r) => r.arrayBuffer()),
    ]);
    for (const [name, packed] of Object.entries(index.models)) {
      this.models.set(name, { name, size: packed.size, lods: packed.lods.map((lod) => geometries(buffer, lod)) });
    }
    for (const [type, name] of Object.entries(index.types)) this.byType.set(type, name);
    for (const [icon, name] of Object.entries(index.icons)) this.byIcon.set(icon, name);

    const { cell, columns, names } = index.icon;
    names.forEach((name, i) => {
      this.iconMapping[name] = {
        x: (i % columns) * cell,
        y: Math.floor(i / columns) * cell,
        width: cell,
        height: cell,
        mask: true, // a white alpha mask, so `getColor` tints it
      };
    });
    this.iconAtlas = `${BASE}/icons.png`;
    this.loaded = true;
  }

  /** The closest published model for a flight, or `null` to fall back. */
  forFlight(typecode: string, icon: string): AircraftModel | null {
    const name = this.byType.get(typecode.toUpperCase()) ?? this.byIcon.get(icon);
    return name ? (this.models.get(name) ?? null) : null;
  }
}

export const aircraftModels = new AircraftModels();
