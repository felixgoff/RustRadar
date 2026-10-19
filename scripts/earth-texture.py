#!/usr/bin/env python3
"""
Builds the realistic map's picture of the whole Earth (textures/realistic/
earth.color.webp), which colours the land zoomed out where the map's own data
says nothing (deserts, savanna, tundra), from NASA's Blue Marble: Next
Generation (public domain; NASA Earth Observatory, Reto Stöckli), cloud-free,
July 2004, without relief shading (the map lights it itself):

    curl -O https://eoimages.gsfc.nasa.gov/images/imagerecords/74000/74092/world.200407.3x5400x2700.jpg
    python3 scripts/earth-texture.py world.200407.3x5400x2700.jpg

- The sea is filled with the colour of the land nearest it: the map draws
  water from its own data, and a coast a pixel off would otherwise be fringed
  dark.
- The brightness is brought to the albedo of Streets GL's textures (the
  picture is stretched for show: its forests are near black), keeping each
  colour's hue and saturation; snow and ice stay bright.

Equirectangular, north at the top, 4096 × 2048 (about 10 km a pixel). Needs
Pillow and NumPy.
"""

import sys
from pathlib import Path

import numpy as np
from PIL import Image

WIDTH, HEIGHT = 4096, 2048
OUT = Path(__file__).resolve().parent.parent / "textures/realistic/earth.color.webp"


def to_linear(c):
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def to_srgb(c):
    c = np.clip(c, 0, 1)
    return np.where(c <= 0.0031308, c * 12.92, 1.055 * c ** (1 / 2.4) - 0.055)


def fill(color, known):
    """`color` where `known`, elsewhere the colour of the nearest known pixels (pull-push over halvings)."""
    levels = [(color * known[..., None], known.astype(np.float64))]
    while min(levels[-1][1].shape) > 1:
        c, w = levels[-1]
        h, wd = (c.shape[0] // 2) * 2, (c.shape[1] // 2) * 2
        c, w = c[:h, :wd], w[:h, :wd]
        levels.append((
            c[0::2, 0::2] + c[1::2, 0::2] + c[0::2, 1::2] + c[1::2, 1::2],
            w[0::2, 0::2] + w[1::2, 0::2] + w[0::2, 1::2] + w[1::2, 1::2],
        ))
    filled = None
    for c, w in reversed(levels):
        mean = c / np.maximum(w, 1e-9)[..., None]
        if filled is not None:
            size = (c.shape[1], c.shape[0])
            up = np.stack([np.asarray(Image.fromarray(filled[..., i].astype(np.float32), "F").resize(size, Image.BILINEAR)) for i in range(3)], -1)
            mean = np.where(w[..., None] > 0, mean, up)
        filled = mean
    return filled


def main(source):
    rgb = np.asarray(Image.open(source).convert("RGB")).astype(np.float64)
    r, g, b = rgb[..., 0], rgb[..., 1], rgb[..., 2]
    # the sea (and lakes): blue over red and green, and not the bright blue-white of ice; or all but black (the deep sea, dark coastal water)
    water = ((b > r + 3) & (b >= g) & (r + g + b < 360)) | (r + g + b < 30)
    lin = fill(to_linear(rgb / 255), ~water)

    # brightness to Streets GL's albedo: the desert as its sand, the rainforest as its woods; snow left bright
    lum = lin @ np.array([0.2126, 0.7152, 0.0722])
    curve = 0.456 * np.maximum(lum, 1e-6) ** 0.49
    t = np.clip((lum - 0.45) / 0.35, 0, 1)
    target = curve + (lum * 0.95 - curve) * (t * t * (3 - 2 * t))
    lin = lin * (target / np.maximum(lum, 1e-6))[..., None]

    out = Image.fromarray((to_srgb(lin) * 255 + 0.5).astype(np.uint8), "RGB").resize((WIDTH, HEIGHT), Image.LANCZOS)
    out.save(OUT, quality=88, method=6)
    print(f"{OUT}: {OUT.stat().st_size // 1024} KB")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
