/**
 * Dev builds without the Rust backend (`#demo` in the address): a few made-up
 * trains running straight through the middle of the view, to see how trains
 * are drawn. Never in the app itself.
 */
import type { Train, TrainKind } from "./api";

const M_PER_DEG = 111_320;

/** `[lat, lon]` `m` metres from `[lat, lon]` going `heading` degrees. */
function ahead(at: [number, number], heading: number, m: number): [number, number] {
  const h = (heading * Math.PI) / 180;
  const k = Math.cos((at[0] * Math.PI) / 180);
  return [at[0] + (Math.cos(h) * m) / M_PER_DEG, at[1] + (Math.sin(h) * m) / (M_PER_DEG * k)];
}

export function demoTrains(centre: { latitude: number; longitude: number }, nowMs: number): Train[] {
  const runs: { heading: number; cars: number; kind: TrainKind; operator: string; offset: number; speed: number }[] = [
    { heading: 70, cars: 3, kind: "regional", operator: "Skånetrafiken", offset: 0, speed: 14 },
    { heading: 250, cars: 6, kind: "highSpeed", operator: "SJ", offset: 40, speed: 22 },
    { heading: 160, cars: 12, kind: "intercity", operator: "Öresundståg", offset: -60, speed: 0 },
  ];
  return runs.map((r, n) => {
    const start = ahead(ahead([centre.latitude, centre.longitude], r.heading + 90, r.offset), r.heading, -40);
    const frames: [number, number, number][] = [];
    for (let s = 0; s <= 120; s += 2) frames.push([...ahead(start, r.heading, r.speed * s), nowMs + s * 1000]);
    const tail: [number, number][] = [0, 100, 200, 300, 400].map((m) => ahead(start, r.heading, -m));
    return {
      id: `trafikverket:demo:${n}`,
      source: "trafikverket",
      name: `Demo ${r.operator} ${100 + n}`,
      kind: r.kind,
      lat: start[0],
      lon: start[1],
      heading: r.heading,
      speedKmh: r.speed * 3.6,
      destination: null,
      operator: r.operator,
      delayMinutes: 0,
      positionUnix: Math.floor(nowMs / 1000),
      frames,
      cars: r.cars,
      tail,
    };
  });
}
