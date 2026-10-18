#!/usr/bin/env python3
"""Builds static/street-trees/*.bin, the trees towns keep registers of.

Malmö's and Umeå's registers of the trees in their streets and parks (both
CC0): where each stands, what kind it is, and in Malmö how wide its crown is.
The realistic map plants them where they are (src/lib/street-trees.ts).

    python3 scripts/street-trees.py

Format (little-endian), one file a town:

    "RST1"                         magic
    u32 trees
    per tree: i32 lon * 1e6, i32 lat * 1e6, u8 kind, u8 crown   (10 bytes)

kind: 0 a broadleaf, 1 a conifer, 2 a beech, 3 a linden, 4 an oak (Streets
GL's trees); crown: its diameter in half metres, 0 where not known.
"""

import json
import os
import struct
import urllib.request

OUT = os.path.join(os.path.dirname(__file__), "..", "static", "street-trees")
TOWNS = {
    "malmo": "https://opendata-api.malmo.se/dataset/0d9523fd-40da-4e6f-8a90-981f61034ff3/resource/1fc8d406-9bd2-481d-9b80-885cd5e3655c/download/trad.geojson",
    "umea": "https://opendata.umea.se/api/explore/v2.1/catalog/datasets/trad-som-forvaltas-av-gator-och-parker/exports/geojson",
}
CONIFERS = {
    "pinus", "picea", "abies", "larix", "thuja", "taxus", "juniperus", "chamaecyparis", "pseudotsuga",
    "tsuga", "metasequoia", "sequoiadendron", "cedrus", "cryptomeria", "taxodium", "cupressus", "sciadopitys",
}
KINDS = {"fagus": 2, "tilia": 3, "quercus": 4}


def kind(scientific, category):
    genus = (scientific or "").strip().split(" ")[0].lower()
    if genus in CONIFERS or (category or "").lower().startswith("barr"):
        return 1
    return KINDS.get(genus, 0)


def number(v):
    try:
        f = float(v)
    except (TypeError, ValueError):
        return None
    return f if f == f else None


def trees(town, data):
    for f in data["features"]:
        g = f.get("geometry") or {}
        if g.get("type") != "Point":
            continue
        lon, lat = g["coordinates"][:2]
        p = f.get("properties") or {}
        if town == "malmo":
            k = kind(p.get("vetenskapligt namn"), p.get("kategori"))
            crown = number(p.get("krondiameter"))
        else:
            k = kind(p.get("tradart_vetenskap_namn_1_1_2"), p.get("lov_eller_barrtrad_1_1_1"))
            crown = None
        half = 0 if crown is None or crown <= 0 else min(255, round(crown * 2))
        yield lon, lat, k, half


def main():
    os.makedirs(OUT, exist_ok=True)
    for town, url in TOWNS.items():
        with urllib.request.urlopen(url, timeout=300) as r:
            data = json.load(r)
        found = sorted(trees(town, data), key=lambda t: (t[1], t[0]))
        with open(os.path.join(OUT, f"{town}.bin"), "wb") as out:
            out.write(b"RST1")
            out.write(struct.pack("<I", len(found)))
            for lon, lat, k, half in found:
                out.write(struct.pack("<iiBB", round(lon * 1e6), round(lat * 1e6), k, half))
        print(f"{town}: {len(found)} trees")


if __name__ == "__main__":
    main()
