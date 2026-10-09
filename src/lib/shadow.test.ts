// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { subsolarPoint } from "./daynight";
import { localSun, SHADOW_MAX_ALPHA, shadowMatrix, shadowOpacity, sunDirection } from "./shadow";

const DEG = Math.PI / 180;
type V3 = [number, number, number];
type M3 = [V3, V3, V3]; // rows

const mul = (a: M3, b: M3): M3 =>
  [0, 1, 2].map((i) => [0, 1, 2].map((j) => a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j])) as M3;
const apply = (m: M3, v: V3): V3 => [0, 1, 2].map((i) => m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2]) as V3;
const rotX = (a: number): M3 => [[1, 0, 0], [0, Math.cos(a), -Math.sin(a)], [0, Math.sin(a), Math.cos(a)]];
const rotY = (a: number): M3 => [[Math.cos(a), 0, Math.sin(a)], [0, 1, 0], [-Math.sin(a), 0, Math.cos(a)]];
const rotZ = (a: number): M3 => [[Math.cos(a), -Math.sin(a), 0], [Math.sin(a), Math.cos(a), 0], [0, 0, 1]];

/** deck.gl's `[pitch, yaw, roll]`: yaw about z, after pitch about y, after roll about x. */
const deckRotation = ([p, y, r]: V3): M3 => mul(rotZ(y * DEG), mul(rotY(p * DEG), rotX(r * DEG)));

/** Where a column-major 4x4 takes a point. */
const transform = (m: number[], [x, y, z]: V3): V3 => [
  m[0] * x + m[4] * y + m[8] * z + m[12],
  m[1] * x + m[5] * y + m[9] * z + m[13],
  m[2] * x + m[6] * y + m[10] * z + m[14],
];

const near = (a: number[], b: number[], tolerance = 1e-9) =>
  a.forEach((v, i) => expect(Math.abs(v - b[i])).toBeLessThan(tolerance));

const normalize = (v: V3): V3 => {
  const n = Math.hypot(...v);
  return v.map((c) => c / n) as V3;
};

describe("shadowMatrix", () => {
  test("projects the oriented, scaled mesh onto the ground along the sun's rays", () => {
    const cases: [V3, V3, V3, number][] = [
      [[0, 0, 0], [1, 1, 1], [0, 0, 1], 0],
      [[12, 180 - 75, 4], [2, 2, 2], normalize([0.3, -0.5, 0.8]), 150],
      [[-25, 33, -8], [0.5, 0.5, 0.02], normalize([-0.9, 0.1, 0.2]), 900],
      [[5, -140, 15], [3, 3, 3], normalize([0.01, 0.6, 0.05]), 40],
    ];
    const vertices: V3[] = [[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1], [3.5, -2, 1.25], [-17, 4, -0.6]];
    for (const [orientation, scale, sun, height] of cases) {
      const m = shadowMatrix(orientation, scale, sun, height);
      const rotation = deckRotation(orientation);
      for (const v of vertices) {
        // the vertex as the aircraft draws it, `height` above the ground
        const [x, y, z] = apply(rotation, [v[0] * scale[0], v[1] * scale[1], v[2] * scale[2]]);
        const lifted: V3 = [x, y, z + height];
        // follow the ray away from the sun down to z = 0
        const t = lifted[2] / sun[2];
        const ground: V3 = [lifted[0] - t * sun[0], lifted[1] - t * sun[1], 0];
        near(transform(m, v), ground, 1e-6);
      }
    }
  });

  test("with the sun overhead the shadow is the plan view, straight below", () => {
    const m = shadowMatrix([10, 60, -20], [1, 1, 1], [0, 0, 1], 500);
    expect(m[12]).toBe(-0);
    expect(m[13]).toBe(-0);
    const rotation = deckRotation([10, 60, -20]);
    const [x, y] = apply(rotation, [0, 10, 0]);
    near(transform(m, [0, 10, 0]), [x, y, 0]);
  });

  test("a point at the origin lands height / tan(sun altitude) away from the sun", () => {
    // sun 30 degrees up, due east (+x)
    const sun: V3 = [Math.cos(30 * DEG), 0, Math.sin(30 * DEG)];
    const m = shadowMatrix([0, 0, 0], [1, 1, 1], sun, 100);
    near(transform(m, [0, 0, 0]), [-100 / Math.tan(30 * DEG), 0, 0]);
  });

  test("lands on a ground plane below the map, meeting the wheels there", () => {
    const sun = normalize([0.4, -0.2, 0.7]);
    // wheels 6 m below the origin, which stands on the map: they touch the plane z = -6
    const m = shadowMatrix([3, 20, -2], [1, 1, 1], sun, 0, undefined, -6);
    const wheel: V3 = apply(deckRotation([3, 20, -2]), [0, 0, -6]);
    const shadow = transform(m, [0, 0, -6]);
    expect(shadow[2]).toBe(-6);
    // a point on the plane is its own shadow
    const onPlane = shadowMatrix([0, 0, 0], [1, 1, 1], sun, 0, undefined, -6);
    near(transform(onPlane, [0, 0, -6]), [0, 0, -6]);
    // and in general it follows the ray down to the plane
    const t = (wheel[2] + 6) / sun[2];
    near(shadow, [wheel[0] - t * sun[0], wheel[1] - t * sun[1], -6]);
  });

  test("writes into the array it is given", () => {
    const out = new Array(16).fill(7);
    expect(shadowMatrix([0, 0, 0], [1, 1, 1], [0, 0, 1], 0, out)).toBe(out);
    expect(out[15]).toBe(1);
  });
});

describe("localSun", () => {
  const time = Date.UTC(2026, 5, 21, 15, 30);
  const sun = sunDirection(time);
  const subsolar = subsolarPoint(time);

  test("is a unit vector", () => {
    for (const [lat, lon] of [[0, 0], [51.5, -0.1], [-33.9, 151.2], [64, -150]]) {
      expect(Math.abs(Math.hypot(...localSun(sun, lat, lon)) - 1)).toBeLessThan(1e-12);
    }
  });

  test("puts the sun overhead at the subsolar point", () => {
    near(localSun(sun, subsolar.lat, subsolar.lon), [0, 0, 1], 1e-12);
  });

  test("puts the sun on the horizon a quarter turn round the equator, and below it opposite", () => {
    expect(Math.abs(localSun(sun, 0, subsolar.lon + 90)[2])).toBeLessThan(1e-9);
    expect(localSun(sun, -subsolar.lat, subsolar.lon + 180)[2]).toBeCloseTo(-1, 9);
  });

  test("points east of a place in its morning and north of it beyond the tropic", () => {
    // 45 degrees west of the subsolar point it is morning: the sun is in the east
    const [east, , up] = localSun(sun, subsolar.lat, subsolar.lon + -45);
    expect(east).toBeGreaterThan(0.6);
    expect(up).toBeGreaterThan(0.6);
    // in June the sun stands north of noon in the southern hemisphere
    const [e2, north, up2] = localSun(sun, -40, subsolar.lon);
    expect(Math.abs(e2)).toBeLessThan(1e-9);
    expect(north).toBeGreaterThan(0);
    // and 40 + 23.4 degrees from the zenith
    expect(Math.acos(up2) / DEG).toBeCloseTo(40 + subsolar.lat, 6);
  });
});

describe("shadowOpacity", () => {
  const A320 = 37.6;
  const C172 = 11;
  const high = Math.sin(60 * DEG);

  test("is darkest on the ground", () => {
    expect(shadowOpacity(0, A320, high)).toBe(SHADOW_MAX_ALPHA);
    expect(shadowOpacity(200, A320, high)).toBe(SHADOW_MAX_ALPHA);
  });

  test("fades with height and is gone at cruise", () => {
    const at = (ft: number) => shadowOpacity(ft, A320, high);
    expect(at(2000)).toBeLessThan(at(500));
    expect(at(3000)).toBeLessThan(0.15);
    expect(at(6000)).toBe(0);
    expect(at(36000)).toBe(0);
  });

  test("small aircraft lose their shadow lower", () => {
    expect(shadowOpacity(2000, C172, high)).toBe(0);
    expect(shadowOpacity(2000, A320, high)).toBeGreaterThan(0.1);
  });

  test("none at night, faint with the sun on the horizon", () => {
    expect(shadowOpacity(0, A320, -0.1)).toBe(0);
    expect(shadowOpacity(0, A320, 0)).toBe(0);
    expect(shadowOpacity(0, A320, Math.sin(3 * DEG))).toBeCloseTo(SHADOW_MAX_ALPHA / 2, 6);
    // long rays: a low sun takes the shadow away sooner
    expect(shadowOpacity(1000, A320, Math.sin(10 * DEG))).toBeLessThan(shadowOpacity(1000, A320, high));
  });
});
