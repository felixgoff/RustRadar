// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import type { Device } from "@luma.gl/core";
import { MeshArena, type ArenaPiece } from "./radar-arena";
import { buildMesh, buildWalls, mergeMeshes, type BandPolygon, type PolygonData, type RadarMesh } from "./radar-mesh";

/** A stand-in for luma.gl's buffers: plain memory, counting what is written. */
class FakeBuffer {
  bytes: Uint8Array;
  destroyed = false;
  constructor(
    public byteLength: number,
    private device: FakeDevice,
  ) {
    this.bytes = new Uint8Array(byteLength); // zeroed, as WebGL's are
  }
  write(data: ArrayBufferView, byteOffset = 0) {
    expect(byteOffset + data.byteLength).toBeLessThanOrEqual(this.byteLength);
    this.bytes.set(new Uint8Array(data.buffer, data.byteOffset, data.byteLength), byteOffset);
    this.device.written += data.byteLength;
  }
  destroy() {
    this.destroyed = true;
  }
}
class FakeDevice {
  written = 0;
  buffers: FakeBuffer[] = [];
  createBuffer({ byteLength }: { byteLength: number }) {
    const buffer = new FakeBuffer(byteLength, this);
    this.buffers.push(buffer);
    return buffer;
  }
}

/** A square of `band` at (x, y), a degree across. */
const square = (band: number, x: number, y: number): BandPolygon => ({
  band,
  positions: [x, y, x + 1, y, x + 1, y + 1, x, y + 1],
  holeIndices: [],
});
/** A tile's mesh: a few squares in several bands, offset so tiles differ. */
const tile = (offset: number, count = 6): RadarMesh =>
  buildMesh(Array.from({ length: count }, (_, i) => square(i % 4, offset + 2 * i, offset)));

type Tri = string;
/** The triangles drawn, as position and colour per corner; collapsed ones left out. */
function drawn(arena: MeshArena, size: number): Tri[] {
  const at = (b: unknown) => (b as FakeBuffer).bytes.buffer;
  const { getPolygon, getFillColor, indices } = arena.data!.attributes;
  const positions = new Float32Array(at(getPolygon.buffer));
  const colors = new Uint8Array(at(getFillColor.buffer));
  const index = new Uint32Array(at(indices.buffer)).subarray(0, arena.indexCount);
  return triangles(index, positions, colors, size);
}
function merged(parts: ArenaPiece[], size: number): Tri[] {
  const data = mergeMeshes(parts) as PolygonData;
  return triangles(data.attributes.indices, data.attributes.getPolygon.value, data.attributes.getFillColor.value, size);
}
function triangles(index: Uint32Array, positions: Float32Array, colors: Uint8Array, size: number): Tri[] {
  const out: Tri[] = [];
  for (let t = 0; t < index.length; t += 3) {
    const corners = [index[t], index[t + 1], index[t + 2]];
    if (corners[0] === corners[1] && corners[1] === corners[2]) continue; // collapsed
    out.push(corners.map((v) => [...positions.subarray(v * size, v * size + size), ...colors.subarray(v * 4, v * 4 + 4)].join()).join("|"));
  }
  return out;
}
const sorted = (a: Tri[]) => [...a].sort();

describe("MeshArena", () => {
  const [a, b, c] = [tile(0), tile(20), tile(40)];

  test("draws the same triangles, in the same order, as merging the tiles", () => {
    const device = new FakeDevice();
    const arena = new MeshArena(device as unknown as Device, 2);
    const pieces = new Map([
      [a, { mesh: a, fromBand: 1 }],
      [b, { mesh: b, fromBand: 1 }],
    ]);
    arena.update(pieces);
    expect(drawn(arena, 2)).toEqual(merged([...pieces.values()], 2));
  });

  test("a tile coming into view is appended, one leaving collapsed, and the rest left alone", () => {
    const device = new FakeDevice();
    const arena = new MeshArena(device as unknown as Device, 2);
    arena.update(new Map([[a, { mesh: a, fromBand: 0 }], [b, { mesh: b, fromBand: 0 }]]));
    const data = arena.data;
    const version = arena.version;
    device.written = 0;
    arena.update(new Map([[b, { mesh: b, fromBand: 0 }], [c, { mesh: c, fromBand: 0 }]]));
    // the same buffers: deck.gl has nothing to upload
    expect(arena.data).toBe(data);
    expect(arena.version).not.toBe(version);
    expect(sorted(drawn(arena, 2))).toEqual(sorted(merged([{ mesh: b, fromBand: 0 }, { mesh: c, fromBand: 0 }], 2)));
    // only the new tile and the leaving tile's indices were written, not `b`
    const bytes = (m: RadarMesh) => m.positions.byteLength + m.colors.byteLength + m.indices.byteLength;
    expect(device.written).toBe(bytes(c) + a.indices.byteLength);
  });

  test("nothing changes for the same tiles", () => {
    const device = new FakeDevice();
    const arena = new MeshArena(device as unknown as Device, 2);
    const pieces = new Map([[a, { mesh: a, fromBand: 0 }]]);
    arena.update(pieces);
    const version = arena.version;
    device.written = 0;
    arena.update(new Map(pieces));
    expect(device.written).toBe(0);
    expect(arena.version).toBe(version);
  });

  test("walls keep their height and are packed afresh once the buffers are full", () => {
    const device = new FakeDevice();
    const arena = new MeshArena(device as unknown as Device, 3);
    // a long ring: enough vertices to outgrow the smallest buffers in a few tiles
    const ring = (offset: number) =>
      buildWalls([{ band: 2, positions: Array.from({ length: 30_000 }, (_, i) => offset + (i % 2 ? i * 1e-4 : i * 2e-4)) }]);
    const meshes = Array.from({ length: 6 }, (_, i) => ring(i * 10));
    let data = null;
    let reallocated = 0;
    for (let i = 0; i < meshes.length; i++) {
      // a sliding window of two tiles, as in a pan
      const pieces = new Map(meshes.slice(Math.max(0, i - 1), i + 1).map((m) => [m, { mesh: m, fromBand: 0 }]));
      arena.update(pieces);
      if (arena.data !== data) reallocated++;
      data = arena.data;
      expect(sorted(drawn(arena, 3))).toEqual(sorted(merged([...pieces.values()], 3)));
    }
    expect(reallocated).toBeGreaterThan(1);
    // the replaced buffers were released
    expect(device.buffers.filter((b) => !b.destroyed).length).toBe(3);
  });

  test("bands below `fromBand` are left out", () => {
    const device = new FakeDevice();
    const arena = new MeshArena(device as unknown as Device, 2);
    arena.update(new Map([[a, { mesh: a, fromBand: 3 }]]));
    expect(drawn(arena, 2)).toEqual(merged([{ mesh: a, fromBand: 3 }], 2));
    expect(drawn(arena, 2).length).toBeLessThan(merged([{ mesh: a, fromBand: 0 }], 2).length);
  });

  test("an empty view frees the buffers", () => {
    const device = new FakeDevice();
    const arena = new MeshArena(device as unknown as Device, 2);
    arena.update(new Map([[a, { mesh: a, fromBand: 0 }]]));
    arena.update(new Map());
    expect(arena.data).toBeNull();
    expect(device.buffers.every((b) => b.destroyed)).toBe(true);
  });
});
