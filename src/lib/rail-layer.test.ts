// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
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
  railSpeedColor,
  railSpeedWidth,
  railTilesUrl,
  SPEED_BANDS,
  SPEED_UNKNOWN,
  speedColor,
  speedLabels,
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

describe("speed limits", () => {
  const line = (coordinates: [number, number][], properties: Record<string, unknown>) => ({
    geometry: { type: "LineString", coordinates },
    properties: { feature: "rail", state: "present", ...properties },
  });

  test("each limit gets its band's colour; unmapped and other railways their own", () => {
    expect(speedColor(30)).toEqual(SPEED_BANDS[0].color);
    expect(speedColor(80)).toEqual(SPEED_BANDS[1].color);
    expect(speedColor(81)).toEqual(SPEED_BANDS[2].color);
    expect(speedColor(320)).toEqual(SPEED_BANDS[SPEED_BANDS.length - 1].color);
    expect(speedColor(undefined)).toEqual(SPEED_UNKNOWN);
    expect(railSpeedColor({ properties: { feature: "tram", maxspeed: 50 } })).toEqual(RAIL_TRANSIT);
    expect(railSpeedColor({ properties: { feature: "rail", state: "razed", maxspeed: 50 } })[3]).toBe(0);
    expect(railSpeedWidth({ properties: { feature: "rail", service: "siding" } })).toBeLessThan(
      railSpeedWidth({ properties: { feature: "rail" } }),
    );
  });

  test("one label per stretch: a double track says its limit once, sidings not at all", () => {
    const labels = speedLabels(
      [
        line([[0, 0.5], [1, 0.5]], { maxspeed: 160 }),
        line([[0, 0.505], [1, 0.505]], { maxspeed: 160 }),
        line([[0, 0.8], [1, 0.8]], { maxspeed: 80 }),
        line([[0, 0.2], [1, 0.2]], { maxspeed: 40, service: "siding" }),
        line([[0, 0.9], [0.01, 0.9]], { maxspeed: 120 }),
      ],
      1,
    );
    expect(labels.map((l) => l.text).sort()).toEqual(["160", "80"]);
    expect(labels.find((l) => l.text === "160")!.position).toEqual([0.5, 0.5]);
  });

  test("the tiles come through the app's protocol", () => {
    expect(railTilesUrl("Mozilla/5.0 (Macintosh)")).toBe("railtiles://localhost/{z}/{x}/{y}");
    expect(railTilesUrl("Mozilla/5.0 (Linux; Android 14)")).toBe("http://railtiles.localhost/{z}/{x}/{y}");
  });
});
