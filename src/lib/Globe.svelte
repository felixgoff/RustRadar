<script lang="ts" module>
  export type ViewState = { longitude: number; latitude: number; zoom: number; bearing: number; pitch: number };
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
  import { BitmapLayer, PathLayer, ScatterplotLayer, SolidPolygonLayer, TextLayer } from "@deck.gl/layers";
  import { SimpleMeshLayer } from "@deck.gl/mesh-layers";
  import { SphereGeometry } from "@luma.gl/engine";
  import { load } from "@loaders.gl/core";
  import { MVTLoader } from "@loaders.gl/mvt";
  // decode vector tiles off the main thread with a bundled worker, not one from a CDN
  import mvtWorkerUrl from "@loaders.gl/mvt/mvt-worker.js?url";
  import type { Airport, FlightDetails, LiveFlight } from "./api";
  import { nightPolygons } from "./daynight";
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
    /** The user dragged the map while following. */
    onfollowend?: () => void;
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
    onfollowend,
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
  const MESH_MATERIAL = { ambient: 0.38, diffuse: 0.65, shininess: 48, specular: [255, 236, 200] };
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
  const renderRadarTile = (props: any) => {
    const [[west, south], [east, north]] = props.tile.boundingBox;
    return new BitmapLayer(props, {
      data: undefined,
      image: props.data,
      bounds: [west, south, east, north],
      _imageCoordinateSystem: COORDINATE_SYSTEM.CARTESIAN,
      opacity: 0.6,
      parameters: SURFACE,
    });
  };
  const radarLayer = (url: string) =>
    new TileLayer({
      id: `radar-${url}`,
      data: url,
      minZoom: 0,
      maxZoom: 7,
      tileSize: 256,
      renderSubLayers: renderRadarTile,
      parameters: SURFACE,
    });

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
    const { longitude: cLon, latitude: cLat, zoom, pitch } = view;
    const flat = zoom > FLAT_ABOVE_ZOOM;
    const ex = effectiveExaggeration(exaggeration, zoom);

    const mpp = metersPerPixel(zoom, cLat);
    const minPx = minPxAt(zoom);
    /** On-screen size of an aircraft whose largest dimension is `meters`. */
    const screenPx = (meters: number) => {
      // zoomed out everything sits at the floor; keep a hint of big versus small
      const floor = minPx * Math.min(1.25, Math.max(0.8, Math.sqrt(meters / MODEL_SIZE.narrow)));
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
    type Group = {
      flights: LiveFlight[];
      positions: [number, number, number][];
      liveries: Livery[];
      index: Map<number, number>;
    };
    const byKind = new Map<ModelKind, Group>();
    for (const f of flights) {
      if (cosAngle(f.lon, f.lat, cLon, cLat) <= minCos) continue;
      const [kind] = modelFor(f.icon);
      let group = byKind.get(kind);
      if (!group) byKind.set(kind, (group = { flights: [], positions: [], liveries: [], index: new Map() }));
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
      if (radarUrl) layers.push(radarLayer(radarUrl));
    }
    if (daylight) layers.push(nightLayer(now));
    if (tileUrl && buildings && flat && zoom >= 13) layers.push(buildingLayer(tileUrl));
    if (tileUrl && labels && fontsReady) {
      layers.push(labelTileLayer(tileUrl), textLayer(labelsInView(cLon, cLat, zoom)));
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
    for (const kind of MODEL_KINDS) {
      const group = byKind.get(kind);
      if (!group) continue;
      const shared = {
        data: group.flights,
        getPosition: (_: LiveFlight, { index }: { index: number }) => group.positions[index],
        getOrientation: (f: LiveFlight): [number, number, number] => [0, yawOffset - f.track, 0],
        getScale: (f: LiveFlight): [number, number, number] => {
          const s = (screenPx(sizeOf(f.icon)) * mpp) / MODEL_SIZE[kind];
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
            id: `aircraft-${kind}`,
            mesh: meshes[kind].combined,
            getColor: (f) => (f.id === selectedId ? SELECTED_COLOR : AIRCRAFT_COLOR),
            updateTriggers: { ...triggers, getColor: selectedId },
          }),
        );
        continue;
      }
      for (const [part, mesh] of meshes[kind].parts) {
        layers.push(
          new SimpleMeshLayer<LiveFlight>({
            ...shared,
            id: `aircraft-${kind}-${part}`,
            mesh,
            getColor: (_, { index }) => mix(AIRCRAFT_COLOR, group.liveries[index][part as Part], liveryAmount),
            updateTriggers: { ...triggers, getColor: [liveryAmount, liveries.version] },
          }),
        );
      }
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

  function render() {
    tick++;
    if (follow && selected && Date.now() > transitionUntil) {
      const [longitude, latitude] = extrapolate(selected, Date.now());
      deck?.setProps({ initialViewState: { ...view, longitude, latitude, ...LIMITS } });
    }
    deck?.setProps({ layers: buildLayers() });
  }

  /** Re-render on an interval that depends on zoom: motion is invisible zoomed out. */
  function loop(t: number) {
    const interval = view.zoom > 7 ? 50 : view.zoom > 4 ? 150 : 500;
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
      controller: true,
      effects: [lighting],
      pickingRadius: 6,
      onViewStateChange: ({ viewState }) => {
        const { longitude, latitude, zoom, bearing = 0, pitch = 0 } = viewState as Partial<ViewState>;
        view = { longitude: longitude!, latitude: latitude!, zoom: zoom!, bearing, pitch };
        onviewchange?.(view);
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
      onDragStart: () => {
        if (follow) onfollowend?.();
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
