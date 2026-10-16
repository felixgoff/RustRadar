// OpenFreeMap's 3D buildings, with their shadows on the ground.
//
// An MVTLayer that draws, under each tile's extruded buildings, the shadows
// they cast (building-shadow.ts). The shadows are worked out once per tile and
// sun position, in the tile's own coordinates, and drawn with the tile's own
// model matrix: nothing is recomputed as the camera moves, only when a tile
// loads or the `sun` prop changes (the caller steps it every minute or so).

import type { DefaultProps, Layer, LayersList } from "@deck.gl/core";
import { MVTLayer } from "@deck.gl/geo-layers";
import { SolidPolygonLayer } from "@deck.gl/layers";
import type { BinaryFeatureCollection } from "@loaders.gl/schema";
import { BUILDING_SHADOW_ALPHA, buildingShadows, shadowStep } from "./building-shadow";
import { wallColour } from "./realistic";
import { localSun, SHADOW_MIN_ALPHA, sunlight } from "./shadow";

const DEG = Math.PI / 180;
/** Web Mercator's equator, metres. */
const EARTH_CIRCUMFERENCE = 2 * Math.PI * 6_378_137;

type Properties = Record<string, unknown>;

/** A building's height as drawn, metres: OpenFreeMap's `render_height`, 6 m when it has none. */
export function buildingHeight(properties: Properties): number {
  return properties.hide_3d ? 0 : Number(properties.render_height ?? 6);
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
  const ids = polygons.featureIds.value;
  const numeric = polygons.numericProps ?? {};
  const properties: Properties = {};
  for (let p = 0; p < count; p++) {
    const vertex = starts[p];
    // the drawn layer reads a feature's string properties and its numeric ones (stored per vertex) together
    const own = (polygons.properties[ids[vertex]] ?? {}) as Properties;
    properties.hide_3d = numeric.hide_3d ? numeric.hide_3d.value[vertex] : own.hide_3d;
    properties.render_height = numeric.render_height ? numeric.render_height.value[vertex] : own.render_height;
    heights[p] = buildingHeight(properties);
  }
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
  /** Each building its own wall colour (`wallColour`), in place of `getFillColor`. */
  wallColours: boolean;
};

const defaultProps: DefaultProps<BuildingLayerProps> = {
  sun: { type: "array", value: null, compare: false },
  shadowColor: { type: "color", value: [0, 0, 0] },
  shadowParameters: { type: "object", value: {}, compare: false },
  shadowExtensions: { type: "array", value: [], compare: true },
  wallColours: false,
};

type BinaryPolygons = { startIndices: ArrayLike<number>; attributes: { getPolygon: { value: ArrayLike<number>; size: number } } };

/**
 * Each building's wall colour in a tile, picked by where its outline starts in
 * the world (in the tile grid's own units). A building on a tile's edge is in
 * both tiles whole (the tiles overlap a little), so it is the same colour in
 * each, and so is every building each time it is loaded.
 */
function wallColoursOf(tile: { index: { x: number; y: number; z: number } }) {
  const { x, y } = tile.index;
  return (feature: { properties: Properties } | null, info: { index: number; data: BinaryPolygons }) => {
    const { startIndices, attributes } = info.data;
    const { value, size } = attributes.getPolygon;
    const vertex = startIndices[info.index] * size;
    const gx = Math.round((x + value[vertex]) * 4096);
    const gy = Math.round((y + value[vertex + 1]) * 4096);
    return wallColour(Math.imul(gx, 73_856_093) ^ Math.imul(gy, 19_349_663), feature?.properties.colour);
  };
}

/**
 * Main-thread time (ms) a frame may spend recasting shadows when the sun has
 * moved; the rest of the tiles keep their old shadows for a frame or two. A
 * dense city tile takes a few milliseconds, and at least one is done each frame.
 */
const RECAST_BUDGET_MS = 4;

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
    const { sun, shadowColor, shadowParameters, shadowExtensions } = this.props;
    const content = props.data as BinaryFeatureCollection | unknown[] | null;
    if (!sun || !content || Array.isArray(content)) return buildings;
    const tile = props.tile as TileProps["tile"];
    const shadow = (this.shadowsOf(content, tile, sun) ?? shadowCache.get(content))?.shadow;
    if (!shadow) return buildings;
    const tileProps = props as unknown as TileProps;
    return [
      // Under the tile's buildings, on the ground. Not clipped to the tile: a
      // shadow reaches into the next one, whose own copy of a building on the
      // border only ever casts part of the same shadow.
      new SolidPolygonLayer({
        id: `${props.id}-shadow`,
        data: shadow.data as never,
        _normalize: false,
        positionFormat: "XY",
        filled: true,
        extruded: false,
        material: false,
        getFillColor: [...shadowColor, Math.round(shadow.alpha * 255)],
        modelMatrix: tileProps.modelMatrix as never,
        coordinateOrigin: tileProps.coordinateOrigin,
        coordinateSystem: tileProps.coordinateSystem as never,
        parameters: shadowParameters,
        extensions: shadowExtensions as never,
        pickable: false,
      }),
      buildings,
    ];
  }
}
