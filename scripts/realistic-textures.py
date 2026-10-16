#!/usr/bin/env python3
"""
Builds the realistic map's textures (static/realistic/) from Streets GL's
(https://github.com/StrandedKitty/streets-gl, MIT licence, © StrandedKitty):

    python3 scripts/realistic-textures.py path/to/streets-gl

Each texture becomes one layer of a texture array in the map's shaders
(src/lib/realistic-textures.ts lists them in the same order):

- `<name>.color.webp`: the colour
- `<name>.normal.webp`: the normal map
- `<name>.mask.webp` (buildings): roughness (red), metalness (green) and where
  the building's own colour tints it (blue)
- `<name>.glow.webp` (buildings): where windows light up at night, smaller

Every image is opaque: a browser may premultiply an image's colour by its
alpha as it decodes it (Safari does), which would lose the colour wherever
the alpha is 0. Needs Pillow. 512 px, as Streets GL draws them.
"""

import shutil
import sys
from pathlib import Path

from PIL import Image

SIZE = 512
# a glow is a soft shape: it needs far fewer pixels
GLOW_SIZE = 128

# (name, colour, normal, mask, glow): paths under src/resources/textures
F = "buildings/facades/"
R = "buildings/roofs/"
S = "surfaces/"
BUILDINGS = [
    ("plaster-wall", F + "plaster_wall_diffuse", F + "plaster_wall_normal", F + "plaster_wall_mask", None),
    ("plaster-window", F + "plaster_window_diffuse", F + "plaster_window_normal", F + "plaster_window_mask", F + "window1_glow"),
    ("brick-wall", F + "brick_wall_diffuse", F + "brick_wall_normal", F + "brick_wall_mask", None),
    ("brick-window", F + "brick_window_diffuse", F + "brick_window_normal", F + "brick_window_mask", F + "window0_glow"),
    ("block-wall", F + "block_wall_diffuse", F + "block_wall_normal", F + "block_wall_mask", None),
    ("block-window", F + "block_window_diffuse", F + "block_window_normal", F + "block_window_mask", F + "window1_glow"),
    ("wood-wall", F + "wood_wall_diffuse", F + "wood_wall_normal", F + "wood_wall_mask", None),
    ("wood-window", F + "wood_window_diffuse", F + "wood_window_normal", F + "wood_window_mask", F + "window0_glow"),
    ("glass", F + "glass_diffuse", F + "glass_normal", F + "glass_mask", F + "glass_glow"),
    ("roof-tar", R + "tar_diffuse", R + "tar_normal", R + "tar_mask", None),
    ("roof-concrete", R + "concrete_diffuse", R + "concrete_normal", R + "concrete_mask", None),
    ("roof-metal", R + "metal_diffuse", R + "metal_normal", R + "metal_mask", None),
    ("roof-tiles", R + "tiles_diffuse", R + "tiles_normal", R + "tiles_mask", None),
    ("roof-eternit", R + "eternit_diffuse", R + "eternit_normal", R + "eternit_mask", None),
    ("roof-generic1", R + "generic1_diffuse", R + "generic1_normal", R + "common_mask", None),
    ("roof-generic3", R + "generic3_diffuse", R + "generic3_normal", R + "common_mask", None),
]
GROUND = [
    ("grass", S + "grass_diffuse", S + "grass_normal", S + "common_mask", None),
    ("park", S + "manicured_grass_diffuse", S + "common_normal", S + "common_mask", None),
    ("forest", S + "forest_floor_diffuse", S + "forest_floor_normal", S + "forest_floor_mask", None),
    ("farmland0", S + "farmland0_diffuse", S + "farmland0_normal", S + "farmland_mask", None),
    ("farmland1", S + "farmland1_diffuse", S + "farmland1_normal", S + "farmland_mask", None),
    ("farmland2", S + "farmland2_diffuse", S + "farmland2_normal", S + "farmland_mask", None),
    ("garden", S + "garden_diffuse", S + "garden_normal", S + "common_mask", None),
    ("sand", S + "sand_diffuse", S + "sand_normal", S + "sand_mask", None),
    ("rock", S + "rock_diffuse", S + "rock_normal", S + "rock_mask", None),
    ("soil", S + "soil_diffuse", S + "soil_normal", S + "soil_mask", None),
    ("pavement", S + "pavement_diffuse", S + "pavement_normal", S + "pavement_mask", None),
    ("asphalt", S + "asphalt_diffuse", S + "asphalt_normal", S + "common_mask", None),
    ("road", S + "asphalt_road_diffuse", S + "asphalt_normal", S + "common_mask", None),
    ("road-unmarked", S + "asphalt_unmarked_road_diffuse", S + "asphalt_normal", S + "common_mask", None),
    ("gravel", S + "gravel_diffuse", S + "gravel_normal", S + "gravel_mask", None),
    ("railway", S + "railway_diffuse", S + "railway_normal", S + "railway_mask", None),
    ("cobblestone", S + "cobblestone_diffuse", S + "cobblestone_normal", S + "common_mask", None),
    ("dirt", S + "dirt_road_diffuse", S + "dirt_road_normal", S + "dirt_road_mask", None),
]


def load(root: Path, name: str, mode: str, size: int = SIZE) -> Image.Image:
    image = Image.open(root / f"{name}.png").convert(mode)
    return image.resize((size, size), Image.LANCZOS) if image.size != (size, size) else image


def build(root: Path, out: Path, sets, masks: bool) -> None:
    for name, colour, normal, mask, glow in sets:
        load(root, colour, "RGB").save(out / f"{name}.color.webp", "WEBP", quality=88, method=6)
        load(root, normal, "RGB").save(out / f"{name}.normal.webp", "WEBP", quality=90, method=6)
        if not masks:
            continue
        load(root, mask, "RGB").save(out / f"{name}.mask.webp", "WEBP", quality=90, method=6)
        # the glow, grey (0 where nothing lights up)
        lit = load(root, glow, "L", GLOW_SIZE) if glow else Image.new("L", (GLOW_SIZE, GLOW_SIZE), 0)
        lit.convert("RGB").save(out / f"{name}.glow.webp", "WEBP", quality=85, method=6)


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    repo = Path(sys.argv[1])
    root = repo / "src/resources/textures"
    out = Path(__file__).resolve().parent.parent / "static" / "realistic"
    out.mkdir(parents=True, exist_ok=True)
    # the ground's roughness is its surface's (realistic-ground.ts), so it needs no masks
    for old in out.glob("*.webp"):
        old.unlink()
    build(root, out, BUILDINGS, masks=True)
    build(root, out, GROUND, masks=False)
    Image.open(root / "surfaces/water_normal.png").convert("RGB").save(out / "water.normal.webp", "WEBP", quality=90, method=6)
    shutil.copyfile(repo / "LICENSE", out / "LICENSE-streets-gl.txt")
    total = sum(f.stat().st_size for f in out.iterdir())
    print(f"{len(list(out.iterdir()))} files, {total / 1e6:.1f} MB in {out}")


if __name__ == "__main__":
    main()
