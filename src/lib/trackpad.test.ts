// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { isTrackpadScroll, pinched, wheelPixels, wrapBearing } from "./trackpad";

const wheel = (over: Record<string, number | boolean>) => ({ deltaX: 0, deltaY: 0, deltaMode: 0, ctrlKey: false, ...over });

describe("trackpad", () => {
  test("a two-finger scroll is told from a mouse wheel's notch", () => {
    expect(isTrackpadScroll(wheel({ deltaX: 3, deltaY: 1 }))).toBe(true);
    // WebKit: a trackpad's wheelDeltaY is -3× its pixels; a wheel's notch is 120
    expect(isTrackpadScroll(wheel({ deltaY: 4, wheelDeltaY: -12 }))).toBe(true);
    expect(isTrackpadScroll(wheel({ deltaY: 4, wheelDeltaY: -120 }))).toBe(false);
    // elsewhere: fractional pixels from a trackpad, whole lines from a wheel
    expect(isTrackpadScroll(wheel({ deltaY: 2.5 }))).toBe(true);
    expect(isTrackpadScroll(wheel({ deltaY: 3, deltaMode: 1 }))).toBe(false);
    // a pinch (ctrl + wheel) is a zoom, not a scroll
    expect(isTrackpadScroll(wheel({ deltaY: 2.5, ctrlKey: true }))).toBe(false);
  });

  test("wheel movement in pixels", () => {
    expect(wheelPixels(wheel({ deltaX: 2, deltaY: 3 }), 800)).toEqual([2, 3]);
    expect(wheelPixels(wheel({ deltaY: 3, deltaMode: 1 }), 800)).toEqual([0, 48]);
    expect(wheelPixels(wheel({ deltaY: 1, deltaMode: 2 }), 800)).toEqual([0, 800]);
  });

  test("pinching apart zooms in, twisting turns the map", () => {
    const start = { zoom: 10, bearing: 20 };
    expect(pinched(start, { scale: 2, rotation: 0 }).zoom).toBeCloseTo(11);
    expect(pinched(start, { scale: 0.5, rotation: 0 }).zoom).toBeCloseTo(9);
    expect(pinched(start, { scale: 1, rotation: 30 }).bearing).toBeCloseTo(-10);
    expect(wrapBearing(190)).toBe(-170);
  });
});
