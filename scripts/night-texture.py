#!/usr/bin/env python3
"""
Builds the realistic map's picture of the Earth's lights at night
(textures/realistic/earth.night.webp), which the globe's night side glows with,
from NASA's Black Marble 2016 (public domain; NASA Earth Observatory, Joshua
Stevens, Miguel Román, Suomi NPP VIIRS):

    curl -O https://eoimages.gsfc.nasa.gov/images/imagerecords/144000/144898/BlackMarble_2016_3km.jpg
    python3 scripts/night-texture.py BlackMarble_2016_3km.jpg

Only the lights are kept: the picture's moonlit land and sea, dark blue, are
taken away (the map lights its night side itself), the lights brought out a
little and warmed towards a sodium lamp's colour.

Equirectangular, north at the top, 4096 × 2048 (about 10 km a pixel). Needs
Pillow and NumPy.
"""

import sys
from pathlib import Path

import numpy as np
from PIL import Image

WIDTH, HEIGHT = 4096, 2048
OUT = Path(__file__).resolve().parent.parent / "textures/realistic/earth.night.webp"
# below this brightness (0 to 1) it is the dark ground, not a light
FLOOR = 0.16


def main():
    Image.MAX_IMAGE_PIXELS = None
    image = Image.open(sys.argv[1]).convert("RGB").resize((WIDTH, HEIGHT), Image.Resampling.LANCZOS)
    c = np.asarray(image, dtype=np.float32) / 255
    value = c.max(axis=2)
    lit = np.clip((value - FLOOR) / (1 - FLOOR), 0, 1) ** 1.1
    # the lights' own colour, warmed: their hue kept, their blue cast taken out
    warm = np.array([1.0, 0.78, 0.48])
    colour = c / np.maximum(value[..., None], 1e-4)
    rgb = (0.55 * warm + 0.45 * colour) * lit[..., None]
    out = Image.fromarray((np.clip(rgb, 0, 1) * 255 + 0.5).astype(np.uint8))
    OUT.parent.mkdir(parents=True, exist_ok=True)
    out.save(OUT, "WEBP", quality=82, method=6)
    print(OUT, OUT.stat().st_size, "bytes")


if __name__ == "__main__":
    main()
