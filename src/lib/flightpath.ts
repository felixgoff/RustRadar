// The shape of an estimated route: great circles en route, and near the
// airports what aircraft actually fly there. Arrivals turn onto the extended
// runway centreline and fly a straight final down a 3° glide path to the
// touchdown zone; departures roll and climb out straight ahead before turning
// on course. Turns are circular arcs of a radius the speed allows (Dubins
// paths), so the drawn path never kinks.
//
// Everything here is an estimate made without the runway in use, the
// procedures or ATC's vectors; `routes.ts` labels it as such. Pure functions
// with no app state, so they can be unit tested.
import type { Airport, LiveFlight, Runway, TrailPoint } from "./api";

export type LonLat = [number, number];
/** Longitude, latitude, altitude in feet above mean sea level. */
export type PathPoint = [number, number, number];

const DEG = Math.PI / 180;
const TAU = 2 * Math.PI;
const EARTH_RADIUS_M = 6_371_000;
const NM_M = 1852;
const FT_M = 0.3048;
const KT_MS = 0.514444;
const GRAVITY = 9.80665;

/** The standard glide path, and the height it crosses the threshold at. */
export const GLIDE_DEG = 3;
const GLIDE_TAN = Math.tan(GLIDE_DEG * DEG);
const THRESHOLD_CROSSING_FT = 50;
/** The glide path's aiming point lies this far past the threshold (about 290 m). */
export const AIM_M = (THRESHOLD_CROSSING_FT * FT_M) / GLIDE_TAN;
/** Where arrivals usually join the final approach, and the limits on it. */
export const FINAL_NM = 10;
const MIN_FINAL_NM = 5;
const MAX_FINAL_NM = 25;
/** The shortest final considered at all, for an aircraft already turning in close. */
const MIN_FINAL_NM_CLOSE = 2;
/** Candidate joining points are this far apart along the centreline. */
const FINAL_STEP_NM = 0.25;
/** Cost per NM of joining the final short of, or beyond, the usual point (in NM of extra flying). */
const SHORT_FINAL_PENALTY = 3;
const LONG_FINAL_PENALTY = 0.2;
/** Cost per NM of descent the path is short of (it would have to be steeper than planned). */
const TOO_HIGH_PENALTY = 4;
/** Airliners turn at up to 25-30° of bank; 25° gives the usual radii. */
const BANK_DEG = 25;
/** Terminal-area speeds for the turn radius, kt (arrivals are slowed to ~180-230 kt). */
const TERMINAL_KT: [number, number] = [140, 230];
const ENROUTE_KT: [number, number] = [140, 480];
/** Steepest descent drawn before the final (idle and speedbrakes), ft per NM. */
const STEEP_FT_PER_NM = 500;
/** Steepest descent a path is planned for: about 3.3°, the 3° rule of thumb with a little in hand. */
const PLANNED_FT_PER_NM = 350;
/** Average climb gradient after take-off, ft per NM. */
const CLIMB_FT_PER_NM = 600;
/** Closer than this, the whole path is shaped in one local frame. */
const LOCAL_M = 150_000;
/** Further out, the great circle hands over to the terminal shape this far from the fix. */
const ENTRY_M = 60_000;
/** ...and the aircraft's current heading joins the great circle within this distance. */
const JOIN_M = 60_000;
/** Straight climb after lift-off before a departure turns on course. */
const CLIMB_OUT_M = 6_000;
/** Sampling: arcs every few degrees, straight legs every few kilometres. */
const ARC_STEP = 3 * DEG;
const LINE_STEP_M = 2_000;
/** Great circles are sampled about this often. */
const GREAT_CIRCLE_STEP_KM = 25;

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const wrap180 = (deg: number) => ((((deg + 180) % 360) + 360) % 360) - 180;
const mod2pi = (a: number) => a - TAU * Math.floor(a / TAU);

// --- Spherical helpers ------------------------------------------------------------

/** Great-circle distance in metres. */
export function distanceM(a: LonLat, b: LonLat): number {
  const [lon1, lat1, lon2, lat2] = [a[0] * DEG, a[1] * DEG, b[0] * DEG, b[1] * DEG];
  const h = Math.sin((lat2 - lat1) / 2) ** 2 + Math.cos(lat1) * Math.cos(lat2) * Math.sin((lon2 - lon1) / 2) ** 2;
  return 2 * EARTH_RADIUS_M * Math.asin(Math.min(1, Math.sqrt(h)));
}

/** Great-circle distance in km. */
export const distanceKm = (a: LonLat, b: LonLat) => distanceM(a, b) / 1000;

/** Initial course from `a` to `b`, degrees clockwise from true north. */
export function bearing(a: LonLat, b: LonLat): number {
  const [lon1, lat1, lon2, lat2] = [a[0] * DEG, a[1] * DEG, b[0] * DEG, b[1] * DEG];
  const y = Math.sin(lon2 - lon1) * Math.cos(lat2);
  const x = Math.cos(lat1) * Math.sin(lat2) - Math.sin(lat1) * Math.cos(lat2) * Math.cos(lon2 - lon1);
  return (Math.atan2(y, x) / DEG + 360) % 360;
}

/** The point `m` metres from `a` on initial course `course`. */
export function destination(a: LonLat, course: number, m: number): LonLat {
  const d = m / EARTH_RADIUS_M;
  const [lon1, lat1, c] = [a[0] * DEG, a[1] * DEG, course * DEG];
  const lat2 = Math.asin(Math.sin(lat1) * Math.cos(d) + Math.cos(lat1) * Math.sin(d) * Math.cos(c));
  const lon2 = lon1 + Math.atan2(Math.sin(c) * Math.sin(d) * Math.cos(lat1), Math.cos(d) - Math.sin(lat1) * Math.sin(lat2));
  return [wrap180(lon2 / DEG), lat2 / DEG];
}

/** The point a fraction `f` of the way along the great circle from `a` to `b`. */
function intermediate(a: LonLat, b: LonLat, f: number): LonLat {
  const d = distanceM(a, b) / EARTH_RADIUS_M;
  if (d < 1e-9) return a;
  const [lon1, lat1, lon2, lat2] = [a[0] * DEG, a[1] * DEG, b[0] * DEG, b[1] * DEG];
  const A = Math.sin((1 - f) * d) / Math.sin(d);
  const B = Math.sin(f * d) / Math.sin(d);
  const x = A * Math.cos(lat1) * Math.cos(lon1) + B * Math.cos(lat2) * Math.cos(lon2);
  const y = A * Math.cos(lat1) * Math.sin(lon1) + B * Math.cos(lat2) * Math.sin(lon2);
  const z = A * Math.sin(lat1) + B * Math.sin(lat2);
  return [Math.atan2(y, x) / DEG, Math.atan2(z, Math.hypot(x, y)) / DEG];
}

/** Points along the great circle from `a` to `b`, about every `stepKm`. */
export function greatCircle(a: LonLat, b: LonLat, stepKm = 80): LonLat[] {
  const km = distanceKm(a, b);
  if (km < 1e-6) return [a, b];
  const n = Math.min(256, Math.max(2, Math.ceil(km / stepKm)));
  return unwrap(Array.from({ length: n + 1 }, (_, i) => (i === 0 ? a : i === n ? b : intermediate(a, b, i / n))));
}

/** Keeps a path's longitudes continuous across the antimeridian. */
function unwrap<T extends LonLat | PathPoint>(points: T[]): T[] {
  let previous = points[0]?.[0] ?? 0;
  return points.map((p) => {
    let lon = p[0];
    while (lon - previous > 180) lon -= 360;
    while (lon - previous < -180) lon += 360;
    previous = lon;
    return (lon === p[0] ? p : [lon, ...p.slice(1)]) as T;
  });
}

/**
 * A local flat frame: azimuthal equidistant around `centre`, x east and y
 * north in metres. Distances and courses from the centre are exact, and
 * within the ~200 km it is used for, headings elsewhere are off by far less
 * than a degree.
 */
function toLocal(centre: LonLat, p: LonLat): [number, number] {
  const d = distanceM(centre, p);
  const c = bearing(centre, p) * DEG;
  return [d * Math.sin(c), d * Math.cos(c)];
}

function fromLocal(centre: LonLat, x: number, y: number): LonLat {
  return destination(centre, Math.atan2(x, y) / DEG, Math.hypot(x, y));
}

/** Radius of a turn at `speedKt` and the usual bank, metres. */
export function turnRadius(speedKt: number): number {
  const v = speedKt * KT_MS;
  return (v * v) / (GRAVITY * Math.tan(BANK_DEG * DEG));
}

// --- Dubins paths -------------------------------------------------------------------
// The shortest path between two poses for a vehicle that can't turn tighter
// than radius r: two arcs joined by a straight line, or three arcs. Angles here
// are mathematical (counter-clockwise from east, radians); L turns left.

type Word = "LSL" | "RSR" | "LSR" | "RSL" | "RLR" | "LRL";
interface Candidate {
  word: Word;
  /** Normalised lengths of the three pieces: radians for arcs, radii for the straight. */
  lengths: [number, number, number];
}

function dubinsCandidates(alpha: number, beta: number, d: number): Candidate[] {
  const [sa, sb, ca, cb] = [Math.sin(alpha), Math.sin(beta), Math.cos(alpha), Math.cos(beta)];
  const cab = Math.cos(alpha - beta);
  const out: Candidate[] = [];
  let p2 = 2 + d * d - 2 * cab + 2 * d * (sa - sb);
  if (p2 >= 0) {
    const tmp = Math.atan2(cb - ca, d + sa - sb);
    out.push({ word: "LSL", lengths: [mod2pi(-alpha + tmp), Math.sqrt(p2), mod2pi(beta - tmp)] });
  }
  p2 = 2 + d * d - 2 * cab + 2 * d * (sb - sa);
  if (p2 >= 0) {
    const tmp = Math.atan2(ca - cb, d - sa + sb);
    out.push({ word: "RSR", lengths: [mod2pi(alpha - tmp), Math.sqrt(p2), mod2pi(-beta + tmp)] });
  }
  p2 = -2 + d * d + 2 * cab + 2 * d * (sa + sb);
  if (p2 >= 0) {
    const p = Math.sqrt(p2);
    const tmp = Math.atan2(-ca - cb, d + sa + sb) - Math.atan2(-2, p);
    out.push({ word: "LSR", lengths: [mod2pi(-alpha + tmp), p, mod2pi(-mod2pi(beta) + tmp)] });
  }
  p2 = -2 + d * d + 2 * cab - 2 * d * (sa + sb);
  if (p2 >= 0) {
    const p = Math.sqrt(p2);
    const tmp = Math.atan2(ca + cb, d - sa - sb) - Math.atan2(2, p);
    out.push({ word: "RSL", lengths: [mod2pi(alpha - tmp), p, mod2pi(beta - tmp)] });
  }
  let c = (6 - d * d + 2 * cab + 2 * d * (sa - sb)) / 8;
  if (Math.abs(c) <= 1) {
    const p = mod2pi(TAU - Math.acos(c));
    const t = mod2pi(alpha - Math.atan2(ca - cb, d - sa + sb) + p / 2);
    out.push({ word: "RLR", lengths: [t, p, mod2pi(alpha - beta - t + p)] });
  }
  c = (6 - d * d + 2 * cab + 2 * d * (sb - sa)) / 8;
  if (Math.abs(c) <= 1) {
    const p = mod2pi(TAU - Math.acos(c));
    const t = mod2pi(-alpha - Math.atan2(ca - cb, d + sa - sb) + p / 2);
    out.push({ word: "LRL", lengths: [t, p, mod2pi(mod2pi(beta) - alpha - t + p)] });
  }
  return out;
}

interface Pose2 {
  x: number;
  y: number;
  /** Mathematical angle, radians. */
  a: number;
}

/** Moves along one piece: `kind` L, R or S, `len` normalised as in `Candidate`. */
function advance(p: Pose2, kind: string, len: number, r: number): Pose2 {
  const { x, y, a } = p;
  if (kind === "L") return { x: x + r * (Math.sin(a + len) - Math.sin(a)), y: y + r * (Math.cos(a) - Math.cos(a + len)), a: a + len };
  if (kind === "R") return { x: x + r * (Math.sin(a) - Math.sin(a - len)), y: y + r * (Math.cos(a - len) - Math.cos(a)), a: a - len };
  return { x: x + r * len * Math.cos(a), y: y + r * len * Math.sin(a), a };
}

/** Points along a sequence of pieces, the start included. */
function samplePieces(start: Pose2, pieces: [string, number][], r: number): [number, number][] {
  const out: [number, number][] = [[start.x, start.y]];
  let p = start;
  for (const [kind, len] of pieces) {
    if (len <= 1e-9) continue;
    const n = Math.max(1, Math.ceil(kind === "S" ? (len * r) / LINE_STEP_M : len / ARC_STEP));
    for (let i = 1; i <= n; i++) {
      const q = advance(p, kind, (len * i) / n, r);
      out.push([q.x, q.y]);
    }
    p = advance(p, kind, len, r);
  }
  return out;
}

/** Compass heading (degrees) to mathematical angle (radians), and back. */
const toMath = (heading: number) => (90 - heading) * DEG;

/** A planned local path: where it starts, its pieces and turn radius, where it ends and how long it is (metres). */
interface Plan {
  start: Pose2;
  pieces: [string, number][];
  r: number;
  to: [number, number];
  length: number;
}

/** Points along a plan, both ends included exactly. */
function samplePlan(plan: Plan): [number, number][] {
  const points = samplePieces(plan.start, plan.pieces, plan.r);
  points[points.length - 1] = plan.to;
  return points;
}

/** A straight line, for when no turning path exists (the target is inside the turn). */
function directPlan(x0: number, y0: number, x1: number, y1: number): Plan {
  const length = Math.hypot(x1 - x0, y1 - y0);
  return { start: { x: x0, y: y0, a: Math.atan2(y1 - y0, x1 - x0) }, pieces: [["S", 1]], r: length, to: [x1, y1], length };
}

/**
 * The shortest smooth path from (x0, y0) heading `h0` to (x1, y1) heading
 * `h1` (compass degrees) with turns of radius `r`.
 */
function dubinsPlan(x0: number, y0: number, h0: number, x1: number, y1: number, h1: number, r: number): Plan {
  const dx = x1 - x0;
  const dy = y1 - y0;
  const theta = Math.atan2(dy, dx);
  const d = Math.hypot(dx, dy) / r;
  const alpha = mod2pi(toMath(h0) - theta);
  const beta = mod2pi(toMath(h1) - theta);
  const start: Pose2 = { x: x0, y: y0, a: toMath(h0) };
  let best: [string, number][] | null = null;
  let bestLength = Infinity;
  for (const { word, lengths } of dubinsCandidates(alpha, beta, d)) {
    const total = lengths[0] + lengths[1] + lengths[2];
    if (total >= bestLength) continue;
    // keep only solutions that really arrive (guards the closed forms' edge cases)
    const pieces: [string, number][] = [...word].map((k, i) => [k, lengths[i]]);
    let end = start;
    for (const [k, len] of pieces) end = advance(end, k, len, r);
    const headingError = Math.abs(Math.atan2(Math.sin(end.a - toMath(h1)), Math.cos(end.a - toMath(h1))));
    if (Math.hypot(end.x - x1, end.y - y1) > Math.max(1, r * 1e-4) || headingError > 1e-3) continue;
    best = pieces;
    bestLength = total;
  }
  if (!best) return directPlan(x0, y0, x1, y1);
  return { start, pieces: best, r, to: [x1, y1], length: bestLength * r };
}

/**
 * The shortest smooth path from (x0, y0) heading `h0` to (x1, y1) heading
 * `h1` (compass degrees) with turns of radius `r`, as local points.
 */
export function dubins(x0: number, y0: number, h0: number, x1: number, y1: number, h1: number, r: number): [number, number][] {
  return samplePlan(dubinsPlan(x0, y0, h0, x1, y1, h1, r));
}

/**
 * The shortest smooth path from (x0, y0) heading `h0` to the point (x1, y1),
 * arriving at whatever heading: one turn, then straight.
 */
function turnThenStraightPlan(x0: number, y0: number, h0: number, x1: number, y1: number, r: number): Plan {
  const a = toMath(h0);
  let best: [string, number][] | null = null;
  let bestLength = Infinity;
  for (const side of [1, -1]) {
    // the turn circle's centre: left of the nose for a left turn
    const cx = x0 - side * r * Math.sin(a);
    const cy = y0 + side * r * Math.cos(a);
    const vx = x1 - cx;
    const vy = y1 - cy;
    const dc = Math.hypot(vx, vy);
    if (dc < r) continue;
    const straight = Math.sqrt(dc * dc - r * r);
    const tangent = Math.atan2(vy, vx) - side * Math.atan2(straight, r);
    const turn = side > 0 ? mod2pi(tangent - (a - Math.PI / 2)) : mod2pi(a + Math.PI / 2 - tangent);
    const length = turn * r + straight;
    if (length < bestLength) {
      bestLength = length;
      best = [
        [side > 0 ? "L" : "R", turn],
        ["S", straight / r],
      ];
    }
  }
  if (!best) return directPlan(x0, y0, x1, y1);
  return { start: { x: x0, y: y0, a }, pieces: best, r, to: [x1, y1], length: bestLength };
}

export function turnThenStraight(x0: number, y0: number, h0: number, x1: number, y1: number, r: number): [number, number][] {
  return samplePlan(turnThenStraightPlan(x0, y0, h0, x1, y1, r));
}

// --- Joining two poses on the globe ---------------------------------------------------

export interface Pose {
  at: LonLat;
  /** Degrees clockwise from true north. */
  heading: number;
}

/** Radii for the two ends of a connection, metres. */
interface Radii {
  start: number;
  end: number;
}

/** A local plan placed on the globe: the frame's centre and the plan. */
interface Placed {
  centre: LonLat;
  plan: Plan;
}

/** A connection: local turns and great circle legs, measured but not yet sampled. */
interface Connection {
  legs: ({ placed: Placed } | { from: LonLat; to: LonLat })[];
  length: number;
}

/**
 * A smooth path from `a` to `b`: a great circle en route, with only the ends
 * shaped (joining from `a`'s heading, and turning onto `b`'s), or, when `b`
 * has no heading, arriving however the great circle does.
 */
function plan(a: Pose, b: LonLat, endHeading: number | null, radii: Radii): Connection {
  const total = distanceM(a.at, b);
  if (total < 1) return { legs: [{ from: a.at, to: b }], length: total };
  const local = (centre: LonLat, from: LonLat, h0: number, h1: number | null, r: number): Placed => {
    const [x0, y0] = toLocal(centre, from);
    return {
      centre,
      plan: h1 === null ? turnThenStraightPlan(x0, y0, h0, 0, 0, r) : dubinsPlan(x0, y0, h0, 0, 0, h1, r),
    };
  };
  if (total <= LOCAL_M) {
    const placed = local(b, a.at, a.heading, endHeading, endHeading === null ? radii.start : radii.end);
    return { legs: [{ placed }], length: placed.plan.length };
  }
  // en route: the great circle, entered from the aircraft's heading
  const entry = endHeading === null ? b : intermediate(a.at, b, (total - ENTRY_M) / total);
  const course = bearing(a.at, entry);
  const legs: Connection["legs"] = [];
  let length = 0;
  let from = a.at;
  if (Math.abs(wrap180(a.heading - course)) > 1) {
    const joinM = Math.min(JOIN_M, (distanceM(a.at, entry) - 1) / 2);
    const join = intermediate(a.at, entry, joinM / distanceM(a.at, entry));
    const placed = local(join, a.at, a.heading, bearing(join, entry), radii.start);
    legs.push({ placed });
    length += placed.plan.length;
    from = join;
  }
  legs.push({ from, to: entry });
  length += distanceM(from, entry);
  if (endHeading !== null) {
    // the great circle's own course where it hands over, turning onto the end pose
    const placed = local(b, entry, bearing(entry, b), endHeading, radii.end);
    legs.push({ placed });
    length += placed.plan.length;
  }
  return { legs, length };
}

/** The points of a connection, from `a` to `b` exactly. */
function sample(connection: Connection, a: LonLat, b: LonLat): LonLat[] {
  const out: LonLat[] = [];
  for (const leg of connection.legs) {
    const points =
      "placed" in leg
        ? samplePlan(leg.placed.plan).map(([x, y]) => fromLocal(leg.placed.centre, x, y))
        : greatCircle(leg.from, leg.to, GREAT_CIRCLE_STEP_KM);
    out.push(...(out.length ? points.slice(1) : points));
  }
  out[0] = a;
  out[out.length - 1] = b;
  return out;
}

function connect(a: Pose, b: LonLat, endHeading: number | null, radii: Radii): LonLat[] {
  return sample(plan(a, b, endHeading, radii), a.at, b);
}

// --- Runways ---------------------------------------------------------------------------

/** One landing (or take-off) direction of a runway. */
export interface RunwayEndInfo {
  ident: string;
  /** Degrees true: the direction of landing and take-off on this end. */
  heading: number;
  /** Where the runway physically begins (take-off rolls start here). */
  start: LonLat;
  /** The landing threshold: the start, moved in by any displacement. */
  threshold: LonLat;
  /** Feet above mean sea level at the threshold. */
  elevationFt: number;
  /** Metres from the start to the far end. */
  lengthM: number;
}

const HARD_SURFACE = /ASP|CON|PEM|BIT|TAR|PAV|SEAL|MAC|CEM/i;

/**
 * The ends airliners would use: on hard surfaces where the airport has any,
 * and not much shorter than its longest runway.
 */
export function runwayEnds(runways: readonly Runway[], fieldElevationFt = 0): RunwayEndInfo[] {
  const hard = runways.filter((r) => HARD_SURFACE.test(r.surface));
  const usable = hard.length ? hard : runways;
  const ends = usable.flatMap((r) =>
    ([0, 1] as const).map((i): RunwayEndInfo => {
      const near = r.ends[i];
      const far = r.ends[1 - i];
      const start: LonLat = [near.lon, near.lat];
      const heading = bearing(start, [far.lon, far.lat]);
      return {
        ident: near.ident,
        heading,
        start,
        threshold: near.displacedFt > 0 ? destination(start, heading, near.displacedFt * FT_M) : start,
        elevationFt: near.elevationFt ?? fieldElevationFt,
        lengthM: distanceM(start, [far.lon, far.lat]),
      };
    }),
  );
  const longest = Math.max(0, ...ends.map((e) => e.lengthM));
  return ends.filter((e) => e.lengthM >= Math.min(0.7 * longest, 1_800));
}

/** Position relative to an end's extended centreline: metres before the threshold (negative past it) and to the right. */
export function centrelineOffset(end: RunwayEndInfo, p: LonLat): { along: number; cross: number } {
  const d = distanceM(end.threshold, p);
  const angle = (bearing(end.threshold, p) - (end.heading + 180)) * DEG;
  // to the right of an aircraft flying the approach is to the left seen from the threshold
  return { along: d * Math.cos(angle), cross: -d * Math.sin(angle) };
}

export interface Traffic {
  id: number;
  lat: number;
  lon: number;
  /** Degrees true. */
  track: number;
  /** Feet. */
  alt: number;
  /** Knots. */
  speed: number;
  onGround: boolean;
}

/** Arrivals and departures seen flying along each end's direction. */
export function runwayUse(ends: readonly RunwayEndInfo[], traffic: readonly Traffic[], ignoreId?: number) {
  const use = new Map<RunwayEndInfo, { arrivals: number; departures: number }>(
    ends.map((e) => [e, { arrivals: 0, departures: 0 }]),
  );
  for (const f of traffic) {
    if (f.id === ignoreId) continue;
    let best: { end: RunwayEndInfo; arrival: boolean; cross: number } | null = null;
    for (const end of ends) {
      if (Math.abs(wrap180(f.track - end.heading)) > 15) continue;
      const { along, cross } = centrelineOffset(end, [f.lon, f.lat]);
      const agl = f.alt - end.elevationFt;
      let arrival: boolean;
      if (f.onGround) {
        // on the runway at speed: a landing or a take-off roll
        if (f.speed < 40 || along > 100 || along < -end.lengthM || Math.abs(cross) > 60) continue;
        arrival = false;
      } else if (agl > 3_000) continue;
      else if (along >= 0) {
        // on final: inside a few degrees of the centreline, within 20 km
        if (along > 20_000 || Math.abs(cross) > 150 + along * 0.06) continue;
        arrival = true;
      } else {
        // climbing out straight ahead
        const past = -along - end.lengthM;
        if (past > 12_000 || Math.abs(cross) > 300 + Math.max(0, past) * 0.08) continue;
        arrival = false;
      }
      if (!best || Math.abs(cross) < Math.abs(best.cross)) best = { end, arrival, cross };
    }
    if (best) {
      const counts = use.get(best.end)!;
      if (best.arrival) counts.arrivals++;
      else counts.departures++;
    }
  }
  return use;
}

/** Why a runway was picked: the aircraft is lined up with it, other traffic is using it, or neither (a guess). */
export type RunwayReason = "aligned" | "traffic" | "guessed";

export interface RunwayChoice {
  end: RunwayEndInfo;
  reason: RunwayReason;
}

/** The direction most traffic is using, if any is seen: its ends (parallel runways share one). */
function flowInUse(ends: readonly RunwayEndInfo[], use: ReturnType<typeof runwayUse>): RunwayEndInfo[] {
  let best: RunwayEndInfo[] = [];
  let bestCount = 0;
  for (const end of ends) {
    const group = ends.filter((e) => Math.abs(wrap180(e.heading - end.heading)) <= 15);
    const count = group.reduce((n, e) => n + use.get(e)!.arrivals * 2 + use.get(e)!.departures, 0);
    if (count > bestCount) {
      best = group;
      bestCount = count;
    }
  }
  return best;
}

export interface Aircraft {
  lon: number;
  lat: number;
  /** Degrees true. */
  track: number;
  /** Feet above mean sea level. */
  altFt: number;
  speedKt: number;
}

/** Lined up with this end's final: close to the centreline, heading the same way, low enough. */
function alignedWith(end: RunwayEndInfo, ac: Aircraft): boolean {
  const { along, cross } = centrelineOffset(end, [ac.lon, ac.lat]);
  return (
    along > 300 &&
    along < 30 * NM_M &&
    Math.abs(cross) < 300 + along * 0.08 &&
    Math.abs(wrap180(ac.track - end.heading)) < 30 &&
    ac.altFt - end.elevationFt < 6_000
  );
}

/**
 * The landing runway: the one the aircraft is lined up with; else the
 * direction other traffic is landing or departing (preferring the parallel
 * arrivals use, then the nearer one); else the one the shortest path reaches.
 */
export function chooseArrivalRunway(
  ends: readonly RunwayEndInfo[],
  ac: Aircraft,
  traffic: readonly Traffic[] = [],
  ignoreId?: number,
): RunwayChoice | null {
  if (!ends.length) return null;
  const here: LonLat = [ac.lon, ac.lat];
  const crossOf = (e: RunwayEndInfo) => Math.abs(centrelineOffset(e, here).cross);
  const aligned = ends.filter((e) => alignedWith(e, ac));
  if (aligned.length) return { end: aligned.reduce((a, b) => (crossOf(b) < crossOf(a) ? b : a)), reason: "aligned" };

  const use = runwayUse(ends, traffic, ignoreId);
  const flow = flowInUse(ends, use);
  if (flow.length) {
    const end = flow.reduce((a, b) => {
      const d = use.get(b)!.arrivals - use.get(a)!.arrivals;
      return d > 0 || (d === 0 && crossOf(b) < crossOf(a)) ? b : a;
    });
    return { end, reason: "traffic" };
  }
  let best = ends[0];
  let bestLength = Infinity;
  for (const end of ends) {
    const length = arrivalPlan(ac, end).length;
    if (length < bestLength) {
      best = end;
      bestLength = length;
    }
  }
  return { end: best, reason: "guessed" };
}

/**
 * The take-off runway, a guess: the direction traffic is using now if any is
 * seen (the flight left recently, or the wind holds), else the end whose
 * climb-out turns onto the first tracked position most directly.
 */
export function chooseDepartureRunway(
  ends: readonly RunwayEndInfo[],
  to: Aircraft,
  traffic: readonly Traffic[] = [],
): RunwayChoice | null {
  if (!ends.length) return null;
  const flow = flowInUse(ends, runwayUse(ends, traffic));
  const candidates = flow.length ? flow : ends;
  let best = candidates[0];
  let bestLength = Infinity;
  for (const end of candidates) {
    const length = departurePlan(end, to).length;
    if (length < bestLength) {
      best = end;
      bestLength = length;
    }
  }
  return { end: best, reason: flow.length ? "traffic" : "guessed" };
}

// --- Arrivals and departures -------------------------------------------------------------

/** Glide path height above the touchdown zone `s` metres before the aiming point, feet. */
const glideFt = (s: number) => (Math.max(0, s) * GLIDE_TAN) / FT_M;

/**
 * The ground track of an arrival: from the aircraft, a smooth turn onto the
 * extended centreline, then the straight final to the threshold and on to
 * the aiming point.
 */
function arrivalGround(ac: Aircraft, end: RunwayEndInfo): LonLat[] {
  return arrivalPlan(ac, end).build();
}

/** An arrival's length (metres), and its ground track when wanted: sampling costs far more than planning. */
function arrivalPlan(ac: Aircraft, end: RunwayEndInfo): { length: number; build: () => LonLat[] } {
  const here: LonLat = [ac.lon, ac.lat];
  const aim = destination(end.threshold, end.heading, AIM_M);
  const radii = {
    start: turnRadius(clamp(ac.speedKt, ...ENROUTE_KT)),
    end: turnRadius(clamp(ac.speedKt, ...TERMINAL_KT)),
  };
  const agl = Math.max(0, ac.altFt - end.elevationFt);
  const fix = (m: number) => destination(end.threshold, end.heading + 180, m);
  const finish = (toFix: Connection, fixM: number) => ({
    length: toFix.length + fixM + AIM_M,
    build: (): LonLat[] => {
      const points = sample(toFix, here, fix(fixM));
      return [...points, ...straight(points[points.length - 1], end.threshold, fixM).slice(1), aim];
    },
  });

  if (alignedWith(end, ac)) {
    // already on (or nearly on) the final: merge onto the centreline a little
    // ahead, far enough on for a gentle S-turn of this radius
    const { along, cross } = centrelineOffset(end, here);
    const offHeading = Math.abs(wrap180(ac.track - end.heading)) * DEG;
    const needed = 1.2 * (2 * Math.sqrt(radii.end * Math.abs(cross)) + radii.end * offHeading);
    const lead = Math.min(along * 0.9, Math.max(1_500, needed));
    if (along - lead < 200) return { length: along + AIM_M, build: () => [here, end.threshold, aim] };
    return finish(plan({ at: here, heading: ac.track }, fix(along - lead), end.heading, radii), along - lead);
  }

  // Join the final where the glide path meets the usual intercept height
  // (about 3,000 ft, 10 NM out), but closer in when that saves a long way
  // round (an aircraft already on base), and further out when the aircraft
  // is too high to get down from where it is.
  const usual = clamp(agl / (GLIDE_TAN * (NM_M / FT_M)), MIN_FINAL_NM, FINAL_NM);
  let best: { toFix: Connection; fixM: number } | null = null;
  let bestCost = Infinity;
  for (let nm = MIN_FINAL_NM_CLOSE; nm <= MAX_FINAL_NM; nm += FINAL_STEP_NM) {
    const fixM = nm * NM_M;
    const toFix = plan({ at: here, heading: ac.track }, fix(fixM), end.heading, radii);
    const length = toFix.length;
    // height to lose before the fix beyond what the path allows for, as the distance it would take
    const excess = agl - glideFt(fixM + AIM_M);
    const short = Math.max(0, (excess / PLANNED_FT_PER_NM) * NM_M - length);
    const cost =
      length +
      fixM +
      (nm < usual ? SHORT_FINAL_PENALTY * (usual - nm) * NM_M : LONG_FINAL_PENALTY * (nm - usual) * NM_M) +
      TOO_HIGH_PENALTY * short;
    if (cost < bestCost) {
      bestCost = cost;
      best = { toFix, fixM };
    }
  }
  return finish(best!.toFix, best!.fixM);
}

/** A straight leg sampled every few kilometres, both ends included. */
function straight(a: LonLat, b: LonLat, metres = distanceM(a, b)): LonLat[] {
  const n = Math.max(1, Math.ceil(metres / LINE_STEP_M));
  const course = bearing(a, b);
  const d = distanceM(a, b);
  return Array.from({ length: n + 1 }, (_, i) => (i === n ? b : i === 0 ? a : destination(a, course, (d * i) / n)));
}

/**
 * Heights for an arrival's ground track: the current altitude until the
 * descent, then down the glide path to the aiming point (an aircraft below it
 * flies level until it meets it, as when intercepting a glideslope from
 * below); an aircraft above it descends more steeply until it is on it.
 */
function descend(ground: LonLat[], altFt: number, elevationFt: number): PathPoint[] {
  const cumulative = [0];
  for (let i = 1; i < ground.length; i++) cumulative.push(cumulative[i - 1] + distanceM(ground[i - 1], ground[i]));
  const total = cumulative[cumulative.length - 1];
  const start = Math.max(0, altFt - elevationFt);
  return unwrap(
    ground.map(([lon, lat], i): PathPoint => {
      const toGo = total - cumulative[i];
      const steep = start - (STEEP_FT_PER_NM * cumulative[i]) / NM_M;
      return [lon, lat, elevationFt + Math.min(start, Math.max(glideFt(toGo), steep))];
    }),
  );
}

/** The estimated path from the aircraft to touchdown on `end`, with heights (ft MSL). */
export function arrivalPath(ac: Aircraft, end: RunwayEndInfo): PathPoint[] {
  return descend(arrivalGround(ac, end), ac.altFt, end.elevationFt);
}

/** Without runways: turn toward the airport and fly straight there, descending at 3°. */
export function arrivalToPoint(ac: Aircraft, airport: LonLat, elevationFt = 0): PathPoint[] {
  const ground = connect({ at: [ac.lon, ac.lat], heading: ac.track }, airport, null, {
    start: turnRadius(clamp(ac.speedKt, ...ENROUTE_KT)),
    end: 0,
  });
  return descend(ground, ac.altFt, elevationFt);
}

/** A departure's length (metres) and, when wanted, its ground track with the index of lift-off. */
function departurePlan(end: RunwayEndInfo, to: Aircraft) {
  // lift-off well before the far end, then straight ahead before turning on course
  const rollM = Math.min(0.6 * end.lengthM, 2_200);
  const liftoff = destination(end.start, end.heading, rollM);
  const climbOut = destination(liftoff, end.heading, CLIMB_OUT_M);
  const target: LonLat = [to.lon, to.lat];
  const rest = plan({ at: climbOut, heading: end.heading }, target, to.track, {
    start: turnRadius(clamp(to.speedKt * 0.6, ...TERMINAL_KT)),
    end: turnRadius(clamp(to.speedKt, ...ENROUTE_KT)),
  });
  return {
    length: rollM + CLIMB_OUT_M + rest.length,
    build: () => ({
      ground: [end.start, ...straight(liftoff, climbOut).slice(0, -1), ...sample(rest, climbOut, target)],
      liftoff: 1,
    }),
  };
}

/**
 * Heights for a climb from the ground at the start of `ground[from]` up to
 * `altFt` at its end: a steady climb, levelling off at the end altitude if it
 * is reached early.
 */
function climb(ground: LonLat[], from: number, elevationFt: number, altFt: number): PathPoint[] {
  const cumulative = ground.map(() => 0);
  for (let i = from + 1; i < ground.length; i++) cumulative[i] = cumulative[i - 1] + distanceM(ground[i - 1], ground[i]);
  const total = cumulative[cumulative.length - 1] || 1;
  const top = Math.max(0, altFt - elevationFt);
  const gradient = Math.max(CLIMB_FT_PER_NM / NM_M, top / total);
  return unwrap(ground.map(([lon, lat], i): PathPoint => [lon, lat, elevationFt + Math.min(top, gradient * cumulative[i])]));
}

/** The estimated path from the take-off roll on `end` to the first tracked position `to`, with heights (ft MSL). */
export function departurePath(end: RunwayEndInfo, to: Aircraft): PathPoint[] {
  const { ground, liftoff } = departurePlan(end, to).build();
  const path = climb(ground, liftoff, end.elevationFt, to.altFt);
  path[path.length - 1][2] = to.altFt;
  return path;
}

/** Without runways: from the airport, the great circle (turning onto the track at its end), climbing steadily. */
export function departureFromPoint(airport: LonLat, elevationFt: number, to: Aircraft): PathPoint[] {
  // flown backwards from the first tracked position, it is an arrival at the airport
  const back = connect({ at: [to.lon, to.lat], heading: (to.track + 180) % 360 }, airport, null, {
    start: turnRadius(clamp(to.speedKt, ...ENROUTE_KT)),
    end: 0,
  }).reverse();
  const path = climb(back, 0, elevationFt, to.altFt);
  path[path.length - 1][2] = to.altFt;
  return path;
}

/**
 * The rest of `path` from the point nearest `here`, starting exactly at
 * `here`: the aircraft moves between route updates, and the drawn path should
 * start at the aircraft without doubling back. Only the first `searchM` of
 * the path is searched; if nothing there is near, `here` is just put first.
 */
export function trimToPosition(path: readonly PathPoint[], here: LonLat, searchM = 20_000, nearM = 3_000): PathPoint[] {
  if (path.length < 2) return [...path];
  let best = { i: 0, f: 0, d: Infinity };
  let walked = 0;
  for (let i = 0; i < path.length - 1 && walked <= searchM; i++) {
    const a: LonLat = [path[i][0], path[i][1]];
    const b: LonLat = [path[i + 1][0], path[i + 1][1]];
    // project in a local frame at the segment start
    const [bx, by] = toLocal(a, b);
    const [px, py] = toLocal(a, here);
    const len2 = bx * bx + by * by;
    const f = len2 > 0 ? clamp((px * bx + py * by) / len2, 0, 1) : 0;
    const d = Math.hypot(px - f * bx, py - f * by);
    if (d < best.d) best = { i, f, d };
    walked += Math.sqrt(len2);
  }
  if (best.d > nearM) return [[here[0], here[1], path[0][2]], ...path.slice(1)];
  const a = path[best.i];
  const b = path[best.i + 1];
  const alt = a[2] + (b[2] - a[2]) * best.f;
  return unwrap([[here[0], here[1], alt] as PathPoint, ...path.slice(best.i + 1)]);
}

// --- A whole estimate ---------------------------------------------------------------------

export interface RunwayGuess {
  /** e.g. `22L` */
  runway: string;
  reason: RunwayReason;
}

/** How far around an airport other traffic says which runway is in use. */
const TRAFFIC_KM = 30;

export interface EstimateInput {
  flight: Pick<LiveFlight, "id" | "lat" | "lon" | "track" | "alt" | "speed" | "onGround">;
  /** The first point of the recorded track. */
  trailStart?: TrailPoint;
  origin?: Airport;
  destination?: Airport;
  originRunways: readonly Runway[];
  destinationRunways: readonly Runway[];
  traffic: readonly Traffic[];
}

/** The estimated parts of a route; pure, given the airports and their runways. */
export function estimateRoute(input: EstimateInput): {
  kind: "estimated" | "none";
  before: PathPoint[];
  ahead: PathPoint[];
  departure: RunwayGuess | null;
  arrival: RunwayGuess | null;
} {
  const { flight, trailStart, origin, destination } = input;
  const kind = origin || destination ? ("estimated" as const) : ("none" as const);
  let ahead: PathPoint[] = [];
  let arrival: RunwayGuess | null = null;
  if (destination && !flight.onGround) {
    const elevation = destination.alt ?? 0;
    const ends = runwayEnds(input.destinationRunways, elevation);
    const ac: Aircraft = { lon: flight.lon, lat: flight.lat, track: flight.track, altFt: flight.alt, speedKt: flight.speed };
    const choice = chooseArrivalRunway(ends, ac, near(input.traffic, destination), flight.id);
    if (choice) {
      ahead = arrivalPath(ac, choice.end);
      arrival = { runway: choice.end.ident, reason: choice.reason };
    } else ahead = arrivalToPoint(ac, [destination.lon, destination.lat], elevation);
  }

  let before: PathPoint[] = [];
  let departure: RunwayGuess | null = null;
  const start = trailStart ?? null;
  // the recorded track usually begins at the gate; only fill a real gap
  if (origin && start && distanceKm([origin.lon, origin.lat], [start.longitude, start.latitude]) > 15) {
    const elevation = origin.alt ?? 0;
    const ends = runwayEnds(input.originRunways, elevation);
    const to: Aircraft = {
      lon: start.longitude,
      lat: start.latitude,
      track: start.track,
      altFt: start.altitude,
      speedKt: start.ground_speed || 250,
    };
    const choice = chooseDepartureRunway(ends, to, near(input.traffic, origin));
    if (choice) {
      before = departurePath(choice.end, to);
      departure = { runway: choice.end.ident, reason: choice.reason };
    } else before = departureFromPoint([origin.lon, origin.lat], elevation, to);
  }
  return { kind, before, ahead, departure, arrival };
}

/** Flights within `TRAFFIC_KM` of an airport. */
function near(traffic: readonly Traffic[], airport: Airport): Traffic[] {
  const dLat = TRAFFIC_KM / 111;
  const dLon = dLat / Math.max(0.05, Math.cos((airport.lat * Math.PI) / 180));
  return traffic.filter(
    (f) =>
      Math.abs(f.lat - airport.lat) < dLat &&
      Math.abs(f.lon - airport.lon) < dLon &&
      distanceKm([f.lon, f.lat], [airport.lon, airport.lat]) < TRAFFIC_KM,
  );
}
