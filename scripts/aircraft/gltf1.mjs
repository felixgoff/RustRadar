// Reads the glTF 1.0 files published at github.com/Flightradar24/fr24-3d-models.
//
// glTF 1.0 predates the array-indexed layout of 2.0 (loaders.gl only speaks
// 2.0): every collection is an object keyed by id, and primitives point at
// accessors by name. Only geometry is read here — the models' own materials
// are COLLADA-era shader techniques, and RustRadar paints its own liveries.

const COMPONENT = {
  5120: Int8Array,
  5121: Uint8Array,
  5122: Int16Array,
  5123: Uint16Array,
  5125: Uint32Array,
  5126: Float32Array,
};
const COMPONENTS = { SCALAR: 1, VEC2: 2, VEC3: 3, VEC4: 4, MAT4: 16 };

/**
 * Splits a model into its JSON scene and binary body. Accepts both the GLB
 * container and a bare `.gltf`, which carries its buffers as data URIs.
 */
export function parseGlb(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (view.getUint32(0, false) !== 0x676c5446) {
    return { json: JSON.parse(new TextDecoder().decode(bytes)), body: new Uint8Array(0) };
  }
  const version = view.getUint32(4, true);
  if (version !== 1) throw new Error(`expected GLB version 1, got ${version}`);
  // 1.0 header: magic, version, length, contentLength, contentFormat
  const contentLength = view.getUint32(12, true);
  const json = JSON.parse(new TextDecoder().decode(bytes.subarray(20, 20 + contentLength)));
  return { json, body: bytes.subarray(20 + contentLength) };
}

const decodeDataUri = (uri) => Uint8Array.from(Buffer.from(uri.slice(uri.indexOf(",") + 1), "base64"));

/** Reads one accessor into a flat array, honouring its byte stride. */
function readAccessor(gltf, buffers, id) {
  const accessor = gltf.accessors[id];
  const view = gltf.bufferViews[accessor.bufferView];
  const Type = COMPONENT[accessor.componentType];
  const size = COMPONENTS[accessor.type];
  const buffer = buffers[view.buffer];
  const base = (view.byteOffset ?? 0) + (accessor.byteOffset ?? 0);
  const stride = accessor.byteStride || size * Type.BYTES_PER_ELEMENT;
  const out = new (Type === Float32Array ? Float32Array : Uint32Array)(accessor.count * size);
  for (let i = 0; i < accessor.count; i++) {
    const at = buffer.byteOffset + base + i * stride;
    const run = new Type(buffer.buffer.slice(at, at + size * Type.BYTES_PER_ELEMENT));
    for (let c = 0; c < size; c++) out[i * size + c] = run[c];
  }
  return out;
}

const multiply = (a, b) => {
  const m = new Float64Array(16);
  for (let c = 0; c < 4; c++) {
    for (let r = 0; r < 4; r++) {
      m[c * 4 + r] = a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1] + a[8 + r] * b[c * 4 + 2] + a[12 + r] * b[c * 4 + 3];
    }
  }
  return m;
};

function localMatrix(node) {
  if (node.matrix) return Float64Array.from(node.matrix);
  const [tx, ty, tz] = node.translation ?? [0, 0, 0];
  const [qx, qy, qz, qw] = node.rotation ?? [0, 0, 0, 1];
  const [sx, sy, sz] = node.scale ?? [1, 1, 1];
  const [x2, y2, z2] = [qx + qx, qy + qy, qz + qz];
  const [xx, xy, xz] = [qx * x2, qx * y2, qx * z2];
  const [yy, yz, zz] = [qy * y2, qy * z2, qz * z2];
  const [wx, wy, wz] = [qw * x2, qw * y2, qw * z2];
  return Float64Array.from([
    (1 - (yy + zz)) * sx, (xy + wz) * sx, (xz - wy) * sx, 0,
    (xy - wz) * sy, (1 - (xx + zz)) * sy, (yz + wx) * sy, 0,
    (xz + wy) * sz, (yz - wx) * sz, (1 - (xx + yy)) * sz, 0,
    tx, ty, tz, 1,
  ]);
}

const transform = (m, x, y, z) => [
  m[0] * x + m[4] * y + m[8] * z + m[12],
  m[1] * x + m[5] * y + m[9] * z + m[13],
  m[2] * x + m[6] * y + m[10] * z + m[14],
];

/** A direction through the same transform: rotation and scale, no translation. */
const rotate = (m, x, y, z) => {
  const v = [m[0] * x + m[4] * y + m[8] * z, m[1] * x + m[5] * y + m[9] * z, m[2] * x + m[6] * y + m[10] * z];
  const len = Math.hypot(...v) || 1;
  return [v[0] / len, v[1] / len, v[2] / len];
};

/**
 * Every triangle of a model, in scene space: `positions` and `normals` each
 * hold 9 numbers per triangle. The authored normals are kept rather than
 * derived from the winding, which is inconsistent across these models and
 * would light most of the airframe from the inside.
 *
 * `nodes` maps each node's name (its id when unnamed) to the scene-space
 * bounds `{ lo, hi }` of every triangle in it and its children, so named parts
 * such as wheel wells can be located on the flattened airframe. Nodes sharing
 * a name are merged; nodes with no geometry are left out.
 */
export function readTriangles(bytes, externalBuffers = {}) {
  const { json: gltf, body } = parseGlb(bytes);
  const buffers = {};
  for (const [id, buffer] of Object.entries(gltf.buffers)) {
    const uri = buffer.uri ?? "";
    // the binary body goes by two names across the published models, and some
    // declare it with an empty uri rather than the conventional `data:,`
    buffers[id] =
      uri.startsWith("data:") && uri.length > 6 ? decodeDataUri(uri)
      : /^(KHR_)?binary_glTF$/.test(id) || uri === "" || uri === "data:," ? body
      : externalBuffers[uri] ?? (() => { throw new Error(`missing buffer ${uri}`); })();
  }

  const positions = [];
  const normals = [];
  const nodes = {};
  const walk = (id, parent) => {
    const node = gltf.nodes[id];
    if (!node) return;
    const world = multiply(parent, localMatrix(node));
    // this node's subtree is the run of positions it and its children append
    const first = positions.length;
    for (const meshId of node.meshes ?? []) {
      for (const primitive of gltf.meshes[meshId]?.primitives ?? []) {
        if ((primitive.mode ?? 4) !== 4) continue; // triangles only
        const positionId = primitive.attributes?.POSITION;
        if (!positionId) continue;
        const points = readAccessor(gltf, buffers, positionId);
        const authored = primitive.attributes.NORMAL
          ? readAccessor(gltf, buffers, primitive.attributes.NORMAL)
          : null;
        const index = primitive.indices ? readAccessor(gltf, buffers, primitive.indices) : null;
        const count = index ? index.length : points.length / 3;
        for (let i = 0; i + 2 < count; i += 3) {
          const corners = [0, 1, 2].map((c) => (index ? index[i + c] : i + c) * 3);
          for (const v of corners) positions.push(...transform(world, points[v], points[v + 1], points[v + 2]));
          if (authored) {
            for (const v of corners) normals.push(...rotate(world, authored[v], authored[v + 1], authored[v + 2]));
          } else {
            // no authored normals: fall back to the face, winding and all
            const [a, b, c] = corners.map((v) => transform(world, points[v], points[v + 1], points[v + 2]));
            const u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            const w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            const f = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
            const len = Math.hypot(...f) || 1;
            for (let k = 0; k < 3; k++) normals.push(f[0] / len, f[1] / len, f[2] / len);
          }
        }
      }
    }
    for (const child of node.children ?? []) walk(child, world);
    if (positions.length > first) {
      const key = node.name || id;
      const box = (nodes[key] ??= { lo: [Infinity, Infinity, Infinity], hi: [-Infinity, -Infinity, -Infinity] });
      for (let i = first; i < positions.length; i += 3) {
        for (let c = 0; c < 3; c++) {
          box.lo[c] = Math.min(box.lo[c], positions[i + c]);
          box.hi[c] = Math.max(box.hi[c], positions[i + c]);
        }
      }
    }
  };

  const identity = Float64Array.from([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]);
  const scene = gltf.scenes?.[gltf.scene] ?? Object.values(gltf.scenes ?? {})[0];
  for (const id of scene?.nodes ?? Object.keys(gltf.nodes)) walk(id, identity);
  return { positions: Float32Array.from(positions), normals: Float32Array.from(normals), nodes };
}
