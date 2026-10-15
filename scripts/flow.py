#!/usr/bin/env python3
"""Trafikverket's traffic flow directions (INSPIRE "TN_RAILWAY_TRAFFICFLOWDIRECTION",
GeoPackage, SWEREF 99 TM) to the compact file the app ships:
src-tauri/data/flow.json.

    python3 scripts/flow.py path/to/TN_RAILWAY_TRAFFICFLOWDIRECTION.gpkg

Only the one-way tracks are kept (on double track, which track runs which
way): each becomes [direction, [[lon, lat], ...]], direction 1 when trains
run the way the line is drawn ("in direction"), 2 against it ("in opposite
direction"). Coordinates in WGS 84 to six decimals, the lines thinned to
within 30 cm. Only the standard
library is needed.
"""

import json
import math
import sqlite3
import struct
import sys

DIRECTIONS = {"in direction": 1, "in opposite direction": 2}


def sweref99tm_to_wgs84(e, n):
    """SWEREF 99 TM (EPSG:3006) to latitude, longitude: Lantmäteriet's Gauss-Krüger formulas."""
    a, f = 6378137.0, 1 / 298.257222101
    lon0, k0, fe = math.radians(15.0), 0.9996, 500000.0
    e2 = f * (2 - f)
    m = f / (2 - f)
    ah = a / (1 + m) * (1 + m**2 / 4 + m**4 / 64)
    d1 = m / 2 - 2 * m**2 / 3 + 37 * m**3 / 96 - m**4 / 360
    d2 = m**2 / 48 + m**3 / 15 - 437 * m**4 / 1440
    d3 = 17 * m**3 / 480 - 37 * m**4 / 840
    d4 = 4397 * m**4 / 161280
    a_ = e2 + e2**2 + e2**3 + e2**4
    b_ = -(7 * e2**2 + 17 * e2**3 + 30 * e2**4) / 6
    c_ = (224 * e2**3 + 889 * e2**4) / 120
    d_ = -(4279 * e2**4) / 1260
    xi, eta = n / (k0 * ah), (e - fe) / (k0 * ah)
    xp = (
        xi
        - d1 * math.sin(2 * xi) * math.cosh(2 * eta)
        - d2 * math.sin(4 * xi) * math.cosh(4 * eta)
        - d3 * math.sin(6 * xi) * math.cosh(6 * eta)
        - d4 * math.sin(8 * xi) * math.cosh(8 * eta)
    )
    ep = (
        eta
        - d1 * math.cos(2 * xi) * math.sinh(2 * eta)
        - d2 * math.cos(4 * xi) * math.sinh(4 * eta)
        - d3 * math.cos(6 * xi) * math.sinh(6 * eta)
        - d4 * math.cos(8 * xi) * math.sinh(8 * eta)
    )
    phi = math.asin(math.sin(xp) / math.cosh(ep))
    lon = lon0 + math.atan(math.sinh(ep) / math.cos(xp))
    s = math.sin(phi) ** 2
    lat = phi + math.sin(phi) * math.cos(phi) * (a_ + b_ * s + c_ * s**2 + d_ * s**3)
    return math.degrees(lat), math.degrees(lon)


def line(blob):
    """The points of a GeoPackage line geometry (2D or 3D)."""
    flags = blob[3]
    off = 8 + [0, 32, 48, 48, 64][(flags >> 1) & 7]
    order = "<" if blob[off] == 1 else ">"
    kind = struct.unpack(order + "I", blob[off + 1 : off + 5])[0]
    dims = 3 if kind in (1002, 2002) else 2
    n = struct.unpack(order + "I", blob[off + 5 : off + 9])[0]
    size, start = 8 * dims, off + 9
    return [struct.unpack(order + "dd", blob[start + i * size : start + i * size + 16]) for i in range(n)]


def simplify(points, tolerance):
    """Douglas-Peucker in the layer's own metres: fewer points, the same line within `tolerance`."""
    if len(points) < 3:
        return points
    (ax, ay), (bx, by) = points[0], points[-1]
    dx, dy = bx - ax, by - ay
    length = math.hypot(dx, dy)
    far, at = 0.0, 0
    for i, (px, py) in enumerate(points[1:-1], 1):
        d = abs(dy * (px - ax) - dx * (py - ay)) / length if length else math.hypot(px - ax, py - ay)
        if d > far:
            far, at = d, i
    if far <= tolerance:
        return [points[0], points[-1]]
    return simplify(points[: at + 1], tolerance)[:-1] + simplify(points[at:], tolerance)


def main(path):
    db = sqlite3.connect(path)
    table = "TN_RAILWAY_TRAFFICFLOWDIRECTION"
    srs = db.execute("select srs_id from gpkg_geometry_columns where table_name = ?", (table,)).fetchone()[0]
    pieces = []
    for geom, direction in db.execute(f'select geom, direction from "{table}"'):
        d = DIRECTIONS.get(direction)
        points = line(geom)
        if d is None or len(points) < 2:
            continue
        if srs == 3006:
            # metres: thinned to within 30 cm first
            coords = [sweref99tm_to_wgs84(x, y) for x, y in simplify(points, 0.3)]
        else:
            coords = [(y, x) for x, y in points]
        pieces.append([d, [[round(lon, 6), round(lat, 6)] for lat, lon in coords]])
    out = "src-tauri/data/flow.json"
    with open(out, "w") as f:
        json.dump({"pieces": pieces}, f, separators=(",", ":"))
    print(f"{out}: {len(pieces)} one-way tracks")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
