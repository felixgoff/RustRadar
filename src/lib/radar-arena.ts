// The lifted RainViewer layers, kept on the GPU between tile changes.
//
// Each height is one layer over every tile in view, so the stack blends in
// height order across tile borders (one layer per tile would draw a seam
// where two tiles' stacks meet). Merging the tiles into fresh arrays each
// time the view brought in or dropped a tile re-uploaded every tile at every
// height, some 40 MB for a stormy tilted view, several times a second while
// panning: that was the stutter. Here each height's layer draws from buffers
// it owns and edits in place: a tile coming into view is appended, one leaving
// has its triangles collapsed, and only when the buffers fill up are they
// packed afresh.

import { Buffer, type Device } from "@luma.gl/core";
import { SolidPolygonLayer } from "@deck.gl/layers";
import { BAND_COUNT, type RadarMesh } from "./radar-mesh";

/** A mesh's bands from `fromBand` up: one tile's share of a layer. */
export interface ArenaPiece {
  mesh: RadarMesh;
  fromBand: number;
}

/** Binary SolidPolygonLayer data whose attributes are the arena's GPU buffers. */
export interface ArenaData {
  length: 1;
  startIndices: Uint32Array;
  attributes: {
    getPolygon: { buffer: Buffer; size: 2 | 3; type: "float32"; stride: number };
    getFillColor: { buffer: Buffer; size: 4; type: "unorm8"; normalized: true };
    indices: { buffer: Buffer };
    /**
     * Present so deck.gl's tesselator skips its own copy of the positions:
     * with `_normalize: false` it never fills it, but would allocate 24 bytes
     * for every vertex of the buffers.
     */
    positions: undefined;
  };
}

/** Packed buffers are this much larger than what they hold, leaving room to append. */
const GROWTH = 2;
/** Smallest buffers, in vertices (indices get three times as many), so a light shower doesn't reallocate on every tile. */
const MIN_VERTICES = 1 << 16;
/** Buffers for `vertices` and `indices`: what a pack allocates. */
const capacityFor = (vertices: number, indices: number) => ({
  vertices: Math.max(MIN_VERTICES, Math.ceil(vertices * GROWTH)),
  indices: Math.max(3 * MIN_VERTICES, Math.ceil(indices * GROWTH)),
});

type Slot = { vertex: number; index: number; indexCount: number };

const span = ({ mesh, fromBand }: ArenaPiece) => {
  const band = Math.min(Math.max(0, fromBand), BAND_COUNT);
  const index = mesh.bandStarts[band];
  const vertex = mesh.bandVertexStarts[band];
  return { index, vertex, indexCount: mesh.indices.length - index, vertexCount: mesh.positions.length / mesh.size - vertex };
};

/** One layer's buffers and which tile sits where in them. */
export class MeshArena {
  /** The layer's data; a new object only when the buffers are reallocated. */
  data: ArenaData | null = null;
  /** Indices in use, up to the last appended piece: what the layer draws. */
  indexCount = 0;
  /** Counts changes to the buffers: deck.gl can't see them, so the layer is told. */
  version = 0;
  private positions: Buffer | null = null;
  private colors: Buffer | null = null;
  private indices: Buffer | null = null;
  private vertexCapacity = 0;
  private indexCapacity = 0;
  private vertexEnd = 0;
  private slots = new Map<object, Slot>();

  constructor(
    private device: Device,
    private size: 2 | 3,
  ) {}

  /** Makes the buffers hold exactly `pieces` (keyed by tile), uploading only what is new to them. */
  update(pieces: Map<object, ArenaPiece>) {
    const version = this.version;
    for (const [key, slot] of this.slots) {
      if (pieces.has(key)) continue;
      // collapsed onto vertex 0: drawn as nothing, and only these indices are written
      this.indices!.write(new Uint32Array(slot.indexCount), slot.index * 4);
      this.slots.delete(key);
      this.version = version + 1;
    }
    let vertices = this.vertexEnd;
    let indices = this.indexCount;
    let liveVertices = 0;
    let liveIndices = 0;
    for (const [key, piece] of pieces) {
      const { indexCount, vertexCount } = span(piece);
      liveVertices += vertexCount;
      liveIndices += indexCount;
      if (this.slots.has(key)) continue;
      vertices += vertexCount;
      indices += indexCount;
    }
    // packed when full, or when what is left would fit in far smaller buffers
    // (so collapsed triangles don't pile up), or freed when nothing is left
    const fit = capacityFor(liveVertices, liveIndices);
    const full = vertices > this.vertexCapacity || indices > this.indexCapacity;
    const shrink = this.vertexCapacity > 2 * fit.vertices || this.indexCapacity > 2 * fit.indices;
    if (full || shrink || (this.data && !liveIndices)) {
      this.pack(pieces);
      this.version = version + 1;
      return;
    }
    for (const [key, piece] of pieces) if (!this.slots.has(key)) this.append(key, piece);
  }

  /** Writes every piece afresh from the start, in buffers sized for them with room to grow. */
  private pack(pieces: Map<object, ArenaPiece>) {
    let vertices = 0;
    let indices = 0;
    for (const piece of pieces.values()) {
      const { indexCount, vertexCount } = span(piece);
      vertices += vertexCount;
      indices += indexCount;
    }
    const stale = this.indexCount;
    this.slots.clear();
    this.vertexEnd = this.indexCount = 0;
    if (!indices) {
      this.destroy();
      return;
    }
    const { vertices: vertexCapacity, indices: indexCapacity } = capacityFor(vertices, indices);
    // the same buffers do if they leave room to append and aren't far too big
    const keep =
      this.data &&
      this.vertexCapacity >= vertexCapacity &&
      this.indexCapacity >= indexCapacity &&
      this.vertexCapacity <= 2 * vertexCapacity &&
      this.indexCapacity <= 2 * indexCapacity;
    if (!keep) {
      this.destroy();
      this.vertexCapacity = vertexCapacity;
      this.indexCapacity = indexCapacity;
      // WebGL fills new buffers with zeros: indices not yet written draw nothing
      this.positions = this.device.createBuffer({ usage: Buffer.VERTEX | Buffer.COPY_DST, byteLength: vertexCapacity * this.size * 4 });
      this.colors = this.device.createBuffer({ usage: Buffer.VERTEX | Buffer.COPY_DST, byteLength: vertexCapacity * 4 });
      this.indices = this.device.createBuffer({
        usage: Buffer.INDEX | Buffer.COPY_DST,
        indexType: "uint32",
        byteLength: indexCapacity * 4,
      });
      this.data = {
        length: 1,
        startIndices: new Uint32Array([0, vertexCapacity]),
        attributes: {
          // stride given: deck.gl would take the attribute's own float64 element size
          getPolygon: { buffer: this.positions, size: this.size, type: "float32", stride: this.size * 4 },
          getFillColor: { buffer: this.colors, size: 4, type: "unorm8", normalized: true },
          indices: { buffer: this.indices },
          positions: undefined,
        },
      };
    }
    for (const [key, piece] of pieces) this.append(key, piece);
    // what was drawn past the new end is no longer
    if (keep && stale > this.indexCount) this.indices!.write(new Uint32Array(stale - this.indexCount), this.indexCount * 4);
  }

  private append(key: object, piece: ArenaPiece) {
    const { mesh } = piece;
    const { index, vertex, indexCount, vertexCount } = span(piece);
    if (!indexCount) return;
    const at: Slot = { vertex: this.vertexEnd, index: this.indexCount, indexCount };
    // the tile's own arrays, written as they are; only the indices need moving
    this.positions!.write(mesh.positions.subarray(vertex * this.size, (vertex + vertexCount) * this.size), at.vertex * this.size * 4);
    this.colors!.write(mesh.colors.subarray(vertex * 4, (vertex + vertexCount) * 4), at.vertex * 4);
    const shifted = new Uint32Array(indexCount);
    const shift = at.vertex - vertex;
    for (let i = 0; i < indexCount; i++) shifted[i] = mesh.indices[index + i] + shift;
    this.indices!.write(shifted, at.index * 4);
    this.slots.set(key, at);
    this.vertexEnd += vertexCount;
    this.indexCount += indexCount;
    this.version++;
  }

  destroy() {
    for (const buffer of [this.positions, this.colors, this.indices]) buffer?.destroy();
    this.positions = this.colors = this.indices = null;
    this.data = null;
    this.vertexCapacity = this.indexCapacity = 0;
    this.slots.clear();
    this.vertexEnd = this.indexCount = 0;
  }
}

/**
 * A SolidPolygonLayer over a `MeshArena`: it draws the arena's `indexCount`
 * indices. The arena changes its buffers in place, so the data stays the same
 * object and deck.gl uploads nothing; a new `version` is what tells it to draw
 * again.
 */
export class ArenaLayer extends SolidPolygonLayer<unknown, { indexCount: number; version: number }> {
  static layerName = "ArenaLayer";

  draw(opts: Parameters<SolidPolygonLayer["draw"]>[0]) {
    // the external index buffer has no array to count, so the tesselator can't
    this.state.polygonTesselator.vertexCount = this.props.indexCount;
    super.draw(opts);
  }
}
