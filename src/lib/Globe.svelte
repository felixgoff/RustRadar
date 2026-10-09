<script lang="ts" module>
  export type ViewState = {
    longitude: number;
    latitude: number;
    zoom: number;
    bearing: number;
    pitch: number;
    /** Metres the camera's target is raised by; only the flat projection (zoom above 12) takes it. */
    position?: [number, number, number];
  };
</script>

<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import {
    _CameraLight as CameraLight,
    AmbientLight,
    COORDINATE_SYSTEM,
    Deck,
    DirectionalLight,
    _GlobeView as GlobeView,
    LightingEffect,
    LinearInterpolator,
    WebMercatorViewport,
    type Layer,
    type PickingInfo,
  } from "@deck.gl/core";
  import {
    CollisionFilterExtension,
    PathStyleExtension,
    type CollisionFilterExtensionProps,
    type PathStyleExtensionProps,
  } from "@deck.gl/extensions";
  import { MVTLayer, TileLayer } from "@deck.gl/geo-layers";
  import { BitmapLayer, IconLayer, PathLayer, ScatterplotLayer, SolidPolygonLayer, TextLayer } from "@deck.gl/layers";
  import { SimpleMeshLayer } from "@deck.gl/mesh-layers";
  import { SphereGeometry } from "@luma.gl/engine";
  import { load } from "@loaders.gl/core";
  import { MVTLoader } from "@loaders.gl/mvt";
  // decode vector tiles off the main thread with a bundled worker, not one from a CDN
  import mvtWorkerUrl from "@loaders.gl/mvt/mvt-worker.js?url";
  import {
    isTauri,
    radarFrame,
    radarImage,
    type Airport,
    type BoundingBox,
    type FlightDetails,
    type LiveFlight,
  } from "./api";
  import { NorthUpGlobeController } from "./controller";
  import { aircraftModels, type AircraftModel } from "./aircraft";
  import { nightPolygons } from "./daynight";
  import { RADAR_LEVELS_KM, type RadarTile } from "./radar";
  import { mergeMeshes, type PolygonData } from "./radar-mesh";
  import { loadRadarTile, loadVolume, radarCoverageKey, setRadarCoverage, type MeasuredVolume } from "./radar-pool";
  import { liveries, type Livery, type Part } from "./liveries";
  import type { LonLat, Route } from "./routes";
  import {
    AIRCRAFT_COLOR,
    altitudeColor,
    cosAngle,
    effectiveExaggeration,
    elevation,
    extrapolate,
    metersPerPixel,
    SELECTED_COLOR,
    unwrapLongitude,
    visibleRadius,
    viewArea,
    type Basemap,
  } from "./geo";
  import { MODEL_KINDS, MODEL_SIZE, modelFor, models, sizeOf, type ModelKind } from "./models";

  interface Props {
    flights: LiveFlight[];
    selected: LiveFlight | null;
    details: FlightDetails | null;
    /** The selected flight's route, filed or estimated. */
    route: Route | null;
    /** An airport to mark, e.g. the one whose board is open. */
    airport: Airport | null;
    /** Vertical exaggeration of altitudes when zoomed out, 1 = true scale. */
    exaggeration: number;
    basemap: Basemap;
    labels: boolean;
    buildings: boolean;
    /** Paint aircraft in airline colours when zoomed in. */
    liveries: boolean;
    /** Shade the night side of the planet. */
    daylight: boolean;
    /** Precipitation radar overlay. */
    weather: boolean;
    /** Keep the camera centred on the selected aircraft. */
    follow: boolean;
    onselect: (flight: LiveFlight | null) => void;
    /** An airport label was clicked. */
    onairport?: (iata: string) => void;
    onviewchange?: (view: ViewState) => void;
  }

  let {
    flights,
    selected,
    details,
    route,
    airport,
    exaggeration,
    basemap,
    labels,
    buildings,
    liveries: showLiveries,
    daylight,
    weather,
    follow,
    onselect,
    onairport,
    onviewchange,
  }: Props = $props();

  // deck.gl 9.4: above zoom 12 GlobeView renders with a WebMercatorViewport,
  // but GlobeController still calls this GlobeViewport-only method while
  // wheel-zooming and throws. Full anchoring keeps the point under the cursor.
  const mercator = WebMercatorViewport.prototype as unknown as { getZoomAnchorStrength?: () => number };
  mercator.getZoomAnchorStrength ??= () => 1;

  const clamp01 = (x: number) => Math.min(1, Math.max(0, x));

  /** GlobeView switches to a flat Web Mercator projection above this zoom. */
  const FLAT_ABOVE_ZOOM = 12;
  const LIMITS = { minZoom: 0.5, maxZoom: 18, maxPitch: 60 };
  /** TileJSON for OpenFreeMap's vector tiles; the tile URL changes with each planet build. */
  const OPENFREEMAP = "https://tiles.openfreemap.org/planet";
  const ESRI = "https://server.arcgisonline.com/ArcGIS/rest/services";
  const LABEL_FONT = "Overpass Variable";

  let container: HTMLDivElement;
  let deck: Deck<GlobeView> | null = null;
  let view: ViewState = { longitude: 10, latitude: 35, zoom: 1.9, bearing: 0, pitch: 0 };
  let fontsReady = false;
  let frame = 0;
  let lastTick = 0;
  let tick = 0;
  /** Camera transitions in progress (fly-to): following must not cut them short. */
  let transitionUntil = 0;
  /** The camera's target is raised to an aircraft's height and must be lowered once the lock ends. */
  let raised = false;
  let hoveredId: number | null = null;

  // GlobeView culls back faces by default, which also drops camera-facing
  // quads (text, rings, paths). The globe still occludes them through depth.
  const NO_CULL = { cullMode: "none" } as const;
  // Ground layers (land, map, imagery) all lie on the surface: depth can't
  // order them reliably and they would flicker against each other while the
  // camera moves. They paint in layer order instead, and still test against
  // the globe sphere so its far side stays hidden.
  const SURFACE = { depthWriteEnabled: false } as const;
  const SURFACE_NO_CULL = { ...NO_CULL, ...SURFACE } as const;

  // Aircraft are drawn true to scale, but never smaller than a floor (which
  // rises from the whole globe to city level) or bigger than a ceiling.
  const MIN_PX_GLOBE = 8;
  const MIN_PX_CITY = 24;
  const MAX_PX = 72;
  const minPxAt = (zoom: number) => MIN_PX_GLOBE + (MIN_PX_CITY - MIN_PX_GLOBE) * clamp01((zoom - 3) / 7);

  // Zoomed out, aircraft are flat, unshaded silhouettes; they gain depth and
  // shading between zoom 6 and 9.
  const depthAt = (zoom: number) => Math.round(clamp01((zoom - 6) / 3) * 10) / 10;
  /** Up to this tilt the view reads as a flat map, where silhouettes belong. */
  const FLAT_PITCH = 5;
  /** From this zoom the detailed meshes are worth their triangles. */
  const CLOSE_ZOOM = 11;
  // White aircraft have to read as white against a dark map, so most of the
  // light is ambient and the directional part only models the airframe.
  const MESH_MATERIAL = { ambient: 0.72, diffuse: 0.42, shininess: 48, specular: [255, 236, 200] };
  const materials = new Map<number, object>();
  /** Lit 3D material at depth 1, pure flat colour at depth 0. */
  function meshMaterial(depth: number) {
    let m = materials.get(depth);
    if (!m) {
      const { ambient, diffuse, shininess, specular } = MESH_MATERIAL;
      m = {
        ambient: 1 + (ambient - 1) * depth,
        diffuse: diffuse * depth,
        shininess,
        specularColor: specular.map((c) => Math.round(c * depth)) as [number, number, number],
      };
      materials.set(depth, m);
    }
    return m;
  }

  const lighting = new LightingEffect({
    ambient: new AmbientLight({ color: [255, 255, 255], intensity: 1 }),
    camera: new CameraLight({ color: [255, 248, 235], intensity: 0.55 }),
    sun: new DirectionalLight({ color: [255, 244, 225], intensity: 0.7, direction: [-1, -2, -3] }),
  });

  // Layers are rebuilt as cheap descriptors on every render; deck.gl matches
  // them by id and keeps their GPU resources and tile caches. Everything they
  // reference (meshes, callbacks, option objects) is created once so props
  // compare equal between renders.
  const EARTH_MESH = new SphereGeometry({ radius: 6.3e6, nlat: 36, nlong: 72 });
  // the dark map's land is the globe itself; under imagery it only shows at the poles
  const GROUND: Record<Basemap, [number, number, number]> = { dark: [34, 39, 48], satellite: [6, 16, 34] };
  const earthLayer = (kind: Basemap) =>
    new SimpleMeshLayer({
      id: "earth",
      data: [0],
      mesh: EARTH_MESH,
      coordinateSystem: COORDINATE_SYSTEM.CARTESIAN,
      getPosition: [0, 0, 0],
      getColor: GROUND[kind],
      material: false,
      updateTriggers: { getColor: kind },
    });

  // Satellite: Esri imagery, which carries no text of its own.
  const renderImageryTile = (props: any) => {
    const [[west, south], [east, north]] = props.tile.boundingBox;
    return new BitmapLayer(props, {
      data: undefined,
      image: props.data,
      bounds: [west, south, east, north],
      // tiles are Web Mercator images; reproject them onto the globe
      _imageCoordinateSystem: COORDINATE_SYSTEM.CARTESIAN,
      // darkened so the aircraft stand out
      tintColor: [150, 158, 180],
      parameters: SURFACE,
    });
  };
  const imageryLayer = () =>
    new TileLayer({
      id: "imagery",
      data: `${ESRI}/World_Imagery/MapServer/tile/{z}/{y}/{x}`,
      minZoom: 0,
      maxZoom: 19,
      tileSize: 256,
      maxRequests: 12,
      renderSubLayers: renderImageryTile,
      parameters: SURFACE,
    });

  // Dark: drawn from OpenFreeMap vector tiles in the app's palette, with no
  // text baked in. Water, borders, roads and runways go on top of a land
  // polygon.
  type MapFeature = { properties: Record<string, unknown> };
  type RGBA = [number, number, number, number];
  const NONE: RGBA = [0, 0, 0, 0];
  const LAND: RGBA = [34, 39, 48, 255];
  const WATER: RGBA = [14, 24, 37, 255];
  const BORDER: RGBA = [90, 100, 114, 255];
  const BORDER_STATE: RGBA = [59, 67, 77, 255];
  const ROAD_MAJOR: RGBA = [65, 72, 83, 255];
  const ROAD_MINOR: RGBA = [47, 54, 63, 255];
  const RUNWAY: RGBA = [163, 119, 52, 255];
  const TAXIWAY: RGBA = [105, 80, 46, 255];
  const APRON: RGBA = [39, 46, 56, 255];
  const MAJOR_ROADS = new Set(["motorway", "trunk", "primary"]);
  const MINOR_ROADS = new Set(["secondary", "tertiary", "minor", "service"]);

  const vectorFill = ({ properties: p }: MapFeature): RGBA => {
    if (p.layerName === "water") return WATER;
    if (p.layerName === "aeroway") return p.class === "runway" ? RUNWAY : p.class === "taxiway" ? TAXIWAY : APRON;
    return NONE;
  };
  const vectorLine = ({ properties: p }: MapFeature): RGBA => {
    switch (p.layerName) {
      case "boundary":
        return p.maritime ? NONE : Number(p.admin_level) <= 2 ? BORDER : BORDER_STATE;
      case "transportation":
        return MAJOR_ROADS.has(String(p.class)) ? ROAD_MAJOR : MINOR_ROADS.has(String(p.class)) ? ROAD_MINOR : NONE;
      case "aeroway":
        return p.class === "runway" ? RUNWAY : p.class === "taxiway" ? TAXIWAY : NONE;
      default:
        return NONE;
    }
  };
  const vectorLineWidth = ({ properties: p }: MapFeature): number => {
    switch (p.layerName) {
      case "boundary":
        return Number(p.admin_level) <= 2 ? 1 : 0.6;
      case "transportation":
        return p.class === "motorway" || p.class === "trunk" ? 1.4 : p.class === "primary" ? 1 : 0.6;
      case "aeroway":
        return p.class === "runway" ? 4 : 1.5;
      default:
        return 0;
    }
  };
  const VECTOR_LOAD_OPTIONS = {
    mvt: { layers: ["water", "boundary", "transportation", "aeroway"], workerUrl: mvtWorkerUrl },
  };

  // The sphere sits below the surface and is clipped up close (and absent in
  // the flat projection), so land is an explicit polygon under the vector map.
  const LAND_POLYGON = [
    [
      [-180, -85.06],
      [180, -85.06],
      [180, 85.06],
      [-180, 85.06],
    ],
  ];
  const LAND_DATA = [LAND_POLYGON];
  const landLayer = () =>
    new SolidPolygonLayer({
      id: "land",
      data: LAND_DATA,
      getPolygon: (d) => d,
      getFillColor: LAND,
      parameters: SURFACE,
    });

  /** MVTLayer decodes tiles per projection, so each projection gets its own layer and cache. */
  const vectorMapLayer = (url: string, flat: boolean) =>
    new MVTLayer({
      id: flat ? "map-flat" : "map-globe",
      data: url,
      minZoom: 0,
      maxZoom: 14,
      loadOptions: VECTOR_LOAD_OPTIONS,
      filled: true,
      stroked: false,
      getFillColor: vectorFill,
      getLineColor: vectorLine,
      getLineWidth: vectorLineWidth,
      lineWidthUnits: "pixels",
      getPointRadius: 0,
      parameters: SURFACE_NO_CULL,
    });

  type LabelFeature = {
    geometry: { type: string; coordinates: [number, number] };
    properties: Record<string, string | number | undefined>;
  };

  const PLACE_CLASSES = new Set(["country", "city", "town", "village", "suburb"]);
  const isLabel = (f: LabelFeature) =>
    f.geometry?.type === "Point" &&
    (f.properties.layerName === "aerodrome_label"
      ? Boolean(f.properties.iata)
      : PLACE_CLASSES.has(String(f.properties.class)));
  const nameOf = (p: LabelFeature["properties"]) => String(p.name_en || p["name:latin"] || p.name || "");
  const rank = (p: LabelFeature["properties"]) => Number(p.rank ?? 10);

  function labelText(f: LabelFeature): string {
    const p = f.properties;
    if (p.layerName === "aerodrome_label") return String(p.iata);
    return p.class === "country" ? nameOf(p).toUpperCase() : nameOf(p);
  }

  function labelSize(f: LabelFeature): number {
    const p = f.properties;
    if (p.layerName === "aerodrome_label") return 12;
    switch (p.class) {
      case "country":
        return 12;
      case "city":
        return rank(p) <= 3 ? 14 : 13;
      case "town":
        return 12;
      default:
        return 11;
    }
  }

  function labelColor(f: LabelFeature): [number, number, number, number] {
    const p = f.properties;
    if (p.layerName === "aerodrome_label") return [252, 180, 66, 255];
    if (p.class === "country") return [162, 172, 183, 255];
    if (p.class === "city") return [241, 238, 231, 245];
    return [179, 191, 204, 235];
  }

  function labelPriority(f: LabelFeature): number {
    const p = f.properties;
    if (p.layerName === "aerodrome_label") return 600;
    const base = p.class === "country" ? 900 : p.class === "city" ? 800 : p.class === "town" ? 400 : 100;
    return base - rank(p) * 10;
  }

  // Labels: a data-only TileLayer decodes OpenFreeMap's place and airport
  // points to lon/lat (in either projection), and one TextLayer draws them in
  // screen space. They skip the depth test so the curved globe can't slice
  // them; labels beyond the horizon are filtered out instead.
  let tileUrl: string | null = null;
  let labelFeatures: LabelFeature[] = [];
  let visibleLabels: { key: string; data: LabelFeature[] } = { key: "", data: [] };

  const getLabelTile = async ({ index, url, signal }: { index: unknown; url?: string | null; signal?: AbortSignal }) => {
    if (!url) return [];
    const features = (await load(url, MVTLoader, {
      worker: false,
      mvt: { shape: "geojson", coordinates: "wgs84", tileIndex: index, layers: ["place", "aerodrome_label"] },
      fetch: { signal },
    })) as unknown as LabelFeature[];
    return features.filter(isLabel);
  };
  const onLabelTiles = (tiles: { content: LabelFeature[] | null }[]) => {
    labelFeatures = tiles.flatMap((t) => t.content ?? []);
    visibleLabels = { key: "", data: [] };
  };
  const renderNothing = () => null;
  const labelTileLayer = (url: string) =>
    new TileLayer<LabelFeature[]>({
      id: "label-tiles",
      data: url,
      minZoom: 0,
      maxZoom: 14,
      tileSize: 512,
      getTileData: getLabelTile,
      onViewportLoad: onLabelTiles,
      renderSubLayers: renderNothing,
    });

  /** Labels facing the camera; recomputed only when the view centre moves. */
  function labelsInView(cLon: number, cLat: number, zoom: number): LabelFeature[] {
    if (zoom > 6) return labelFeatures; // the horizon is off-screen
    const key = `${Math.round(cLon)},${Math.round(cLat)},${labelFeatures.length}`;
    if (visibleLabels.key !== key) {
      // the camera is close enough that the horizon sits well short of 90° from
      // the view centre, and labels near it are foreshortened anyway
      const data = labelFeatures.filter(
        (f) => cosAngle(f.geometry.coordinates[0], f.geometry.coordinates[1], cLon, cLat) > 0.42,
      );
      visibleLabels = { key, data };
    }
    return visibleLabels.data;
  }

  /** Airports with a marker of their own lose their map label, so the code isn't drawn twice. */
  let unmarked: { base: LabelFeature[] | null; key: string; data: LabelFeature[] } = { base: null, key: "", data: [] };
  function withoutMarked(data: LabelFeature[], codes: string[]): LabelFeature[] {
    if (!codes.length) return data;
    const key = codes.join(",");
    if (unmarked.base !== data || unmarked.key !== key) {
      const skip = new Set(codes);
      const keep = (f: LabelFeature) => !(f.properties.layerName === "aerodrome_label" && skip.has(String(f.properties.iata)));
      unmarked = { base: data, key, data: data.filter(keep) };
    }
    return unmarked.data;
  }

  const collision = new CollisionFilterExtension();
  const LABEL_EXTENSIONS = [collision];
  const LABEL_PARAMETERS = { ...NO_CULL, depthCompare: "always", depthWriteEnabled: false } as const;
  const LABEL_OUTLINE: [number, number, number, number] = [6, 12, 19, 220];
  const LABEL_FONT_SETTINGS = { sdf: true, fontSize: 64, buffer: 6 };
  const LABEL_COLLISION_PROPS = { sizeScale: 1.3 };
  const labelPosition = (f: LabelFeature) => f.geometry.coordinates;
  const textLayer = (data: LabelFeature[]) =>
    new TextLayer<LabelFeature, CollisionFilterExtensionProps<LabelFeature>>({
      id: "labels",
      data,
      getPosition: labelPosition,
      getText: labelText,
      getSize: labelSize,
      getColor: labelColor,
      fontFamily: LABEL_FONT,
      fontWeight: 600,
      characterSet: "auto",
      fontSettings: LABEL_FONT_SETTINGS,
      outlineWidth: 3,
      outlineColor: LABEL_OUTLINE,
      sizeUnits: "pixels",
      billboard: true,
      pickable: true,
      parameters: LABEL_PARAMETERS,
      extensions: LABEL_EXTENSIONS,
      collisionGroup: "labels",
      getCollisionPriority: labelPriority,
      collisionTestProps: LABEL_COLLISION_PROPS,
    });

  const BUILDING_LOAD_OPTIONS = { mvt: { layers: ["building"], workerUrl: mvtWorkerUrl } };
  const BUILDING_MATERIAL = {
    ambient: 0.42,
    diffuse: 0.6,
    shininess: 16,
    specularColor: [60, 66, 80] as [number, number, number],
  };
  const BUILDING_COLOR: [number, number, number] = [58, 68, 84];
  const buildingHeight = (f: { properties: Record<string, unknown> }) =>
    f.properties.hide_3d ? 0 : Number(f.properties.render_height ?? 6);
  const buildingLayer = (url: string) =>
    new MVTLayer({
      id: "buildings",
      data: url,
      minZoom: 13,
      maxZoom: 14,
      loadOptions: BUILDING_LOAD_OPTIONS,
      extruded: true,
      filled: true,
      stroked: false,
      getElevation: buildingHeight,
      getFillColor: BUILDING_COLOR,
      material: BUILDING_MATERIAL,
      parameters: NO_CULL,
    });

  // Night shading, recomputed each minute: four nested bands (sunset and the
  // three twilights) whose overlap darkens toward the deep night side.
  const NIGHT_COLOR: [number, number, number, number] = [2, 6, 14, 46];
  let night: { minute: number; polygons: LonLat[][] } = { minute: -1, polygons: [] };
  function nightLayer(now: number) {
    const minute = Math.floor(now / 60_000);
    if (night.minute !== minute) night = { minute, polygons: nightPolygons(now) };
    return new SolidPolygonLayer<LonLat[]>({
      id: "night",
      data: night.polygons,
      getPolygon: (d) => d,
      getFillColor: NIGHT_COLOR,
      parameters: SURFACE_NO_CULL,
      updateTriggers: { getPolygon: minute },
    });
  }

  // Precipitation radar from RainViewer: the latest frame, refreshed every 10 minutes.
  let radarUrl: string | null = null;
  let radarCheckedAt = 0;
  async function refreshRadar() {
    radarCheckedAt = Date.now();
    try {
      const maps = await (await fetch("https://api.rainviewer.com/public/weather-maps.json")).json();
      const latest = maps.radar?.past?.at(-1);
      radarUrl = latest ? `${maps.host}${latest.path}/256/{z}/{x}/{y}/2/1_1.png` : null;
      render();
    } catch {
      radarUrl = null;
    }
  }
  // NOAA MRMS over the US: measured in 3D, checked every two minutes (the
  // backend refetches at most every four). RainViewer fills in elsewhere.
  let measured: MeasuredVolume[] = [];
  let measuredCheckedAt = 0;
  let measuredLoading = false;
  async function refreshMeasured(area: BoundingBox | null) {
    if (measuredLoading) return; // a slow load must not race the next poll
    measuredLoading = true;
    measuredCheckedAt = Date.now();
    try {
      const frames = await radarFrame(area);
      // a scan already in hand is kept: the sources publish on their own
      // schedules, and re-triangulating an unchanged one costs a dozen meshes
      const held = frames.map((f) => measured.find((v) => v.source === f.source && v.time === f.time));
      if (held.length === measured.length && held.every(Boolean)) return;
      // dropped volumes need no cleanup: their GPU buffers go with their layers
      measured = await Promise.all(frames.map((f, i) => held[i] ?? loadVolume(f, radarImage)));
      // RainViewer stands back where some service measures the weather in 3D
      setRadarCoverage(measured.map((v) => v.coverage));
      render();
    } catch (e) {
      console.warn("measured radar unavailable:", e);
    } finally {
      measuredLoading = false;
      measuredCheckedAt = Date.now();
    }
  }
  const getRadarTile = ({ url, index, signal }: { url?: string | null; index: { x: number; y: number; z: number }; signal?: AbortSignal }) =>
    url ? loadRadarTile(url, index, signal) : null;
  const RADAR_LIFTED = { ...NO_CULL, depthWriteEnabled: false } as const;
  const FEET_PER_KM = 3280.84;
  /**
   * Looking straight down, stacked layers only smear outward in perspective:
   * the 3D stack fades in as the camera tilts.
   */
  const liftFor = (pitch: number) => Math.round(clamp01((pitch - 10) / 30) * 10) / 10;
  /**
   * Radar polygons from prebuilt binary data (radar-mesh.ts): triangulated
   * already, so deck.gl only uploads them. A lifted layer is raised by its
   * model matrix rather than by its positions, so a change of height scale
   * moves it without touching the data.
   *
   * Walls (`bottom` set) have z from 0 to 1 in their data; the model matrix
   * scales that to the gap between `bottom` and `z` and lifts it to `bottom`.
   * deck.gl applies the whole matrix to longitude, latitude and metres before
   * projecting onto the globe, so the stretch is exact.
   */
  const radarPolygons = (
    props: Record<string, unknown> | null,
    own: { id: string; data: PolygonData; opacity: number; parameters: object; z?: number; bottom?: number },
  ) =>
    new SolidPolygonLayer(props ?? {}, {
      id: own.id,
      data: own.data as never,
      _normalize: false,
      positionFormat: own.bottom === undefined ? "XY" : "XYZ",
      filled: true,
      extruded: false,
      // flat colour, like the images were: no shading from the scene light
      material: false,
      opacity: own.opacity,
      parameters: own.parameters,
      modelMatrix:
        own.bottom !== undefined
          ? [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, (own.z ?? 0) - own.bottom, 0, 0, 0, own.bottom, 1]
          : own.z
            ? [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, own.z, 1]
            : null,
    });

  // deck.gl's TileLayer renders each tile's sublayers once and keeps them, so
  // anything that changes with the camera (tilt, height exaggeration) can't
  // live in there: tiles loaded at different moments would disagree, with a
  // hard edge where they meet. The tiles carry only the flat picture; the
  // stacked layers are built here from the tiles in view.
  const renderRadarTile = (props: any) => {
    const tile = props.data as RadarTile | null;
    if (!tile?.ground) return null;
    // both faces: the triangles' winding isn't kept consistent
    return radarPolygons(props, { id: `${props.id}-ground`, data: tile.ground, opacity: 0.55, parameters: SURFACE_NO_CULL });
  };
  let radarTiles: RadarTile[] = [];
  let radarVersion = 0;
  const onRadarTiles = (tiles: { content: RadarTile | null }[]) => {
    const next = tiles.map((t) => t.content).filter((c): c is RadarTile => c !== null);
    // fired again whenever the view settles; the same tiles need nothing rebuilt
    const known = new Set(radarTiles);
    if (next.length === radarTiles.length && next.every((t) => known.has(t))) return;
    radarTiles = next;
    radarVersion++;
  };
  /**
   * The lifted layers of all tiles in view, merged: one slice layer and one
   * wall layer per height rather than two per tile per height (hundreds of
   * draw calls, each tile's whole mesh uploaded again for every height).
   * Merged again only when the tiles change.
   */
  let liftedData: { version: number; levels: { km: number; slices: PolygonData | null; walls: PolygonData | null }[] } = {
    version: -1,
    levels: [],
  };
  function mergedLevels() {
    if (liftedData.version !== radarVersion) {
      const levels = RADAR_LEVELS_KM.map((km, i) => {
        // each tile's levels run up from the lowest without gaps, so index i is this height
        const parts = radarTiles.flatMap((tile) => (tile.levels[i] ? [{ tile, fromBand: tile.levels[i].fromBand }] : []));
        return {
          km,
          slices: mergeMeshes(parts.map(({ tile, fromBand }) => ({ mesh: tile.slices, fromBand }))),
          walls: mergeMeshes(parts.flatMap(({ tile, fromBand }) => (tile.walls ? [{ mesh: tile.walls, fromBand }] : []))),
        };
      });
      liftedData = { version: radarVersion, levels };
    }
    return liftedData.levels;
  }
  let lifted: { key: string; layers: Layer[] } = { key: "", layers: [] };
  /** Built again only when the tiles, the height scale or the tilt change. */
  function radarLifted(ex: number, lift: number): Layer[] {
    const scale = Math.round(ex * 10) / 10;
    const key = `${scale}/${lift}/${radarVersion}`;
    if (lifted.key !== key) {
      const layers: Layer[] = [];
      if (lift > 0) {
        // higher layers keep only the stronger echoes, so cells rise like towers
        mergedLevels().forEach(({ km, slices, walls }, i) => {
          if (!slices) return;
          const opacity = Math.max(0.16, 0.34 - i * 0.03) * lift;
          const z = elevation(km * FEET_PER_KM, scale);
          layers.push(radarPolygons(null, { id: `radar-lift-${km}`, data: slices, opacity, parameters: RADAR_LIFTED, z }));
          // walled down to the layer below, the lowest to the ground, so the stack has no gaps
          const bottom = i ? elevation(RADAR_LEVELS_KM[i - 1] * FEET_PER_KM, scale) : 0;
          if (walls) {
            layers.push(radarPolygons(null, { id: `radar-wall-${km}`, data: walls, opacity, parameters: RADAR_LIFTED, z, bottom }));
          }
        });
      }
      lifted = { key, layers };
    }
    return lifted.layers;
  }
  const radarLayer = (url: string) =>
    new TileLayer<RadarTile | null>({
      // the tiles have the measured area punched out of them, so they reload
      // when its coverage changes (not with every scan); the tiles it doesn't
      // reach come back from the pool's cache without being contoured again
      id: `radar-${url}-${radarCoverageKey()}`,
      data: url,
      minZoom: 0,
      maxZoom: 7,
      tileSize: 256,
      getTileData: getRadarTile,
      onViewportLoad: onRadarTiles,
      renderSubLayers: renderRadarTile,
      parameters: SURFACE,
    });

  function measuredLayers(volume: MeasuredVolume, ex: number, lift: number): Layer[] {
    const layers: Layer[] = [];
    if (volume.ground) {
      layers.push(
        radarPolygons(null, {
          id: `${volume.source}-ground-${volume.time}`,
          data: volume.ground,
          opacity: 0.7,
          parameters: SURFACE_NO_CULL,
        }),
      );
    }
    // the lowest height is already in the ground composite
    const scale = Math.round(ex * 10) / 10;
    let bottom = 0;
    for (const { km, data, walls } of lift ? volume.levels.filter((l) => l.km > 1) : []) {
      const z = elevation(km * FEET_PER_KM, scale);
      const opacity = 0.18 * lift;
      layers.push(radarPolygons(null, { id: `${volume.source}-${km}-${volume.time}`, data, opacity, parameters: RADAR_LIFTED, z }));
      // walled down to the layer below, the lowest to the ground, so the stack has no gaps
      if (walls) {
        layers.push(
          radarPolygons(null, { id: `${volume.source}-wall-${km}-${volume.time}`, data: walls, opacity, parameters: RADAR_LIFTED, z, bottom }),
        );
      }
      bottom = z;
    }
    return layers;
  }

  const dashed = new PathStyleExtension({ dash: true });
  const DASHED = [dashed];
  const ROUTE_AHEAD: [number, number, number, number] = [252, 180, 66, 210];
  const ROUTE_BEFORE: [number, number, number, number] = [241, 238, 231, 110];
  const ROUTE_FILED: [number, number, number, number] = [241, 238, 231, 150];

  /** A ground path that climbs or descends linearly between two heights. */
  function ramp(points: LonLat[], fromZ: number, toZ: number): [number, number, number][] {
    const n = Math.max(1, points.length - 1);
    return points.map(([lon, lat], i) => [lon, lat, fromZ + ((toZ - fromZ) * i) / n]);
  }

  function routeLayers(r: Route, pos: [number, number, number], trailStartZ: number): Layer[] {
    const layers: Layer[] = [];
    if (r.filed.length > 1) {
      layers.push(
        new PathLayer({
          id: "route-filed",
          data: [{ path: r.filed }],
          getPath: (d) => d.path,
          getColor: ROUTE_FILED,
          getWidth: 1.5,
          widthUnits: "pixels",
          parameters: { ...NO_CULL, depthWriteEnabled: false },
        }),
        new ScatterplotLayer({
          id: "route-waypoints",
          data: r.filed,
          getPosition: (d: LonLat) => d,
          getRadius: 2.5,
          radiusUnits: "pixels",
          getFillColor: ROUTE_FILED,
          parameters: { ...NO_CULL, depthWriteEnabled: false },
        }),
      );
    }
    if (r.before.length > 1) {
      layers.push(
        new PathLayer<{ path: [number, number, number][] }, PathStyleExtensionProps>({
          id: "route-before",
          data: [{ path: ramp(r.before, 0, trailStartZ) }],
          getPath: (d) => d.path,
          getColor: ROUTE_BEFORE,
          getWidth: 1.5,
          widthUnits: "pixels",
          getDashArray: [3, 3],
          dashJustified: true,
          extensions: DASHED,
          parameters: NO_CULL,
        }),
      );
    }
    if (r.ahead.length > 1) {
      // from the aircraft as drawn now, descending to the destination
      const ahead = [[pos[0], pos[1]] as LonLat, ...r.ahead.slice(1)];
      layers.push(
        new PathLayer<{ path: [number, number, number][] }, PathStyleExtensionProps>({
          id: "route-ahead",
          data: [{ path: ramp(ahead, pos[2], 0) }],
          getPath: (d) => d.path,
          getColor: ROUTE_AHEAD,
          getWidth: 2,
          widthUnits: "pixels",
          getDashArray: [4, 3],
          dashJustified: true,
          extensions: DASHED,
          parameters: NO_CULL,
          updateTriggers: { getPath: tick },
        }),
      );
    }
    const ends = [r.origin, r.destination].filter((a): a is Airport => Boolean(a));
    if (ends.length) layers.push(...airportMarkers("route-airports", ends));
    return layers;
  }

  const MARKER_FILL: [number, number, number, number] = [252, 180, 66, 255];
  const MARKER_LINE: [number, number, number, number] = [6, 12, 19, 230];
  const airportPosition = (a: Airport): LonLat => [a.lon, a.lat];
  const airportCode = (a: Airport) => a.iata || a.icao;
  function airportMarkers(id: string, airports: Airport[]): Layer[] {
    return [
      new ScatterplotLayer<Airport>({
        id: `${id}-dots`,
        data: airports,
        getPosition: airportPosition,
        getRadius: 5,
        radiusUnits: "pixels",
        getFillColor: MARKER_FILL,
        stroked: true,
        getLineColor: MARKER_LINE,
        getLineWidth: 2,
        lineWidthUnits: "pixels",
        billboard: true,
        parameters: { ...NO_CULL, depthCompare: "always" },
      }),
      new TextLayer<Airport>({
        id: `${id}-codes`,
        data: airports,
        getPosition: airportPosition,
        getText: airportCode,
        getSize: 13,
        getColor: [241, 238, 231, 255],
        getPixelOffset: [0, -16],
        fontFamily: LABEL_FONT,
        fontWeight: 700,
        fontSettings: LABEL_FONT_SETTINGS,
        outlineWidth: 3,
        outlineColor: LABEL_OUTLINE,
        billboard: true,
        parameters: LABEL_PARAMETERS,
      }),
    ];
  }

  const mix = (a: [number, number, number], b: [number, number, number], t: number): [number, number, number] => [
    a[0] + (b[0] - a[0]) * t,
    a[1] + (b[1] - a[1]) * t,
    a[2] + (b[2] - a[2]) * t,
  ];

  function positionOf(f: LiveFlight, now: number, ex: number): [number, number, number] {
    const [lon, lat] = extrapolate(f, now);
    return [lon, lat, elevation(f.alt, ex)];
  }

  function buildLayers(): Layer[] {
    const now = Date.now();
    const { longitude: cLon, latitude: cLat, zoom, bearing, pitch } = view;
    const flat = zoom > FLAT_ABOVE_ZOOM;
    const ex = effectiveExaggeration(exaggeration, zoom);

    const mpp = metersPerPixel(zoom, cLat);
    const minPx = minPxAt(zoom);
    /** On-screen size of an aircraft whose largest dimension is `meters`. */
    const screenPx = (meters: number) => {
      // zoomed out everything sits at the floor, so the floor itself has to say
      // big from small: an A380 reads about 1.8x an A320, a Cessna about 0.6x
      const floor = minPx * Math.min(2, Math.max(0.6, (meters / MODEL_SIZE.narrow) ** 0.8));
      return Math.min(MAX_PX, Math.max(floor, meters / mpp));
    };
    const depth = depthAt(zoom);
    const material = meshMaterial(depth);
    // the globe's local frame is rotated half a turn from the flat projection's
    const yawOffset = flat ? 0 : 180;

    // skip aircraft that are off-screen or behind the globe
    const radius = visibleRadius(zoom, pitch, container.clientWidth, container.clientHeight);
    const minCos = Math.cos((radius * Math.PI) / 180);
    // airline colours fade in over zoom 6-8, as the models gain depth
    const liveryAmount = showLiveries ? Math.round(clamp01((zoom - 6) / 2) * 10) / 10 : 0;
    // Straight down and wide: top-down silhouettes, the right idiom for a map
    // read from above. Tilted, or close in, the real shapes instead.
    const useIcons = pitch <= FLAT_PITCH && zoom < CLOSE_ZOOM && aircraftModels.loaded;
    const detail = zoom >= CLOSE_ZOOM ? 1 : 0;

    type Group = {
      /** The published model for this type, or `null` for the built-in shapes. */
      real: AircraftModel | null;
      kind: ModelKind;
      flights: LiveFlight[];
      positions: [number, number, number][];
      liveries: Livery[];
      index: Map<number, number>;
    };
    // the aircraft the camera is locked to is always drawn, even if the
    // camera is aimed far enough ahead of it to put it past the cull radius
    const lockedOn = follow ? selected?.id : undefined;
    const groups = new Map<string, Group>();
    const icons: LiveFlight[] = [];
    const iconPositions: [number, number, number][] = [];
    const iconNames: string[] = [];
    const iconSizes: number[] = [];
    const iconIndex = new Map<number, number>();

    for (const f of flights) {
      if (f.id !== lockedOn && cosAngle(f.lon, f.lat, cLon, cLat) <= minCos) continue;
      const real = aircraftModels.forFlight(f.typecode, f.icon);
      if (useIcons && real) {
        iconIndex.set(f.id, icons.length);
        icons.push(f);
        iconPositions.push(positionOf(f, now, ex));
        iconNames.push(real.name);
        iconSizes.push(real.size);
        continue;
      }
      // balloons, ground vehicles and anything else with no published model
      const [kind] = modelFor(f.icon);
      const id = real ? `t-${real.name}` : `k-${kind}`;
      let group = groups.get(id);
      if (!group) groups.set(id, (group = { real, kind, flights: [], positions: [], liveries: [], index: new Map() }));
      group.index.set(f.id, group.flights.length);
      group.flights.push(f);
      group.positions.push(positionOf(f, now, ex));
      if (liveryAmount > 0) group.liveries.push(liveries.forFlight(f));
    }

    const layers: Layer[] = [earthLayer(basemap)];
    if (basemap === "satellite") layers.push(imageryLayer());
    else {
      layers.push(landLayer());
      if (tileUrl) layers.push(vectorMapLayer(tileUrl, flat));
    }
    if (weather) {
      if (Date.now() - radarCheckedAt > 600_000) refreshRadar();
      const lift = liftFor(pitch);
      if (radarUrl) layers.push(radarLayer(radarUrl), ...radarLifted(ex, lift));
      // only the sources on screen are worth fetching; each is tens of megabytes
      if (isTauri() && Date.now() - measuredCheckedAt > 120_000) {
        refreshMeasured(viewArea(view, container.clientWidth, container.clientHeight));
      }
      for (const volume of measured) layers.push(...measuredLayers(volume, ex, lift));
    }
    if (daylight) layers.push(nightLayer(now));
    if (tileUrl && buildings && flat && zoom >= 13) layers.push(buildingLayer(tileUrl));
    if (tileUrl && labels && fontsReady) {
      const marked = [airport, ...(selected && route ? [route.origin, route.destination] : [])]
        .map((a) => a?.iata)
        .filter((code): code is string => Boolean(code));
      layers.push(labelTileLayer(tileUrl), textLayer(withoutMarked(labelsInView(cLon, cLat, zoom), marked)));
    }

    if (selected) {
      const pos = positionOf(selected, now, ex);
      const trail = (details?.trail ?? []).filter((p) => p.latitude || p.longitude);
      const path = trail.map((p) => [p.longitude, p.latitude, elevation(p.altitude, ex)]);
      path.push(pos);
      const colors = trail.map((p) => [...altitudeColor(p.altitude), 255]);
      colors.push([...altitudeColor(selected.alt), 255]);
      layers.push(
        // the trail's shadow on the ground
        new PathLayer({
          id: "trail-shadow",
          data: [{ path: path.map(([lon, lat]) => [lon, lat, 0]) }],
          getPath: (d) => d.path,
          getColor: [241, 238, 231, 60],
          getWidth: 1.5,
          widthUnits: "pixels",
          parameters: { ...NO_CULL, depthWriteEnabled: false },
        }),
        new PathLayer({
          id: "trail",
          data: [{ path, colors }],
          getPath: (d) => d.path,
          getColor: (d) => d.colors,
          getWidth: 3,
          widthUnits: "pixels",
          jointRounded: true,
          capRounded: true,
          parameters: NO_CULL,
          updateTriggers: { getPath: tick },
        }),
      );
      if (route) layers.push(...routeLayers(route, pos, path.length > 1 ? path[0][2] : pos[2]));
    }
    if (airport) layers.push(...airportMarkers("focus-airport", [airport]));

    const meshes = models();
    const selectedId = selected?.id;
    for (const [id, group] of groups) {
      const real = group.real?.lods[Math.min(detail, group.real.lods.length - 1)];
      const shape = real ?? meshes[group.kind];
      // the published meshes carry their own real-world size; the built-in
      // ones are drawn at a nominal size and stretched per aircraft class
      const metres = group.real?.size;
      const divisor = metres ?? MODEL_SIZE[group.kind];
      const shared = {
        data: group.flights,
        getPosition: (_: LiveFlight, { index }: { index: number }) => group.positions[index],
        getOrientation: (f: LiveFlight): [number, number, number] => [0, yawOffset - f.track, 0],
        getScale: (f: LiveFlight): [number, number, number] => {
          const s = (screenPx(metres ?? sizeOf(f.icon)) * mpp) / divisor;
          // flattened into a silhouette when zoomed out
          return [s, s, s * Math.max(0.04, depth)];
        },
        material,
        pickable: true,
        // one highlight across all of an aircraft's parts
        highlightedObjectIndex: hoveredId !== null ? (group.index.get(hoveredId) ?? -1) : -1,
        highlightColor: [252, 180, 66, 150] as [number, number, number, number],
        parameters: NO_CULL,
      };
      const triggers = { getPosition: tick, getOrientation: yawOffset, getScale: [mpp, minPx, depth] };
      if (liveryAmount === 0) {
        layers.push(
          new SimpleMeshLayer<LiveFlight>({
            ...shared,
            id: `aircraft-${id}-${detail}`,
            mesh: shape.combined,
            getColor: (f) => (f.id === selectedId ? SELECTED_COLOR : AIRCRAFT_COLOR),
            updateTriggers: { ...triggers, getColor: selectedId },
          }),
        );
        continue;
      }
      for (const [part, mesh] of shape.parts) {
        layers.push(
          new SimpleMeshLayer<LiveFlight>({
            ...shared,
            id: `aircraft-${id}-${detail}-${part}`,
            mesh,
            getColor: (_, { index }) => mix(AIRCRAFT_COLOR, group.liveries[index][part as Part], liveryAmount),
            updateTriggers: { ...triggers, getColor: [liveryAmount, liveries.version] },
          }),
        );
      }
    }

    if (icons.length) {
      layers.push(
        new IconLayer<LiveFlight>({
          id: "aircraft-icons",
          data: icons,
          getPosition: (_, { index }) => iconPositions[index],
          getIcon: (_, { index }) => iconNames[index],
          iconAtlas: aircraftModels.iconAtlas!,
          iconMapping: aircraftModels.iconMapping,
          // the silhouettes are drawn 94% of the way across their cell
          getSize: (_, { index }) => screenPx(iconSizes[index]) * 1.06,
          // the icons point north; `getAngle` turns anticlockwise on screen,
          // and a rotated map has already turned the world underneath them
          getAngle: (f) => bearing - f.track,
          getColor: (f) => (f.id === selectedId ? SELECTED_COLOR : AIRCRAFT_COLOR),
          sizeUnits: "pixels",
          billboard: true,
          pickable: true,
          highlightedObjectIndex: hoveredId !== null ? (iconIndex.get(hoveredId) ?? -1) : -1,
          highlightColor: [252, 180, 66, 150],
          parameters: NO_CULL,
          updateTriggers: {
            getPosition: tick,
            getAngle: [tick, bearing],
            getSize: [mpp, minPx],
            getColor: selectedId,
          },
        }),
      );
    }

    // ring the selection, drawn over neighbouring aircraft but not through the globe
    if (selected && cosAngle(selected.lon, selected.lat, cLon, cLat) > 0) {
      layers.push(
        new ScatterplotLayer({
          id: "selection-ring",
          data: [positionOf(selected, now, ex)],
          getPosition: (d) => d,
          getRadius: screenPx(sizeOf(selected.icon)) * 0.6 + 4,
          radiusUnits: "pixels",
          stroked: true,
          filled: false,
          getLineColor: [252, 180, 66, 230],
          getLineWidth: 1.5,
          lineWidthUnits: "pixels",
          billboard: true,
          parameters: { ...NO_CULL, depthCompare: "always" },
        }),
      );
    }
    return layers;
  }

  const DEGREES = Math.PI / 180;
  const METRES_PER_DEGREE = 111_320;
  /** deck.gl parks the camera this many viewport heights back from its target. */
  const CAMERA_HEIGHTS = 1.5;
  /** Whether the wheel zooms about the cursor or the centre of the screen. */
  let zoomAround: "center" | "pointer" = "pointer";
  /** Locked to an aircraft, a plain drag orbits it instead of panning away. */
  const controllerFor = (locked: boolean) =>
    ({ type: NorthUpGlobeController, zoomAround: locked ? "center" : "pointer", dragMode: locked ? "rotate" : "pan" }) as const;

  /**
   * The view with its centre moved so the camera is locked on the aircraft.
   * Gestures run it too (see `onViewStateChange`): re-aiming only on a timer
   * leaves the aim stale for every frame of a zoom, and the aircraft jumps.
   */
  function lockedView(v: ViewState, flight: LiveFlight): ViewState {
    const [lon, lat] = extrapolate(flight, Date.now());
    const altitude = elevation(flight.alt, effectiveExaggeration(exaggeration, v.zoom));
    // Close in, the projection is flat and can raise the camera's target to the
    // aircraft's own height, which centres it at any tilt, bearing and zoom. A
    // ground point aimed along the bearing can't: the camera sits only a few
    // hundred metres out by street level, below a real cruise altitude.
    if (v.zoom > FLAT_ABOVE_ZOOM) return { ...v, longitude: lon, latitude: lat, position: [0, 0, altitude] };
    // The globe has no such offset, and the camera aims at a point on the
    // ground, so once the view is tilted an aircraft drawn at altitude rides
    // above the centre of the screen. Aiming further along the bearing puts
    // the aircraft itself on the camera's axis.
    const pitch = v.pitch * DEGREES;
    const camera = CAMERA_HEIGHTS * container.clientHeight * metersPerPixel(v.zoom, lat);
    // Only so far, though: aim past the camera itself and the aircraft ends
    // up behind it, leaving the view staring at empty ground ahead.
    const height = Math.min(altitude, 0.6 * camera * Math.cos(pitch));
    const lift = height * Math.tan(pitch);
    const latitude = Math.max(-85, Math.min(85, lat + (lift * Math.cos(v.bearing * DEGREES)) / METRES_PER_DEGREE));
    const longitude =
      lon + (lift * Math.sin(v.bearing * DEGREES)) / (METRES_PER_DEGREE * Math.max(0.05, Math.cos(lat * DEGREES)));
    return { ...v, longitude, latitude, position: undefined };
  }

  function render() {
    tick++;
    // locked to an aircraft, the wheel has to zoom about the centre, or every
    // notch shoves the aircraft aside and the lock drags it straight back; a
    // drag orbits for the same reason
    const wanted = follow && selected ? "center" : "pointer";
    if (wanted !== zoomAround) {
      zoomAround = wanted;
      deck?.setProps({ controller: controllerFor(wanted === "center") });
    }
    if (follow && selected && Date.now() > transitionUntil) {
      view = lockedView(view, selected);
      // deck.gl reports gestures back through `onViewStateChange` but not the
      // moves we make ourselves, so say where the camera went: the feed area
      // and the off-screen culling both read it
      onviewchange?.(view);
      raised = Boolean(view.position);
      deck?.setProps({ initialViewState: { ...view, ...LIMITS } });
    } else if (raised && !(follow && selected)) {
      // the lock was released with the camera raised: bring it back to the ground
      raised = false;
      view = { ...view, position: undefined };
      // `position` is the flat projection's; GlobeViewState doesn't declare it
      const grounded: ViewState = { ...view, position: [0, 0, 0] };
      deck?.setProps({ initialViewState: { ...grounded, ...LIMITS } });
    }
    deck?.setProps({ layers: buildLayers() });
  }

  /** Re-render on an interval that depends on zoom: motion is invisible zoomed out. */
  function loop(t: number) {
    // locked on an aircraft the camera and the aircraft must move together
    // every frame, or at street zoom each 50 ms step is many pixels of jitter
    const locked = follow && selected && Date.now() > transitionUntil;
    const interval = locked && view.zoom > 7 ? 0 : view.zoom > 7 ? 50 : view.zoom > 4 ? 150 : 500;
    if (t - lastTick >= interval) {
      lastTick = t;
      render();
    }
    frame = requestAnimationFrame(loop);
  }

  function goTo(target: Partial<ViewState>, duration: number, props: (keyof ViewState)[]) {
    transitionUntil = Date.now() + duration;
    deck?.setProps({
      initialViewState: {
        ...view,
        ...target,
        ...LIMITS,
        transitionDuration: duration,
        transitionInterpolator: new LinearInterpolator(props),
      },
    });
  }

  /** Fly to a point; without a zoom, far targets land at a regional view rather than street level. */
  export function flyTo(longitude: number, latitude: number, zoom?: number) {
    const near = cosAngle(longitude, latitude, view.longitude, view.latitude) > Math.cos((2 * Math.PI) / 180);
    const target = {
      longitude: unwrapLongitude(view.longitude, longitude),
      latitude,
      zoom: Math.min(zoom ?? (near ? Math.max(view.zoom, 8) : 8), LIMITS.maxZoom),
    };
    goTo(target, 1500, ["longitude", "latitude", "zoom"]);
  }

  export function zoomBy(delta: number) {
    const zoom = Math.min(LIMITS.maxZoom, Math.max(LIMITS.minZoom, view.zoom + delta));
    goTo({ zoom }, 250, ["zoom"]);
  }

  /** North up and looking straight down. */
  export function resetOrientation() {
    goTo({ bearing: 0, pitch: 0 }, 400, ["bearing", "pitch"]);
  }

  export function setPitch(pitch: number) {
    goTo({ pitch }, 400, ["pitch"]);
  }

  const escape = (s: string) => s.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);

  function tooltip(info: PickingInfo) {
    if (info.layer?.id === "labels") {
      const p = (info.object as LabelFeature | undefined)?.properties;
      if (p?.layerName !== "aerodrome_label") return null;
      return {
        html: `<div class="tt-head">${escape(nameOf(p))}</div><div class="tt-sub">Open departures and arrivals</div>`,
        className: "aircraft-tooltip",
      };
    }
    return flightTooltip(info as PickingInfo<LiveFlight>);
  }

  function flightTooltip({ object, layer }: PickingInfo<LiveFlight>) {
    if (!object || !layer?.id.startsWith("aircraft-")) return null;
    const route = object.origin || object.destination ? `${object.origin || "?"} – ${object.destination || "?"}` : "";
    const altitude = object.onGround ? "On ground" : `${object.alt.toLocaleString()} ft`;
    return {
      html: `<div class="tt-head data">${escape(object.callsign || object.flight || "No callsign")}</div>
        <div class="tt-sub">${escape([object.typecode, object.reg].filter(Boolean).join(" · "))}</div>
        ${route ? `<div class="tt-row data">${escape(route)}</div>` : ""}
        <div class="tt-row data">${altitude} · ${object.speed} kt</div>`,
      className: "aircraft-tooltip",
    };
  }

  onMount(() => {
    deck = new Deck<GlobeView>({
      parent: container,
      views: new GlobeView({ resolution: 5 }),
      initialViewState: { ...view, ...LIMITS },
      controller: controllerFor(false),
      effects: [lighting],
      pickingRadius: 6,
      onViewStateChange: ({ viewState }) => {
        const { longitude, latitude, zoom, bearing = 0, pitch = 0 } = viewState as Partial<ViewState>;
        view = { longitude: longitude!, latitude: latitude!, zoom: zoom!, bearing, pitch };
        // a gesture while locked on is re-aimed in the same frame, not on the next tick
        const locked = follow && selected && Date.now() > transitionUntil;
        if (locked) view = lockedView(view, selected!);
        onviewchange?.(view);
        return locked ? { ...viewState, ...view } : viewState;
      },
      getTooltip: tooltip,
      getCursor: ({ isDragging, isHovering }) => (isDragging ? "grabbing" : isHovering ? "pointer" : "grab"),
      onHover: (info) => {
        const id = info.layer?.id.startsWith("aircraft-") ? ((info.object as LiveFlight | undefined)?.id ?? null) : null;
        if (id !== hoveredId) {
          hoveredId = id;
          render();
        }
      },
      onClick: (info) => {
        if (info.layer?.id.startsWith("aircraft-") && info.object) onselect(info.object as LiveFlight);
        else if (info.layer?.id === "labels") {
          const p = (info.object as LabelFeature | undefined)?.properties;
          if (p?.layerName === "aerodrome_label" && p.iata) onairport?.(String(p.iata));
        } else if (!info.layer) onselect(null);
      },
      layers: [earthLayer(basemap)],
    });
    onviewchange?.(view);
    frame = requestAnimationFrame(loop);
    // the label font must be loaded before deck.gl rasterises its glyph atlas
    document.fonts.load(`600 16px "${LABEL_FONT}"`).finally(() => {
      fontsReady = true;
      render();
    });
    // the published aircraft shapes; the built-in ones draw until they land
    aircraftModels.load().then(render);
    fetch(OPENFREEMAP)
      .then((r) => r.json())
      .then((tilejson: { tiles: string[] }) => {
        tileUrl = tilejson.tiles[0];
        render();
      })
      .catch(() => {}); // labels and buildings are optional extras
  });

  onDestroy(() => {
    cancelAnimationFrame(frame);
    deck?.finalize();
    deck = null;
  });

  // re-render immediately when inputs change, rather than waiting for the next tick
  $effect(() => {
    void [flights, selected, details, route, airport, exaggeration, basemap, labels, buildings];
    void [showLiveries, daylight, weather, follow];
    render();
  });
</script>

<div class="globe" bind:this={container}></div>

<style>
  .globe {
    position: absolute;
    inset: 0;
  }
  :global(.aircraft-tooltip) {
    background: var(--surface-raised) !important;
    color: var(--text-1) !important;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    padding: var(--space-2) var(--space-3) !important;
    font: var(--text-sm) / 1.45 var(--font-ui) !important;
    box-shadow: var(--shadow);
  }
  :global(.aircraft-tooltip .tt-head) {
    font-size: var(--text-base);
    font-weight: 650;
  }
  :global(.aircraft-tooltip .tt-sub) {
    color: var(--text-2);
  }
  :global(.aircraft-tooltip .tt-row) {
    color: var(--text-2);
    font-size: var(--text-sm);
  }
</style>
