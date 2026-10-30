// Reads a SketchUp Collada (.dae) export into plain triangles: positions in
// metres (Z up), and each triangle's colour from its material, its texture's
// average colour standing in for a texture. Only what SketchUp writes is
// handled: <matrix> transforms, instanced nodes and <triangles> with a
// VERTEX input.
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

/** The document as a tree of `{ tag, attrs, children, text }`. */
function parseXml(xml) {
  const root = { tag: "", attrs: {}, children: [], text: "" };
  const stack = [root];
  const token = /<(\/?)([\w:.-]+)((?:\s+[\w:.-]+\s*=\s*"[^"]*")*)\s*(\/?)>|<!--[\s\S]*?-->|<\?[\s\S]*?\?>|([^<]+)/g;
  for (let m; (m = token.exec(xml)); ) {
    const [, close, tag, rawAttrs, selfClose, text] = m;
    const top = stack[stack.length - 1];
    if (text !== undefined) {
      top.text += text;
    } else if (close) {
      stack.pop();
    } else if (tag) {
      const attrs = {};
      for (const a of (rawAttrs ?? "").matchAll(/([\w:.-]+)\s*=\s*"([^"]*)"/g)) attrs[a[1]] = a[2];
      const element = { tag, attrs, children: [], text: "" };
      top.children.push(element);
      if (!selfClose) stack.push(element);
    }
  }
  return root;
}

const child = (e, tag) => e?.children.find((c) => c.tag === tag);
const children = (e, tag) => e?.children.filter((c) => c.tag === tag) ?? [];
const numbers = (e) => e.text.trim().split(/\s+/).filter(Boolean).map(Number);

function* walk(e) {
  yield e;
  for (const c of e.children) yield* walk(c);
}

/** 4×4 row-major matrices. */
const IDENTITY = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
function multiply(a, b) {
  const out = new Array(16).fill(0);
  for (let r = 0; r < 4; r++) for (let c = 0; c < 4; c++) for (let k = 0; k < 4; k++) out[r * 4 + c] += a[r * 4 + k] * b[k * 4 + c];
  return out;
}

/**
 * A texture's average colour, 0..1: macOS `sips` scales it to one pixel as
 * a BMP, read here. shortcut: needs macOS (the build runs on the developer's
 * Mac); on another system use a JPEG decoder instead.
 */
function averageColor(file) {
  const dir = mkdtempSync(join(tmpdir(), "train-texture-"));
  try {
    const out = join(dir, "pixel.bmp");
    execFileSync("sips", ["-s", "format", "bmp", "-z", "1", "1", file, "--out", out], { stdio: "ignore" });
    const bmp = readFileSync(out);
    const at = bmp.readUInt32LE(10);
    // BGR(A)
    return [bmp[at + 2] / 255, bmp[at + 1] / 255, bmp[at] / 255];
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

/** SketchUp's default material, front side. */
const DEFAULT_COLOR = [0.8, 0.8, 0.8];

/**
 * The triangles of the model at `file`: `positions` 9 floats per triangle in
 * metres, `colors` 3 per triangle (0..1), and `groups`, which top-level
 * group of the scene each triangle belongs to.
 */
export function readCollada(file) {
  const root = parseXml(readFileSync(file, "utf8"));
  const byId = new Map();
  for (const e of walk(root)) if (e.attrs.id) byId.set(e.attrs.id, e);
  const ref = (url) => byId.get(url.replace(/^#/, ""));
  const collada = child(root, "COLLADA");
  const unit = Number(child(child(collada, "asset"), "unit")?.attrs.meter ?? 1);

  const colors = new Map();
  /** A material's colour, by its id. */
  function materialColor(id) {
    if (colors.has(id)) return colors.get(id);
    let color = DEFAULT_COLOR;
    const effect = ref(child(byId.get(id), "instance_effect")?.attrs.url ?? "");
    const profile = child(effect, "profile_COMMON");
    const technique = child(profile, "technique");
    const shading = technique?.children[0];
    const diffuse = child(shading, "diffuse");
    const plain = child(diffuse, "color");
    const texture = child(diffuse, "texture");
    if (plain) color = numbers(plain).slice(0, 3);
    else if (texture) {
      // texture → sampler2D → surface → image → file
      const param = (sid) => children(profile, "newparam").find((p) => p.attrs.sid === sid);
      const surface = param(child(child(param(texture.attrs.texture), "sampler2D"), "source")?.text.trim());
      const image = byId.get(child(child(surface, "surface"), "init_from")?.text.trim());
      const path = child(image, "init_from")?.text.trim();
      if (path) {
        try {
          color = averageColor(join(dirname(file), decodeURIComponent(path)));
        } catch (e) {
          console.warn(`  ${path}: ${e.message}; grey instead`);
        }
      }
    }
    colors.set(id, color);
    return color;
  }

  const positions = [];
  const triangleColors = [];
  const groups = [];

  function addGeometry(geometry, matrix, bindings, group) {
    const mesh = child(geometry, "mesh");
    if (!mesh) return;
    for (const tris of children(mesh, "triangles")) {
      const inputs = children(tris, "input");
      const vertex = inputs.find((i) => i.attrs.semantic === "VERTEX");
      if (!vertex) continue;
      const stride = Math.max(...inputs.map((i) => Number(i.attrs.offset ?? 0))) + 1;
      const offset = Number(vertex.attrs.offset ?? 0);
      const position = children(ref(vertex.attrs.source), "input").find((i) => i.attrs.semantic === "POSITION");
      const source = ref(position.attrs.source);
      const points = numbers(child(source, "float_array"));
      const step = Number(child(child(child(source, "technique_common"), "accessor"), "stride")?.text ?? child(child(source, "technique_common"), "accessor")?.attrs.stride ?? 3);
      const target = bindings.get(tris.attrs.material);
      const color = target ? materialColor(target.replace(/^#/, "")) : DEFAULT_COLOR;
      const p = numbers(child(tris, "p"));
      for (let i = 0; i + stride * 3 <= p.length; i += stride * 3) {
        for (let v = 0; v < 3; v++) {
          const k = p[i + v * stride + offset] * step;
          const [x, y, z] = [points[k], points[k + 1], points[k + 2]];
          for (let r = 0; r < 3; r++) positions.push((matrix[r * 4] * x + matrix[r * 4 + 1] * y + matrix[r * 4 + 2] * z + matrix[r * 4 + 3]) * unit);
        }
        triangleColors.push(...color);
        groups.push(group);
      }
    }
  }

  function visit(node, parent, group) {
    let matrix = parent;
    for (const m of children(node, "matrix")) matrix = multiply(matrix, numbers(m));
    for (const g of children(node, "instance_geometry")) {
      const bindings = new Map();
      for (const im of walk(g)) if (im.tag === "instance_material") bindings.set(im.attrs.symbol, im.attrs.target);
      addGeometry(ref(g.attrs.url), matrix, bindings, group);
    }
    for (const n of children(node, "instance_node")) visit(ref(n.attrs.url), matrix, group);
    for (const n of children(node, "node")) visit(n, matrix, group);
  }

  const scene = ref(child(child(collada, "scene"), "instance_visual_scene").attrs.url);
  // the scene's top-level groups, one level under SketchUp's root node
  let group = 0;
  for (const top of children(scene, "node")) {
    const inner = children(top, "node");
    const list = inner.length ? inner : [top];
    const base = children(top, "matrix").reduce((m, x) => multiply(m, numbers(x)), IDENTITY);
    for (const n of list) {
      if (n.attrs.name?.startsWith("skp_camera")) continue;
      visit(n, base, group++);
    }
    // geometry directly on the root node
    for (const g of children(top, "instance_geometry")) visit({ children: [g], tag: "node", attrs: {} }, base, group++);
  }
  return { positions: Float32Array.from(positions), colors: Float32Array.from(triangleColors), groups: Uint32Array.from(groups) };
}
