#!/usr/bin/env python3
"""Builds static/rail/railroads.bin, the railway network drawn zoomed out.

The map's vector tiles carry railways only from zoom 8, so a whole country
showed no rail at all. This takes Natural Earth's 1:10m railroads (public
domain, https://www.naturalearthdata.com), simplifies each line to about a
kilometre, and packs it small enough to ship with the app.

    python3 scripts/build-rail-overview.py [ne_10m_railroads.geojson]

Without an argument it downloads the GeoJSON from the natural-earth-vector
repository. Format (little-endian), read by src/lib/rail-overview.ts:

    "RRO1"                      magic
    u32 paths, u32 points
    per path: u8 category, u16 point count   (paths x 3 bytes, packed)
    per point: i16 lon * 100, i16 lat * 100  (points x 4 bytes)
"""

import json
import os
import struct
import sys
import urllib.request

URL = "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/ne_10m_railroads.geojson"
OUT = os.path.join(os.path.dirname(__file__), "..", "static", "rail", "railroads.bin")
# degrees; about 1 km, below what shows at the zooms this is drawn at
TOLERANCE = 0.01
SCALE = 100


def simplify(points, tol):
    """Douglas-Peucker, iterative."""
    if len(points) < 3:
        return points
    keep = [False] * len(points)
    keep[0] = keep[-1] = True
    stack = [(0, len(points) - 1)]
    while stack:
        a, b = stack.pop()
        (ax, ay), (bx, by) = points[a], points[b]
        dx, dy = bx - ax, by - ay
        norm = (dx * dx + dy * dy) ** 0.5
        best, at = 0.0, -1
        for i in range(a + 1, b):
            px, py = points[i]
            d = abs(dy * (px - ax) - dx * (py - ay)) / norm if norm else ((px - ax) ** 2 + (py - ay) ** 2) ** 0.5
            if d > best:
                best, at = d, i
        if best > tol:
            keep[at] = True
            stack += [(a, at), (at, b)]
    return [p for p, k in zip(points, keep) if k]


def main():
    if len(sys.argv) > 1:
        data = json.load(open(sys.argv[1]))
    else:
        with urllib.request.urlopen(URL) as r:
            data = json.load(r)
    paths = []
    for f in data["features"]:
        g = f["geometry"]
        if not g:
            continue
        lines = [g["coordinates"]] if g["type"] == "LineString" else g["coordinates"]
        category = int(f["properties"].get("category") or 0)
        for line in lines:
            pts = simplify([(x, y) for x, y, *_ in line], TOLERANCE)
            q = []
            for x, y in pts:
                p = (round(x * SCALE), round(y * SCALE))
                if not q or q[-1] != p:
                    q.append(p)
            # split anything longer than a u16 can count
            for i in range(0, len(q), 65000):
                part = q[max(0, i - 1) : i + 65000]
                if len(part) >= 2:
                    paths.append((category, part))
    total = sum(len(p) for _, p in paths)
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "wb") as out:
        out.write(b"RRO1" + struct.pack("<II", len(paths), total))
        out.write(b"".join(struct.pack("<BH", c, len(p)) for c, p in paths))
        out.write(b"".join(struct.pack("<hh", x, y) for _, p in paths for x, y in p))
    print(f"{len(paths)} paths, {total} points, {os.path.getsize(OUT)} bytes -> {os.path.normpath(OUT)}")


if __name__ == "__main__":
    main()
