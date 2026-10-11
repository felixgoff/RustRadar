// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { _GlobeViewport as GlobeViewport } from "@deck.gl/core";
import type { Place } from "./api";
import { fitZoom, KIND_ZOOM, placeDetail, placeLanguage, placeView } from "./places";

const place = (p: Partial<Place>): Place => ({ name: "X", label: "X", kind: "poi", lat: 0, lon: 0, ...p });

describe("placeView", () => {
  test("uses the kind's zoom without an extent", () => {
    expect(placeView(place({ kind: "address" }), 1400, 900).zoom).toBe(17.5);
    expect(placeView(place({ kind: "street" }), 1400, 900).zoom).toBe(16);
    expect(placeView(place({ kind: "town" }), 1400, 900).zoom).toBe(12);
    expect(placeView(place({ kind: "country" }), 1400, 900).zoom).toBe(5);
  });

  test("fits an extent, within reach of the kind's zoom", () => {
    // Paris (the city), from Photon
    const paris = place({ kind: "city", lat: 48.8535, lon: 2.3484, bbox: [2.224122, 48.8155755, 2.4697602, 48.902156] });
    const { zoom, lat, lon } = placeView(paris, 1400, 900);
    expect(zoom).toBeGreaterThan(11);
    expect(zoom).toBeLessThan(12.5);
    // centred on the place's point, not its extent's middle
    expect([lat, lon]).toEqual([48.8535, 2.3484]);

    // a country with far-flung territory doesn't zoom out to the whole planet
    const france = place({ kind: "country", lat: 46.6, lon: 2.2, bbox: [-178, -50, 172, 51.1] });
    expect(placeView(france, 1400, 900).zoom).toBe(KIND_ZOOM.country - 2);
    // a few metres of canal don't fly closer than street level
    const canal = place({ kind: "poi", bbox: [4.8985, 52.3767, 4.8989, 52.377] });
    expect(placeView(canal, 1400, 900).zoom).toBe(17.5);
  });

  test("a fitted extent is on screen on the globe", () => {
    const bbox: [number, number, number, number] = [5.8, 47.3, 15.0, 55.1]; // Germany
    const zoom = fitZoom(bbox, 1200, 800);
    const viewport = new GlobeViewport({ width: 1200, height: 800, longitude: 10.4, latitude: 51.2, zoom });
    for (const corner of [
      [bbox[0], bbox[1]],
      [bbox[2], bbox[3]],
      [bbox[0], bbox[3]],
      [bbox[2], bbox[1]],
    ]) {
      const [x, y] = viewport.project([corner[0], corner[1], 0]);
      expect(x).toBeGreaterThan(0);
      expect(x).toBeLessThan(1200);
      expect(y).toBeGreaterThan(0);
      expect(y).toBeLessThan(800);
    }
    // and fills a good part of it: one zoom level closer would not fit
    const closer = new GlobeViewport({ width: 1200, height: 800, longitude: 10.4, latitude: 51.2, zoom: zoom + 1 });
    const [, top] = closer.project([10.4, bbox[3], 0]);
    expect(top).toBeLessThan(0);
  });

  test("handles extents across the antimeridian", () => {
    const fiji = fitZoom([177, -19.2, -179.8, -16.1], 1200, 800);
    expect(fiji).toBeGreaterThan(5);
    expect(fiji).toBeLessThan(8);
  });
});

describe("placeDetail", () => {
  test("says what it is, then where", () => {
    expect(placeDetail(place({ name: "Damrak 1", label: "Damrak 1, 1012 LG Amsterdam, Netherlands", kind: "address" }))).toBe(
      "Address · 1012 LG Amsterdam, Netherlands",
    );
    expect(
      placeDetail(
        place({ name: "Eiffel Tower", label: "Eiffel Tower, 5 Avenue Anatole France, 75007 Paris, France", category: "Tower" }),
      ),
    ).toBe("Tower · 5 Avenue Anatole France, 75007 Paris, France");
    expect(placeDetail(place({ name: "Netherlands", label: "Netherlands", kind: "country" }))).toBe("Country");
  });
});

describe("placeLanguage", () => {
  test("asks for names Photon has, else local ones", () => {
    expect(placeLanguage("de-AT")).toBe("de");
    expect(placeLanguage("en")).toBe("en");
    expect(placeLanguage("nl-NL")).toBeUndefined();
    expect(placeLanguage("")).toBeUndefined();
  });
});
