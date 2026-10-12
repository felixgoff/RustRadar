// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { hullPolygon, shipColor, shipIcon, shipIconSize, shipKindLabel } from "./ships";

const METRES_PER_DEGREE = 111_320;

describe("hullPolygon", () => {
  test("points the bow along the heading, at true length and beam", () => {
    // 200 m long, 30 m wide, reference point 150 m from the bow, centred
    const north = hullPolygon(0, 0, [150, 50, 15, 15], 0);
    const [bow, , sternStbd, sternPort] = north;
    expect(bow[0]).toBeCloseTo(0, 9);
    expect(bow[1] * METRES_PER_DEGREE).toBeCloseTo(150, 6);
    expect(sternStbd[0] * METRES_PER_DEGREE).toBeCloseTo(15, 6); // starboard is east heading north
    expect(sternPort[0] * METRES_PER_DEGREE).toBeCloseTo(-15, 6);
    expect(sternStbd[1] * METRES_PER_DEGREE).toBeCloseTo(-50, 6);

    const east = hullPolygon(0, 0, [150, 50, 15, 15], 90);
    expect(east[0][0] * METRES_PER_DEGREE).toBeCloseTo(150, 6);
    expect(east[0][1]).toBeCloseTo(0, 9);
    expect(east[2][1] * METRES_PER_DEGREE).toBeCloseTo(-15, 6); // starboard is south heading east
  });

  test("longitude stretches with latitude", () => {
    const [bow] = hullPolygon(10, 60, [100, 0, 5, 5], 90);
    expect((bow[0] - 10) * METRES_PER_DEGREE * Math.cos((60 * Math.PI) / 180)).toBeCloseTo(100, 6);
  });
});

describe("type colours and icons", () => {
  test("known and unknown types", () => {
    expect(shipKindLabel(4)).toBe("Cargo");
    expect(shipKindLabel(6)).toBe("Tanker");
    expect(shipKindLabel(13)).toBe("Other");
    expect(shipColor(13)).toEqual(shipColor(0));
    expect(shipIcon({ kind: 3, heading: 90 })).toBe("hull-3");
    expect(shipIcon({ kind: 11, heading: null })).toBe("dot-0");
  });

  test("bigger ships get bigger icons, moored ones a small dot", () => {
    const at = (size: 0 | 1 | 2, heading: number | null) => shipIconSize({ size, heading }, 10);
    expect(at(0, 0)).toBeLessThan(at(1, 0));
    expect(at(1, 0)).toBeLessThan(at(2, 0));
    expect(at(1, null)).toBeLessThan(at(1, 0));
  });
});
