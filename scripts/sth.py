#!/usr/bin/env python3
"""Trafikverket's track speed limits (BIS "Hastighetskategorier", from
Lastkajen as a GeoPackage in WGS 84) to the compact file the app ships:
src-tauri/data/sth.json.

    python3 scripts/sth.py path/to/download.gpkg [more.gpkg ...]

Each piece of track becomes [direction, [speeds], [[lon, lat], ...]]:
direction 0 both ways, 1 along the line as drawn ("Med"), 2 against it
("Mot"); speeds in km/h, the base limit (Sth_bas) then one per cant
deficiency category (Sth_rfb_130 ... Sth_rfb_300). Coordinates to six
decimals (about 10 cm). Only the standard library is needed.
"""

import json
import sqlite3
import struct
import sys

CATEGORIES = [130, 150, 165, 180, 210, 225, 245, 275, 300]
DIRECTIONS = {"Med och mot": 0, "Med": 1, "Mot": 2}


def line(blob):
    """The points of a GeoPackage line geometry (2D or 3D)."""
    flags = blob[3]
    envelope = (flags >> 1) & 7
    off = 8 + [0, 32, 48, 48, 64][envelope]
    order = "<" if blob[off] == 1 else ">"
    kind = struct.unpack(order + "I", blob[off + 1 : off + 5])[0]
    dims = 3 if kind in (1002, 2002) or kind & 0x80000000 else 2
    if kind % 1000 != 2 and (kind & 0xFF) != 2:
        return []
    n = struct.unpack(order + "I", blob[off + 5 : off + 9])[0]
    size = 8 * dims
    start = off + 9
    return [struct.unpack(order + "dd", blob[start + i * size : start + i * size + 16]) for i in range(n)]


def main(paths):
    pieces = []
    for path in paths:
        db = sqlite3.connect(path)
        for (table,) in db.execute("select table_name from gpkg_contents where data_type = 'features'"):
            srs = db.execute("select srs_id from gpkg_geometry_columns where table_name = ?", (table,)).fetchone()[0]
            if srs != 4326:
                sys.exit(f"{table}: coordinates in EPSG:{srs}; export it in WGS 84 (EPSG:4326)")
            columns = ["geom", "DIRECTION", "Sth_bas"] + [f"Sth_rfb_{c}" for c in CATEGORIES]
            query = f'select {",".join(chr(34) + c + chr(34) for c in columns)} from "{table}"'
            for geom, direction, base, *rfb in db.execute(query):
                points = line(geom)
                if len(points) < 2 or base is None:
                    continue
                speeds = [base] + [v if v is not None else base for v in rfb]
                coords = [[round(x, 6), round(y, 6)] for x, y in points]
                pieces.append([DIRECTIONS.get(direction, 0), speeds, coords])
    out = "src-tauri/data/sth.json"
    with open(out, "w") as f:
        json.dump({"categories": CATEGORIES, "pieces": pieces}, f, separators=(",", ":"))
    print(f"{out}: {len(pieces)} pieces of track")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    main(sys.argv[1:])
