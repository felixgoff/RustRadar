// @ts-ignore bun's types are not a dependency; the runner provides the module
import { expect, test } from "bun:test";
import {
  Backoff,
  isStation,
  RAIL_MAIN,
  RAIL_SERVICE,
  RAIL_TRANSIT,
  railColor,
  railDash,
  railKind,
  railLayers,
  railWidth,
  railWidthScale,
} from "./rail-layer";

const t = (p: Record<string, unknown>) => ({ properties: { layerName: "transportation", ...p } });

test("rail features by kind", () => {
  expect(railKind(t({ class: "rail", subclass: "rail" }).properties)).toBe("main");
  expect(railKind(t({ class: "rail" }).properties)).toBe("main");
  expect(railKind(t({ class: "rail", subclass: "narrow_gauge" }).properties)).toBe("minor");
  expect(railKind(t({ class: "rail", subclass: "preserved" }).properties)).toBe("minor");
  expect(railKind(t({ class: "rail", subclass: "rail", service: "yard" }).properties)).toBe("service");
  expect(railKind(t({ class: "transit", subclass: "subway", service: "siding" }).properties)).toBe("service");
  expect(railKind(t({ class: "transit", subclass: "tram" }).properties)).toBe("transit");
  expect(railKind(t({ class: "transit", subclass: "light_rail" }).properties)).toBe("transit");
});

test("roads, paths and other layers are not rail", () => {
  expect(railKind(t({ class: "motorway" }).properties)).toBeNull();
  expect(railKind(t({ class: "path" }).properties)).toBeNull();
  expect(railKind({ layerName: "aeroway", class: "rail" })).toBeNull();
  expect(railColor(t({ class: "primary" }))[3]).toBe(0);
  expect(railWidth(t({ class: "primary" }))).toBe(0);
});

test("mainlines are the brightest and widest, service track the dimmest and thinnest", () => {
  const main = t({ class: "rail", subclass: "rail" });
  const yard = t({ class: "rail", subclass: "rail", service: "yard" });
  expect(railColor(main)).toEqual(RAIL_MAIN);
  expect(railColor(yard)).toEqual(RAIL_SERVICE);
  expect(railColor(t({ class: "transit", subclass: "tram" }))).toEqual(RAIL_TRANSIT);
  expect(railWidth(main)).toBeGreaterThan(railWidth(t({ class: "transit", subclass: "tram" })));
  expect(railWidth(yard)).toBeLessThan(railWidth(t({ class: "transit", subclass: "tram" })));
});

test("tunnels are fainter and dashed; the same colour object each time", () => {
  const tunnel = t({ class: "rail", subclass: "rail", brunnel: "tunnel" });
  const c = railColor(tunnel);
  expect(c.slice(0, 3)).toEqual(RAIL_MAIN.slice(0, 3));
  expect(c[3]).toBeLessThan(RAIL_MAIN[3]);
  expect(railColor(tunnel)).toBe(c);
  expect(railDash(tunnel)[0]).toBeGreaterThan(0);
  expect(railDash(t({ class: "rail", brunnel: "bridge" }))).toEqual([0, 0]);
});

test("lines thicken as the view closes in, within bounds", () => {
  expect(railWidthScale(5)).toBe(0.8);
  expect(railWidthScale(10)).toBeGreaterThan(railWidthScale(8));
  expect(railWidthScale(20)).toBe(1.8);
});

test("stations and halts, not tram or subway stops", () => {
  expect(isStation({ layerName: "poi", class: "railway", subclass: "station" })).toBe(true);
  expect(isStation({ layerName: "poi", class: "railway", subclass: "halt" })).toBe(true);
  expect(isStation({ layerName: "poi", class: "railway", subclass: "tram_stop" })).toBe(false);
  expect(isStation({ layerName: "poi", class: "bus", subclass: "station" })).toBe(false);
});

test("layers by zoom: the network from 8, stations from 12, OpenRailwayMap only when asked, from 10", () => {
  const tileUrl = "https://tiles.example/{z}/{x}/{y}.pbf";
  const ids = (o: Partial<Parameters<typeof railLayers>[0]>) =>
    railLayers({ tileUrl, flat: false, zoom: 9, ...o }).map((l) => l.id);
  expect(ids({ zoom: 6 })).toEqual([]);
  expect(ids({ zoom: 9 })).toEqual(["rail-globe"]);
  expect(ids({ zoom: 12 })).toEqual(["rail-globe", "rail-stations-globe"]);
  expect(ids({ zoom: 13, flat: true })).toEqual(["rail-flat", "rail-stations-flat"]);
  expect(ids({ zoom: 13, flat: true, stations: false })).toEqual(["rail-flat"]);
  expect(ids({ zoom: 9, openRailwayMap: true })).toEqual(["rail-globe"]);
  expect(ids({ zoom: 11, openRailwayMap: true })).toEqual(["rail-globe", "rail-openrailwaymap"]);
  // no vector tiles yet: only the raster, if on
  expect(railLayers({ tileUrl: null, flat: false, zoom: 11, openRailwayMap: true }).map((l) => l.id)).toEqual([
    "rail-openrailwaymap",
  ]);
});

test("OpenRailwayMap backoff: a 429 pauses at once, other failures after a run of them", () => {
  let now = 0;
  const b = new Backoff(() => now);
  expect(b.paused).toBe(false);
  b.failed(false);
  b.failed(false);
  b.succeeded();
  b.failed(false);
  b.failed(false);
  b.failed(false);
  expect(b.paused).toBe(false);
  b.failed(false);
  expect(b.paused).toBe(true);
  now += 16 * 60_000;
  expect(b.paused).toBe(false);
  b.failed(true);
  expect(b.paused).toBe(true);
});
