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
the alpha is 0 (the trees' shapes are masks of their own). Needs Pillow and NumPy. 512 px, as Streets GL draws them.
"""

import shutil
import sys
from pathlib import Path

import numpy as np
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
    # its edges fade into the grass
    ("dirt", [S + "grass_diffuse", S + "dirt_road_diffuse"], S + "dirt_road_normal", S + "dirt_road_mask", None),
    ("pitch", S + "pitch_generic_diffuse", S + "pitch_generic_normal", S + "common_mask", None),
    ("cycleway", S + "cycleway_diffuse", S + "common_normal", S + "common_mask", None),
    ("wood", S + "wood_road_diffuse", S + "wood_road_normal", S + "wood_road_mask", None),
    # a tramway's rails, set in the street
    ("tram", [S + "asphalt_diffuse", S + "rail_diffuse"], S + "asphalt_normal", S + "common_mask", None),
    # the ground where nothing is mapped: Streets GL's terrain, a pale detail its biome's colour tints
    ("terrain", S + "generic_terrain_color", S + "generic_terrain_normal", S + "common_mask", None),
]

# Streets GL's trees, each a picture of a whole tree (under src/resources/models/tree)
TREES = ["beech", "fir", "linden0", "linden1", "oak"]


def load(root: Path, name: str, mode: str, size: int = SIZE) -> Image.Image:
    image = Image.open(root / f"{name}.png").convert(mode)
    return image.resize((size, size), Image.LANCZOS) if image.size != (size, size) else image


def layered(root: Path, names) -> Image.Image:
    """A colour: one image, or several laid one over the next (by their alpha), opaque."""
    if isinstance(names, str):
        return load(root, names, "RGB")
    image = load(root, names[0], "RGBA")
    for over in names[1:]:
        image = Image.alpha_composite(image, load(root, over, "RGBA"))
    return image.convert("RGB")


def bleed(image: Image.Image) -> Image.Image:
    """An RGBA image's colour, opaque: where it is see-through, the colour of what is nearest (so filtering
    and a browser's premultiplying can't darken the edges). Pull-push over halvings; the size a power of 2."""
    a = np.asarray(image, dtype=np.float64)
    known = (a[..., 3] > 8).astype(np.float64)
    levels = [(a[..., :3] * known[..., None], known)]
    while levels[-1][1].shape[0] > 1:
        c, w = levels[-1]
        levels.append((c[0::2, 0::2] + c[1::2, 0::2] + c[0::2, 1::2] + c[1::2, 1::2], w[0::2, 0::2] + w[1::2, 0::2] + w[0::2, 1::2] + w[1::2, 1::2]))
    filled = None
    for c, w in reversed(levels):
        mean = c / np.maximum(w, 1e-9)[..., None]
        if filled is not None:
            mean = np.where(w[..., None] > 0, mean, np.repeat(np.repeat(filled, 2, axis=0), 2, axis=1))
        filled = mean
    return Image.fromarray(np.clip(filled + 0.5, 0, 255).astype(np.uint8), "RGB")


def trees(root: Path, out: Path) -> None:
    """Each tree's colour (opaque, bled), its shape (white where it is, as a mask) and normal map; and the one
    normal map of a crown's volume they all share."""
    models = root.parent / "models/tree"
    for name in TREES:
        image = Image.open(models / f"{name}_diffuse.png").convert("RGBA").resize((SIZE, SIZE), Image.LANCZOS)
        bleed(image).save(out / f"tree-{name}.color.webp", "WEBP", quality=88, method=6)
        image.getchannel("A").convert("RGB").save(out / f"tree-{name}.mask.webp", "WEBP", quality=90, method=6)
        load(models, f"{name}_normal", "RGB").save(out / f"tree-{name}.normal.webp", "WEBP", quality=90, method=6)
    load(models, "tree_volume_normal", "RGB").save(out / "tree-volume.normal.webp", "WEBP", quality=90, method=6)


def build(root: Path, out: Path, sets, masks: bool) -> None:
    for name, colour, normal, mask, glow in sets:
        layered(root, colour).save(out / f"{name}.color.webp", "WEBP", quality=88, method=6)
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
    # (the Earth's picture is earth-texture.py's)
    for old in out.glob("*.webp"):
        if not old.name.startswith("earth."):
            old.unlink()
    build(root, out, BUILDINGS, masks=True)
    build(root, out, GROUND, masks=False)
    trees(root, out)
    Image.open(root / "surfaces/water_normal.png").convert("RGB").save(out / "water.normal.webp", "WEBP", quality=90, method=6)
    shutil.copyfile(repo / "LICENSE", out / "LICENSE-streets-gl.txt")
    total = sum(f.stat().st_size for f in out.iterdir())
    print(f"{len(list(out.iterdir()))} files, {total / 1e6:.1f} MB in {out}")


if __name__ == "__main__":
    main()
