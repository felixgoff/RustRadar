// OpenFreeMap's 3D buildings, with their shadows on the ground.
//
// An MVTLayer that draws, under each tile's extruded buildings, the shadows
// they cast (building-shadow.ts). The shadows are worked out once per tile and
// sun position, in the tile's own coordinates, and drawn with the tile's own
// model matrix: nothing is recomputed as the camera moves, only when a tile
// loads or the `sun` prop changes (the caller steps it every minute or so).

import { COORDINATE_SYSTEM, type DefaultProps, type Layer, type LayersList } from "@deck.gl/core";
import { MVTLayer } from "@deck.gl/geo-layers";
import { PathLayer, SolidPolygonLayer } from "@deck.gl/layers";
import { SimpleMeshLayer } from "@deck.gl/mesh-layers";
import type { BinaryFeatureCollection } from "@loaders.gl/schema";
import { Geometry } from "@luma.gl/engine";
import { BUILDING_SHADOW_ALPHA, buildingShadows, shadowStep } from "./building-shadow";
import { buildingStyle, type BuildingLook } from "./realistic";
import { pitched, roofParts } from "./roofs";
import { sglBuilding } from "./streets-gl";
import { localSun, SHADOW_MIN_ALPHA, sunlight } from "./shadow";

const DEG = Math.PI / 180;
/** Web Mercator's equator, metres. */
const EARTH_CIRCUMFERENCE = 2 * Math.PI * 6_378_137;

type Properties = Record<string, unknown>;

/**
 * The backend's tiles of buildings far out, quantized (building_lod.rs), zooms
 * 10 to 12. Windows and Android webviews reach custom protocols as `http://<scheme>.localhost`.
 */
export function buildingTilesUrl(userAgent = typeof navigator === "undefined" ? "" : navigator.userAgent): string {
  return /Windows|Android/.test(userAgent) ? "http://buildingtiles.localhost/{z}/{x}/{y}" : "buildingtiles://localhost/{z}/{x}/{y}";
}

/**
 * A building's height as drawn, metres: OpenFreeMap's `render_height`, 6 m
 * when it has none; Streets GL's (streets-gl.ts) to its eaves when its roof
 * is pitched (roofs.ts builds it), else to halfway up its roof, and none for
 * one that stands on others or on posts (drawn from the ground, it would fill
 * what is under it).
 */
export function buildingHeight(properties: Properties): number {
  if (properties.layerName === "buildings") {
    const b = sglBuilding(properties);
    if (b.minHeight >= 2) return 0;
    return b.height - (pitched(b.roofType) ? b.roofHeight : b.roofHeight / 2);
  }
  return properties.hide_3d ? 0 : Number(properties.render_height ?? 6);
}

/** How high a building's shadow starts from: halfway up its roof. */
function castHeight(properties: Properties): number {
  if (properties.layerName !== "buildings") return buildingHeight(properties);
  const b = sglBuilding(properties);
  return b.minHeight >= 2 ? 0 : b.height - b.roofHeight / 2;
}

const SGL_FACADES = { plaster: 0, brick: 1, cementBlock: 2, wood: 3, glass: 4 } as const;
const SGL_ROOFS = { tar: 0, concrete: 1, metal: 2, glass: 2, tiles: 3, thatch: 3, eternit: 4, grass: 6 } as const;

/** A building's look as mapped: its walls' colour, and from Streets GL's tiles its façade and roof where they say. */
function lookOf(properties: Properties): { colour: unknown; look?: BuildingLook } {
  if (properties.layerName !== "buildings") return { colour: properties.colour };
  const b = sglBuilding(properties);
  return {
    colour: b.colour,
    look: { facade: b.facade && SGL_FACADES[b.facade], roof: b.roof && SGL_ROOFS[b.roof] },
  };
}

/** A feature's properties in binary tile data: its string ones, and its numeric ones (stored per vertex). */
function propertiesAt(data: { properties: unknown[]; featureIds: { value: ArrayLike<number> }; numericProps?: Record<string, { value: ArrayLike<number> }> }, vertex: number): Properties {
  const own = { ...((data.properties[data.featureIds.value[vertex]] ?? {}) as Properties) };
  for (const key in data.numericProps ?? {}) own[key] = data.numericProps![key].value[vertex];
  return own;
}

type ShadowData = {
  length: 1;
  startIndices: Uint32Array;
  attributes: { getPolygon: { value: Float32Array; size: 2 }; indices: Uint32Array };
};
/** A tile's shadows for one sun: the layer's data and colour, or null when it casts none. */
type TileShadows = { sun: readonly number[]; shadow: { data: ShadowData; alpha: number } | null };
const shadowCache = new WeakMap<object, TileShadows>();

type TileProps = {
  id: string;
  data: unknown;
  tile: { index: { z: number }; bbox: { west?: number; east?: number; north?: number; south?: number } };
  modelMatrix?: unknown;
  coordinateOrigin?: [number, number, number];
  coordinateSystem?: unknown;
};

/**
 * The shadows of one tile's buildings under `sun` (the sun's direction from
 * the Earth's centre, `sunDirection`), in the tile's local coordinates: x
 * east and y south across the tile, 0 to 1.
 */
function tileShadows(content: BinaryFeatureCollection, tile: TileProps["tile"], sun: readonly number[]): TileShadows["shadow"] {
  const { west = 0, east = 0, north = 0, south = 0 } = tile.bbox;
  const lat = (north + south) / 2;
  const toSun = localSun(sun, lat, (west + east) / 2);
  const alpha = BUILDING_SHADOW_ALPHA * sunlight(toSun[2]);
  const step = shadowStep(toSun);
  if (alpha < SHADOW_MIN_ALPHA || !step) return null;
  const polygons = content.polygons;
  if (!polygons || polygons.polygonIndices.value.length < 2) return null;
  // a tile's width in metres on the ground, at its middle
  const tileMetres = (EARTH_CIRCUMFERENCE * Math.cos(lat * DEG)) / 2 ** tile.index.z;
  const starts = polygons.polygonIndices.value;
  const count = starts.length - 1;
  const heights = new Float32Array(count);
  for (let p = 0; p < count; p++) heights[p] = castHeight(propertiesAt(polygons as never, starts[p]));
  const mesh = buildingShadows(
    {
      positions: polygons.positions.value,
      size: polygons.positions.size,
      polygonStarts: starts,
      ringStarts: polygons.primitivePolygonIndices.value,
    },
    heights,
    [step[0] / tileMetres, -step[1] / tileMetres],
  );
  if (!mesh) return null;
  return {
    alpha,
    data: {
      length: 1,
      startIndices: new Uint32Array([0, mesh.positions.length / 2]),
      attributes: { getPolygon: { value: mesh.positions, size: 2 }, indices: mesh.indices },
    },
  };
}

export type BuildingLayerProps = {
  /** The sun's direction from the Earth's centre (`sunDirection`); null casts no shadows. */
  sun: readonly number[] | null;
  /** Shadow colour; the alpha comes from the sun's altitude. */
  shadowColor: [number, number, number];
  /** GPU parameters of the shadow layers, which keep overlapping shadows from darkening twice. */
  shadowParameters: object;
  /** Extensions for the shadow layers (the buildings' own are the `extensions` prop). */
  shadowExtensions: unknown[];
  /** How dark the shadows are, against their own (the sun's height): the weather's say, 0 to 1. */
  shadowStrength: number;
  /** Each building its own look (`buildingStyle`: its colour, and its façade and roof), in place of `getFillColor`. */
  wallColours: boolean;
  /** How dark the ground is at the foot of the walls (0: not drawn), and the extensions it is drawn with. */
  footShade: number;
  footShadeExtensions: unknown[];
  /** The extensions pitched roofs (Streets GL's buildings, roofs.ts) are drawn with; none, none drawn. */
  roofExtensions: unknown[];
};

const defaultProps: DefaultProps<BuildingLayerProps> = {
  sun: { type: "array", value: null, compare: false },
  shadowColor: { type: "color", value: [0, 0, 0] },
  shadowParameters: { type: "object", value: {}, compare: false },
  shadowExtensions: { type: "array", value: [], compare: true },
  shadowStrength: 1,
  roofExtensions: { type: "array", value: [], compare: true },
  wallColours: false,
  footShade: 0,
  footShadeExtensions: { type: "array", value: [], compare: true },
};

type BinaryPolygons = { startIndices: ArrayLike<number>; attributes: { getPolygon: { value: ArrayLike<number>; size: number } } };

/**
 * Each building's look in a tile, picked by where its outline starts in the
 * world (in the tile grid's own units). A building on a tile's edge is in both
 * tiles whole (the tiles overlap a little), so it looks the same in each, and
 * so does every building each time it is loaded.
 */
function wallColoursOf(tile: { index: { x: number; y: number; z: number } }) {
  return (feature: { properties: Properties } | null, info: { index: number; data: BinaryPolygons }) => {
    const { startIndices, attributes } = info.data;
    const { value, size } = attributes.getPolygon;
    return styleAt(tile.index, value, startIndices[info.index] * size, feature?.properties ?? {});
  };
}

/** A building's look (`buildingStyle`), by where its outline starts (`positions[vertex]`, in its tile's own units). */
function styleAt(index: { x: number; y: number }, positions: ArrayLike<number>, vertex: number, properties: Properties) {
  const gx = Math.round((index.x + positions[vertex]) * 4096);
  const gy = Math.round((index.y + positions[vertex + 1]) * 4096);
  const { colour, look } = lookOf(properties);
  return buildingStyle(Math.imul(gx, 73_856_093) ^ Math.imul(gy, 19_349_663), buildingHeight(properties), colour, look);
}

/** A tile's pitched roofs as one mesh, in metres east, north and up of its north-west corner; null when it has none. */
type TileRoofs = { mesh: Geometry; origin: [number, number, number] } | null;
const roofCache = new WeakMap<object, TileRoofs>();
/** A window bay and a storey (realistic.ts's), metres: a gable's texture, in its units. */
const [BAY_M, STOREY_M] = [4, 3.3];

function tileRoofs(content: BinaryFeatureCollection, tile: TileProps["tile"] & { index: { x: number; y: number; z: number } }): TileRoofs {
  if (roofCache.has(content)) return roofCache.get(content)!;
  const polygons = content.polygons;
  let out: TileRoofs = null;
  if (polygons && polygons.polygonIndices.value.length > 1) {
    const { west = 0, north = 0, south = 0 } = tile.bbox;
    const lat = (north + south) / 2;
    const tileMetres = (EARTH_CIRCUMFERENCE * Math.cos(lat * DEG)) / 2 ** tile.index.z;
    const pos = polygons.positions.value;
    const size = polygons.positions.size;
    const starts = polygons.polygonIndices.value;
    const rings = polygons.primitivePolygonIndices.value;
    const [positions, normals, texCoords, colors, info]: number[][] = [[], [], [], [], []];
    let r = 0;
    for (let p = 0; p + 1 < starts.length; p++) {
      const [from, to] = [starts[p], starts[p + 1]];
      const properties = propertiesAt(polygons as never, from);
      if (properties.layerName !== "buildings") continue;
      const b = sglBuilding(properties);
      if (!pitched(b.roofType) || b.roofHeight <= 0 || b.minHeight >= 2) continue;
      // its rings, in metres (the tile's y runs south)
      while (r < rings.length && rings[r] < from) r++;
      const own: [number, number][][] = [];
      for (let k = r; k + 1 < rings.length && rings[k] < to; k++) {
        const ring: [number, number][] = [];
        for (let v = rings[k]; v < rings[k + 1]; v++) ring.push([pos[v * size] * tileMetres, -pos[v * size + 1] * tileMetres]);
        own.push(ring);
      }
      if (!own.length) continue;
      const parts = roofParts({
        ring: own[0],
        holes: own.slice(1),
        base: b.height - b.roofHeight,
        height: b.roofHeight,
        shape: b.roofType,
        orientation: String(properties.roofOrientation ?? ""),
        direction: Number(properties.roofDirection) || undefined,
      });
      if (!parts.walls.length) continue;
      const style = styleAt(tile.index, pos, from * size, properties);
      const code = 255 - style[3];
      const wall = [style[0] / 255, style[1] / 255, style[2] / 255];
      // a roof colour mapped, its own; else the shader's for its kind
      const roofColour = b.roofColour ? hexRgb(b.roofColour) : null;
      parts.walls.forEach((isWall, k) => {
        positions.push(...parts.positions.slice(k * 3, k * 3 + 3));
        normals.push(...parts.normals.slice(k * 3, k * 3 + 3));
        const [u, v] = [parts.uvs[k * 2], parts.uvs[k * 2 + 1]];
        texCoords.push(...(isWall ? [u / BAY_M, v / STOREY_M] : [u, v]));
        colors.push(...(isWall || !roofColour ? wall : roofColour));
        info.push(code, 0, isWall ? 0 : roofColour ? 2 : 1, isWall ? 1 : 0);
      });
    }
    if (positions.length) {
      out = {
        origin: [west, north, 0],
        mesh: new Geometry({
          topology: "triangle-list",
          attributes: {
            positions: { size: 3, value: new Float32Array(positions) },
            normals: { size: 3, value: new Float32Array(normals) },
            texCoords: { size: 2, value: new Float32Array(texCoords) },
            colors: { size: 3, value: new Float32Array(colors) },
            rr_info: { size: 4, value: new Float32Array(info) },
          },
        }),
      };
    }
  }
  roofCache.set(content, out);
  return out;
}

/** A colour's name or `#rrggbb` as 0 to 1, through the map's own names (realistic.ts). */
function hexRgb(colour: string): number[] {
  const [r, g, b] = buildingStyle(0, 0, colour);
  return [r / 255, g / 255, b / 255];
}

/**
 * Main-thread time (ms) a frame may spend recasting shadows when the sun has
 * moved; the rest of the tiles keep their old shadows for a frame or two. A
 * dense city tile takes a few milliseconds, and at least one is done each frame.
 */
const RECAST_BUDGET_MS = 4;
/** The band of shade along the foot of a wall, metres across (half under the building). */
const FOOT_SHADE_M = 5;

type CastTile = { content: unknown; layers: unknown[] | null; isVisible: boolean };

export class BuildingLayer extends MVTLayer<Properties, BuildingLayerProps> {
  static layerName = "BuildingLayer";
  static defaultProps = defaultProps as never;

  /** When this frame's recasting budget runs out, and whether it has recast a tile yet. */
  private recastUntil = 0;
  private recastAny = false;

  /** A tile's shadows under the current sun: from the cache, or recast if the frame allows. */
  private shadowsOf(content: BinaryFeatureCollection, tile: TileProps["tile"], sun: readonly number[]) {
    const cached = shadowCache.get(content);
    if (cached?.sun === sun) return cached;
    // a new tile always gets its shadows; one under an older sun keeps them if the frame is spent
    if (cached && this.recastAny && performance.now() > this.recastUntil) return null;
    this.recastAny = true;
    const fresh = { sun, shadow: tileShadows(content, tile, sun) };
    shadowCache.set(content, fresh);
    return fresh;
  }

  renderLayers() {
    this.recastUntil = performance.now() + RECAST_BUDGET_MS;
    this.recastAny = false;
    const sun = this.props.sun;
    const tiles = (this.state.tileset?.tiles ?? []) as unknown as CastTile[];
    /** On screen, with shadows cast under an older sun (the others are recast when they come back). */
    const stale = (tile: CastTile) =>
      Boolean(sun && tile.isVisible && tile.layers && tile.content && !Array.isArray(tile.content) && shadowCache.get(tile.content)?.sun !== sun);
    for (const tile of tiles) {
      // rebuilt below, if there is time for its shadows
      if (stale(tile) && this.shadowsOf(tile.content as BinaryFeatureCollection, tile as never, sun!)) tile.layers = null;
    }
    const layers = super.renderLayers();
    // the rest next frame
    if (tiles.some(stale)) this.setNeedsUpdate();
    return layers;
  }

  renderSubLayers(props: Parameters<MVTLayer["renderSubLayers"]>[0]): Layer | null | LayersList {
    if (this.props.wallColours) (props as unknown as { getFillColor: unknown }).getFillColor = wallColoursOf(props.tile as never);
    // sets the tile's model matrix and coordinate system on `props`
    const buildings = super.renderSubLayers(props);
    const { sun, shadowColor, shadowParameters, shadowExtensions, shadowStrength, footShade, footShadeExtensions } = this.props;
    const content = props.data as BinaryFeatureCollection | unknown[] | null;
    if (!content || Array.isArray(content)) return buildings;
    const tile = props.tile as TileProps["tile"];
    const shadow = sun && shadowStrength > 0 ? (this.shadowsOf(content, tile, sun) ?? shadowCache.get(content))?.shadow : null;
    const tileProps = props as unknown as TileProps;
    const ground: Layer[] = [];
    if (footShade > 0 && content.polygons && content.polygons.primitivePolygonIndices.value.length > 1) {
      // A soft dark band along the foot of every wall, half of it under the
      // building: where the walls meet the ground, little of the sky's light
      // reaches (Streets GL's ambient occlusion).
      const polygons = content.polygons;
      ground.push(
        new PathLayer({
          id: `${props.id}-foot`,
          data: {
            length: polygons.primitivePolygonIndices.value.length - 1,
            startIndices: polygons.primitivePolygonIndices.value,
            attributes: { getPath: polygons.positions },
          } as never,
          _pathType: "open",
          positionFormat: "XY",
          getColor: [...shadowColor, Math.round(footShade * 255)],
          getWidth: FOOT_SHADE_M,
          widthUnits: "meters",
          modelMatrix: tileProps.modelMatrix as never,
          coordinateOrigin: tileProps.coordinateOrigin,
          coordinateSystem: tileProps.coordinateSystem as never,
          parameters: shadowParameters,
          extensions: footShadeExtensions as never,
          pickable: false,
        }),
      );
    }
    if (shadow) {
      // Under the tile's buildings, on the ground. Not clipped to the tile: a
      // shadow reaches into the next one, whose own copy of a building on the
      // border only ever casts part of the same shadow.
      ground.push(
        new SolidPolygonLayer({
          id: `${props.id}-shadow`,
          data: shadow.data as never,
          _normalize: false,
          positionFormat: "XY",
          filled: true,
          extruded: false,
          material: false,
          getFillColor: [...shadowColor, Math.round(Math.min(1, shadow.alpha * shadowStrength) * 255)],
          modelMatrix: tileProps.modelMatrix as never,
          coordinateOrigin: tileProps.coordinateOrigin,
          coordinateSystem: tileProps.coordinateSystem as never,
          parameters: shadowParameters,
          extensions: shadowExtensions as never,
          pickable: false,
        }),
      );
    }
    const roofs = this.props.roofExtensions.length ? tileRoofs(content, tile as never) : null;
    const over: Layer[] = [];
    if (roofs) {
      over.push(
        new SimpleMeshLayer({
          id: `${props.id}-roofs`,
          data: ONE,
          mesh: roofs.mesh,
          getPosition: ORIGIN,
          coordinateSystem: COORDINATE_SYSTEM.METER_OFFSETS,
          coordinateOrigin: roofs.origin,
          material: this.props.material as never,
          extensions: this.props.roofExtensions as never,
          parameters: this.props.parameters,
          pickable: false,
        }),
      );
    }
    return ground.length || over.length ? [...ground, buildings, ...over] : buildings;
  }
}

const ONE = [0];
const ORIGIN: [number, number, number] = [0, 0, 0];
