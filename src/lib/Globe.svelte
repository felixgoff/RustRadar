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
  import { onDestroy, onMount, untrack } from "svelte";
  import {
    _CameraLight as CameraLight,
    AmbientLight,
    COORDINATE_SYSTEM,
    Deck,
    DirectionalLight,
    _GlobeView as GlobeView,
    _GlobeViewport as GlobeViewport,
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
  import {
    BitmapLayer,
    IconLayer,
    PathLayer,
    ScatterplotLayer,
    SolidPolygonLayer,
    TextLayer,
  } from "@deck.gl/layers";
  import { SimpleMeshLayer } from "@deck.gl/mesh-layers";
  import { SphereGeometry } from "@luma.gl/engine";
  import type { Device } from "@luma.gl/core";
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
    type Ship,
    type Train,
    type TrainRoute,
    type TrainBlock,
    trainBlocks as fetchTrainBlocks,
    type Sighting,
  } from "./api";
  import { NorthUpGlobeController } from "./controller";
  import { aircraftModels, type AircraftModel } from "./aircraft";
  import { aircraftLabel, altitudeLabel, labelPriority as aircraftLabelPriority, SELECTED_LABEL_PRIORITY } from "./aircraft-labels";
  import { nightPolygons } from "./daynight";
  import { RADAR_LEVELS_KM, type RadarTile } from "./radar";
  import type { PolygonData } from "./radar-mesh";
  import { ArenaLayer, MeshArena, type ArenaData, type ArenaPiece } from "./radar-arena";
  import { loadRadarTile, loadVolume, radarCoverageKey, setRadarCoverage, type MeasuredVolume } from "./radar-pool";
  import { liveries, type Livery, type Part } from "./liveries";
  import { distanceKm, trimToPosition, type LonLat, type PathPoint, type Route } from "./routes";
  import {
    AIRCRAFT_COLOR,
    altitudeColor,
    cosAngle,
    effectiveExaggeration,
    elevation,
    metersPerPixel,
    SELECTED_COLOR,
    unwrapLongitude,
    visibleRadius,
    viewArea,
    type Basemap,
  } from "./geo";
  import { MODEL_KINDS, MODEL_SIZE, modelFor, models, sizeOf, type ModelKind } from "./models";
  import { motion, type Pose } from "./motion";
  import { TAXIWAY_ZOOM, taxiwaysFor } from "./taxiways";
  import { screenAngle } from "./screen";
  import { localSun, SHADOW_MIN_ALPHA, shadowMatrix, shadowOpacity, sunDirection, sunlight } from "./shadow";
  import { BuildingLayer, buildingHeight } from "./building-layer";
  import HoverCard from "./HoverCard.svelte";
  import { flightDetails, isRateLimited } from "./api";
  import { HoverLoader, hoverInfo, routeProgress } from "./hover-details";
  import { reference } from "./reference.svelte";
  import { offsetEastNorth, shipMeshes, shipPlacement, type ShipClass } from "./ship-models";
  import { shipAtlas, shipColor, shipIcon, shipIconScale, shipIconSize, shipKindLabel } from "./ships";
  import { terrainLayers } from "./terrain-layer";
  import { RAIL_MIN_ZOOM, railLayers, SPEED_BANDS, SPEED_UNKNOWN } from "./rail-layer";
  import {
    delayText,
    headingAt,
    positionAt,
    trainAtlas,
    trainColorOf,
    trainIcon,
    trainIconScale,
    trainIconSize,
    trainLabel,
    trainPositionAge,
  } from "./trains";
  import { type RouteCursor, routeAhead, wayToStop } from "./train-route";
  import { isTrackpadScroll, pinched, type PinchEvent, wheelPixels, wrapBearing } from "./trackpad";

  interface Props {
    flights: LiveFlight[];
    /** Ships to draw on the water, under the aircraft. */
    ships?: Ship[];
    selected: LiveFlight | null;
    details: FlightDetails | null;
    /** The selected flight's route, filed or estimated. */
    route: Route | null;
    /** An airport to mark, e.g. the one whose board is open. */
    airport: Airport | null;
    /** A searched place to mark: a small dot and its name. */
    place?: { lon: number; lat: number; name: string } | null;
    /** Vertical exaggeration of altitudes when zoomed out, 1 = true scale. */
    exaggeration: number;
    basemap: Basemap;
    labels: boolean;
    /** Place-name size, as a multiple of the default (a setting). */
    labelScale: number;
    buildings: boolean;
    /** Draw the aircraft at all; off, none of them nor anything of the selected one's. */
    aircraft?: boolean;
    /** Write each aircraft's callsign beside it. */
    aircraftLabels?: boolean;
    /** Shade the night side of the planet. */
    daylight: boolean;
    /** Precipitation radar overlay. */
    weather: boolean;
    /** Shaded relief under the map. */
    terrain?: boolean;
    /** The railway network over the map. */
    railMap?: boolean;
    /** Trains to draw on land, over the ships and under the aircraft. */
    trains?: Train[];
    /** The selected train, ringed in the selection amber. */
    selectedTrain?: Train | null;
    /** The selected train's whole way along the track. */
    trainRoute?: TrainRoute | null;
    /** Signal blocks close in: free, or taken by the train drawn in them. */
    trainBlocks?: boolean;
    /** The selected train isn't on its route (it is out of date): ask for it again. */
    ontrainrouteoff?: () => void;
    /** A train was clicked. */
    ontrainselect?: (train: Train) => void;
    /** A train to highlight, e.g. the one under the pointer in the sidebar's list. */
    hoveredTrainId?: string | null;
    /** A ship to highlight, by MMSI. */
    hoveredShipMmsi?: number | null;
    /** Keep the camera centred on the selected aircraft. */
    follow: boolean;
    /** Keep the camera centred on the selected train or ship. */
    followVehicle?: boolean;
    onselect: (flight: LiveFlight | null) => void;
    /** The selected ship, ringed in the selection amber. */
    selectedShip?: Ship | null;
    /** A ship was clicked. */
    onshipselect?: (ship: Ship) => void;
    /** An airport label was clicked. */
    onairport?: (iata: string) => void;
    onviewchange?: (view: ViewState) => void;
  }

  let {
    flights,
    ships = [],
    selected,
    details,
    route,
    airport,
    place = null,
    exaggeration,
    basemap,
    labels,
    labelScale,
    buildings,
    aircraft: showAircraft = true,
    aircraftLabels = false,
    daylight,
    weather,
    terrain = false,
    railMap = false,
    trains = [],
    selectedTrain = null,
    trainRoute = null,
    trainBlocks = false,
    ontrainrouteoff,
    ontrainselect,
    hoveredTrainId = null,
    hoveredShipMmsi = null,
    follow,
    followVehicle = false,
    onselect,
    selectedShip = null,
    onshipselect,
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
  /** Following an aircraft, the camera can come down nearly level with it and circle it. */
  const FOLLOW_MAX_PITCH = 85;
  /** ...and close in until the camera is this many of its lengths away, so it can't end up inside it. */
  const FOLLOW_CLOSEST = 1.3;
  /** Past this the map tiles have long run out of detail. */
  const FOLLOW_MAX_ZOOM = 23;

  /**
   * The closest zoom while locked on an aircraft: until it fills the screen,
   * worked out from its real size, so the camera can't end up inside it.
   */
  function followMaxZoom(flight: LiveFlight): number {
    const size = aircraftModels.forFlight(flight.typecode, flight.icon)?.size ?? sizeOf(flight.icon);
    // the camera sits CAMERA_HEIGHTS viewport heights back from its target
    const closest = (FOLLOW_CLOSEST * size) / (CAMERA_HEIGHTS * Math.max(1, container?.clientHeight ?? 900));
    const zoom = Math.log2(metersPerPixel(0, flight.lat) / closest);
    return Math.min(FOLLOW_MAX_ZOOM, Math.max(LIMITS.maxZoom, zoom));
  }

  /**
   * The camera's limits, as handed to deck.gl. Locked on an aircraft they open
   * up: close in until it fills the screen and tilt down to almost level, to
   * look at it from any side.
   */
  function limits() {
    if (!(follow && selected)) return LIMITS;
    // GlobeController shifts its zoom limits by latitude (its zoomAdjust); undo
    // that so the aircraft's limit is the zoom actually reached
    const shift = Math.log2(Math.max(0.05, Math.cos(selected.lat * DEGREES)));
    return { minZoom: LIMITS.minZoom, maxZoom: followMaxZoom(selected) - shift, maxPitch: lockedMaxPitch };
  }
  /** The closest zoom reachable now, in the view's own terms. */
  const maxZoomNow = () => (follow && selected ? followMaxZoom(selected) : LIMITS.maxZoom);
  /** How far the locked camera may tilt right now; see `lockedView`. */
  let lockedMaxPitch = FOLLOW_MAX_PITCH;
  /** Whether the last render was locked on an aircraft, to notice the lock ending. */
  let wasLocked = false;
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
  // rises from the whole globe to city level).
  const MIN_PX_GLOBE = 8;
  const MIN_PX_CITY = 24;
  const minPxAt = (zoom: number) => MIN_PX_GLOBE + (MIN_PX_CITY - MIN_PX_GLOBE) * clamp01((zoom - 3) / 7);

  // Zoomed out, aircraft are flat, unshaded silhouettes; they gain depth and
  // shading between zoom 6 and 9.
  const depthAt = (zoom: number) => Math.round(clamp01((zoom - 6) / 3) * 10) / 10;
  /** Up to this tilt the view reads as a flat map, where silhouettes belong. */
  const FLAT_PITCH = 5;
  /** From this zoom the detailed meshes are worth their triangles. */
  const CLOSE_ZOOM = 11;
  /** Model depth from which landing gear is drawn (about zoom 8.5). */
  const GEAR_DEPTH = 0.8;
  /** Struts and tyres in one grey: light enough to read against the night map. */
  const GEAR_COLOR: [number, number, number] = [178, 182, 190];
  /**
   * Shadows fade in over these zooms: the models have full depth from zoom 9,
   * and before that a shadow would be a speck under a floor-sized silhouette.
   */
  const SHADOW_ZOOM: [number, number] = [9, 11];
  /** The app's navy ink, so a shadow darkens the map rather than greying it. */
  const SHADOW_COLOR: [number, number, number] = [4, 8, 16];
  // On the ground, under the aircraft. Every shadow, an aircraft's or a
  // building's, marks the stencil where it darkens, and only an unmarked pixel
  // takes one: the wing over the fuselage doesn't darken twice, nor two
  // shadows that overlap. (Depth can't do this reliably: overlapping triangles
  // on one plane interpolate slightly different depths, and the overlaps
  // speckle.) They test depth so buildings in front hide them, and write none;
  // the ground layers write none either, so there is nothing to fight.
  const SHADOW_PARAMETERS = {
    ...NO_CULL,
    depthWriteEnabled: false,
    depthCompare: "less-equal",
    stencilWriteMask: 0xff,
    stencilCompare: "equal",
    stencilPassOperation: "increment-clamp",
    stencilFailOperation: "keep",
    stencilDepthFailOperation: "keep",
  } as const;
  /** Lowest point (model z, metres) of each mesh, where it stands; worked out once per mesh. */
  const lowestPoints = new WeakMap<object, number>();
  function lowestZ(mesh: { attributes: Record<string, { value: ArrayLike<number> } | undefined> }): number {
    let z = lowestPoints.get(mesh);
    if (z === undefined) {
      const positions = mesh.attributes.positions?.value ?? [];
      z = 0;
      for (let i = 2; i < positions.length; i += 3) z = Math.min(z, positions[i]);
      lowestPoints.set(mesh, z);
    }
    return z;
  }
  // reused by the shadow accessors, which deck.gl copies out of at once
  const shadowMatrixOut = new Array<number>(16).fill(0);
  const shadowSunOut = [0, 0, 0];
  const shadowPositionOut: [number, number, number] = [0, 0, 0];
  const shadowColorOut: [number, number, number, number] = [...SHADOW_COLOR, 0];
  /** How far ahead (screen pixels at the view centre) an icon's heading is sampled. */
  const ICON_HEADING_STEP_PX = 20;
  /** The silhouettes' border: the app's navy ink (`--ink`), nearly opaque. */
  const OUTLINE_COLOR: [number, number, number, number] = [8, 12, 20, 215];
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

  /** The scene light's usual direction, a fixed key light from the north-east. */
  const KEY_LIGHT: [number, number, number] = [-1, -2, -3];
  const keyLight = new DirectionalLight({ color: [255, 244, 225], intensity: 0.7, direction: KEY_LIGHT });
  const KEY_LIGHT_UNIT = [...keyLight.direction];
  /** The sine of the usual light's altitude (about 53°). */
  const KEY_LIGHT_UP = -KEY_LIGHT_UNIT[2];
  const lighting = new LightingEffect({
    ambient: new AmbientLight({ color: [255, 255, 255], intensity: 1 }),
    camera: new CameraLight({ color: [255, 248, 235], intensity: 0.55 }),
    sun: keyLight,
  });
  /**
   * Points the key light along the real sun's rays, by `weight` (0..1) of the
   * way from its usual direction, so sunlit walls face the sun and agree with
   * the shadows. Only in the flat projection, whose common space is east,
   * north and up; deck.gl reads the direction afresh each frame.
   */
  function aimKeyLight(toSun: readonly number[], weight: number) {
    // From the sun's bearing, but no steeper than the usual light: a high sun
    // straight onto the roofs would leave every wall in the dark.
    const across = Math.hypot(toSun[0], toSun[1]) || 1;
    const up = Math.min(toSun[2], KEY_LIGHT_UP);
    const flat = Math.sqrt(1 - up * up) / across;
    const ray = [-toSun[0] * flat, -toSun[1] * flat, -up];
    const d = keyLight.direction;
    for (let i = 0; i < 3; i++) d[i] = KEY_LIGHT_UNIT[i] + (ray[i] - KEY_LIGHT_UNIT[i]) * weight;
    const n = Math.hypot(d[0], d[1], d[2]) || 1;
    for (let i = 0; i < 3; i++) d[i] /= n;
  }

  // Layers are rebuilt as cheap descriptors on every render; deck.gl matches
  // them by id and keeps their GPU resources and tile caches. Everything they
  // reference (meshes, callbacks, option objects) is created once so props
  // compare equal between renders.
  const EARTH_MESH = new SphereGeometry({ radius: 6.3e6, nlat: 36, nlong: 72 });
  // the dark map's land is the globe itself; under imagery it only shows at the poles
  const GROUND: Record<Basemap, [number, number, number]> = { dark: [50, 50, 50], satellite: [6, 16, 34] };
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
  const LAND: RGBA = [50, 50, 50, 255];
  const WATER: RGBA = [30, 30, 30, 255];
  const BORDER: RGBA = [118, 118, 118, 255];
  const BORDER_STATE: RGBA = [84, 84, 84, 255];
  const ROAD_MAJOR: RGBA = [88, 88, 88, 255];
  const ROAD_MINOR: RGBA = [66, 66, 66, 255];
  const RUNWAY: RGBA = [163, 119, 52, 255];
  const TAXIWAY: RGBA = [105, 80, 46, 255];
  const APRON: RGBA = [60, 60, 60, 255];
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
        return p.class === "runway" ? RUNWAY_PX : 1.5;
      default:
        return 0;
    }
  };
  const VECTOR_LOAD_OPTIONS = {
    mvt: { layers: ["water", "boundary", "transportation", "aeroway"], workerUrl: mvtWorkerUrl },
  };

  // Close in, a runway drawn as a centreline a few pixels wide ends up far
  // thinner than it is. It switches to its real width (the tiles don't carry
  // it: a typical one) once that is as wide as the pixel line, so nothing
  // jumps. Taxiways stay thin centre lines at every zoom.
  const RUNWAY_WIDTH_M = 45;
  const RUNWAY_PX = 4;
  const AEROWAY_LOAD_OPTIONS = { mvt: { layers: ["aeroway"], workerUrl: mvtWorkerUrl } };
  /** Runways at true width, over the map layer, for close in. */
  const aerowayLayer = (url: string, flat: boolean) =>
    new MVTLayer({
      id: flat ? "aeroway-true-flat" : "aeroway-true-globe",
      data: url,
      minZoom: 0,
      maxZoom: 14,
      loadOptions: AEROWAY_LOAD_OPTIONS,
      filled: false,
      stroked: false,
      getLineColor: ({ properties: p }: MapFeature) => (p.class === "runway" ? RUNWAY : NONE),
      getLineWidth: ({ properties: p }: MapFeature) => (p.class === "runway" ? RUNWAY_WIDTH_M : 0),
      lineWidthUnits: "meters",
      getPointRadius: 0,
      parameters: SURFACE_NO_CULL,
    });

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
  const vectorMapLayer = (url: string, flat: boolean, trueAeroways: boolean) =>
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
      // the aeroway layer draws runways once they are at true width
      getLineWidth: trueAeroways
        ? (f: MapFeature) =>
            f.properties.layerName === "aeroway" && f.properties.class === "runway" ? 0 : vectorLineWidth(f)
        : vectorLineWidth,
      updateTriggers: { getLineWidth: trueAeroways },
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
    if (p.layerName === "aerodrome_label") return [55, 142, 240, 255];
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
  const textLayer = (data: LabelFeature[], sizeScale: number) =>
    new TextLayer<LabelFeature, CollisionFilterExtensionProps<LabelFeature>>({
      id: "labels",
      data,
      getPosition: labelPosition,
      getText: labelText,
      getSize: labelSize,
      sizeScale,
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

  // Aircraft labels: each aircraft's callsign beside it, and close in its
  // height under that. The text is laid out once per set of labels (a new
  // feed, aircraft coming into or leaving the view, the height line coming or
  // going); each frame only moves them to where their aircraft are drawn.
  /** Too far out even a thinned set of labels is a carpet of text: only the selected aircraft's. */
  const AIRCRAFT_LABEL_MIN_ZOOM = 4;
  /** From here a second, dimmer line gives the height. */
  const AIRCRAFT_LABEL_HEIGHT_ZOOM = 7;
  const AIRCRAFT_LABEL_SIZE = 11;
  const AIRCRAFT_LABEL_HEIGHT_SIZE = 10;
  const AIRCRAFT_LABEL_COLOR: RGBA = [241, 238, 231, 235];
  const AIRCRAFT_LABEL_SELECTED: RGBA = [...SELECTED_COLOR, 255];
  const AIRCRAFT_LABEL_HEIGHT_COLOR: RGBA = [162, 172, 183, 220];
  const AIRCRAFT_LABEL_HEIGHT_SELECTED: RGBA = [...SELECTED_COLOR, 205];
  /** Between the aircraft as drawn and its label, pixels. */
  const AIRCRAFT_LABEL_GAP_PX = 5;
  /** Half the distance between the two lines' middles, pixels at the default size. */
  const AIRCRAFT_LABEL_LINE_PX = 6;
  /**
   * The collision filter only asks whether something more important covers
   * the point an aircraft is drawn at, not its label. So each label claims an
   * invisible box (its background) reaching this far left of its aircraft too,
   * about one callsign wide, and its own height above and below: an aircraft
   * whose label would run into this one's then loses its label.
   */
  const AIRCRAFT_LABEL_RESERVE_PX = 60;
  const NO_BACKGROUND: RGBA = [0, 0, 0, 0];
  const AIRCRAFT_LABEL_GROUP = "aircraft-labels";
  /**
   * The collision filter fades a label by how much of a small patch around
   * its aircraft it holds, which leaves half-covered labels as faint ghosts
   * in busy airspace. These snap to shown or hidden over a short range, so
   * they still ease in and out as aircraft move but rarely linger half-seen.
   */
  const crispCollision = (() => {
    // the stock shader module (its `getShaders` reads nothing off the layer)
    const base = CollisionFilterExtension.prototype.getShaders.call(null as never).modules[0] as {
      inject: Record<string, string>;
    };
    return {
      ...base,
      inject: { ...base.inject, "vs:DECKGL_FILTER_COLOR": "color.a *= smoothstep(0.3, 0.65, collision_fade);" },
    };
  })();
  class CrispCollisionFilterExtension extends CollisionFilterExtension {
    getShaders() {
      return { modules: [crispCollision] };
    }
  }
  const AIRCRAFT_LABEL_EXTENSIONS = [new CrispCollisionFilterExtension()];

  /** What each aircraft is called on the map, worked out once per report. */
  const labelTexts = new WeakMap<LiveFlight, string>();
  function labelTextOf(f: LiveFlight): string {
    let text = labelTexts.get(f);
    if (text === undefined) labelTexts.set(f, (text = aircraftLabel(f)));
    return text;
  }

  type LabelSet = {
    /** The feed the labels were made from. */
    feed: LiveFlight[] | null;
    withHeight: boolean;
    items: LiveFlight[];
    index: Map<number, number>;
    texts: string[];
    heights: string[];
    priorities: number[];
    /** Each aircraft's size, metres, for the gap to its label. */
    sizes: number[];
    largest: number;
    /** Bumped whenever the set is laid out again. */
    version: number;
  };
  const emptyLabelSet = (version: number): LabelSet => ({
    feed: null,
    withHeight: false,
    items: [],
    index: new Map(),
    texts: [],
    heights: [],
    priorities: [],
    sizes: [],
    largest: 0,
    version,
  });
  let labelSet = emptyLabelSet(0);
  /** Where each label's aircraft is drawn this frame, by the set's index. */
  let labelPositions: [number, number, number][] = [];

  /**
   * Keeps the labelled set while the same aircraft are labelled, and points
   * each label at its aircraft's drawn position for this frame.
   */
  function updateLabelSet(drawn: LiveFlight[], positions: [number, number, number][], sizes: number[], withHeight: boolean) {
    let same = labelSet.feed === flights && labelSet.withHeight === withHeight && labelSet.items.length === drawn.length;
    for (let i = 0; same && i < drawn.length; i++) same = labelSet.index.has(drawn[i].id);
    if (same) {
      const placed = new Array<[number, number, number]>(drawn.length);
      for (let i = 0; i < drawn.length; i++) placed[labelSet.index.get(drawn[i].id)!] = positions[i];
      labelPositions = placed;
      return;
    }
    const set = emptyLabelSet(labelSet.version + 1);
    set.feed = flights;
    set.withHeight = withHeight;
    set.items = drawn;
    set.sizes = sizes;
    drawn.forEach((f, i) => {
      set.index.set(f.id, i);
      set.texts.push(labelTextOf(f));
      set.heights.push(withHeight ? altitudeLabel(f.alt, f.onGround) : "");
      set.priorities.push(aircraftLabelPriority(sizes[i], f.alt, f.onGround));
      set.largest = Math.max(set.largest, sizes[i]);
    });
    labelSet = set;
    labelPositions = positions;
  }

  /** The collision pass's own props, kept while the size setting is. */
  let labelTestProps = { scale: 0, props: {} as { sizeScale: number } };
  const pixelOffsetOut: [number, number] = [0, 0];

  /** `sizing` changes whenever the aircraft's size on screen does, which moves the labels out with them. */
  function aircraftLabelLayers(
    screenPx: (meters: number) => number,
    sizing: number[],
    selectedId: number | undefined,
    scale: number,
    zoom: number,
  ): Layer[] {
    const set = labelSet;
    if (!set.items.length) return [];
    const positions = labelPositions;
    if (labelTestProps.scale !== scale) labelTestProps = { scale, props: { sizeScale: scale } };
    const line = set.withHeight ? AIRCRAFT_LABEL_LINE_PX * scale : 0;
    const gapFor = (meters: number) => screenPx(meters) / 2 + AIRCRAFT_LABEL_GAP_PX;
    const gap = (i: number) => gapFor(set.sizes[i]);
    // the claimed box: from left of the largest aircraft's gap, as tall again as the label above and below
    const half = AIRCRAFT_LABEL_SIZE * scale * 0.5;
    // zoomed out, where traffic is densest, labels keep further apart
    const spread = 1 + 0.6 * clamp01((AIRCRAFT_LABEL_HEIGHT_ZOOM - zoom) / 3);
    const padY = (half + 2 * line + 4 * scale) * spread;
    const reserve = AIRCRAFT_LABEL_RESERVE_PX * scale * spread;
    const padding: [number, number, number, number] = [gapFor(set.largest) + reserve, padY, 4 + reserve - AIRCRAFT_LABEL_RESERVE_PX * scale, padY];
    const priority = (f: LiveFlight, { index }: { index: number }) =>
      f.id === selectedId ? SELECTED_LABEL_PRIORITY : set.priorities[index];
    const shared = {
      data: set.items,
      getPosition: (_: LiveFlight, { index }: { index: number }) => positions[index],
      fontFamily: LABEL_FONT,
      fontWeight: 600,
      characterSet: "auto" as const,
      fontSettings: LABEL_FONT_SETTINGS,
      outlineWidth: 3,
      outlineColor: LABEL_OUTLINE,
      sizeUnits: "pixels" as const,
      sizeScale: scale,
      billboard: true,
      getTextAnchor: "start" as const,
      getAlignmentBaseline: "center" as const,
      parameters: LABEL_PARAMETERS,
      extensions: AIRCRAFT_LABEL_EXTENSIONS,
      collisionGroup: AIRCRAFT_LABEL_GROUP,
      getCollisionPriority: priority,
      collisionTestProps: labelTestProps.props,
    };
    const offset = (dy: number) => (_: LiveFlight, { index }: { index: number }) => {
      pixelOffsetOut[0] = gap(index);
      pixelOffsetOut[1] = dy;
      return pixelOffsetOut;
    };
    // Both lines share the data, so the same aircraft has the same picking
    // colour in each, which is what the collision filter compares: the two
    // lines show and hide together.
    const layers: Layer[] = [
      new TextLayer<LiveFlight, CollisionFilterExtensionProps<LiveFlight>>({
        ...shared,
        id: "aircraft-label-names",
        getText: (_, { index }) => set.texts[index],
        getSize: AIRCRAFT_LABEL_SIZE,
        getColor: (f) => (f.id === selectedId ? AIRCRAFT_LABEL_SELECTED : AIRCRAFT_LABEL_COLOR),
        getPixelOffset: offset(-line),
        background: true,
        getBackgroundColor: NO_BACKGROUND,
        backgroundPadding: padding,
        updateTriggers: {
          getPosition: tick,
          getText: set.version,
          getColor: selectedId,
          getPixelOffset: [...sizing, line],
          getCollisionPriority: selectedId,
        },
      }),
    ];
    if (set.withHeight) {
      layers.push(
        new TextLayer<LiveFlight, CollisionFilterExtensionProps<LiveFlight>>({
          ...shared,
          id: "aircraft-label-heights",
          getText: (_, { index }) => set.heights[index],
          getSize: AIRCRAFT_LABEL_HEIGHT_SIZE,
          getColor: (f) => (f.id === selectedId ? AIRCRAFT_LABEL_HEIGHT_SELECTED : AIRCRAFT_LABEL_HEIGHT_COLOR),
          getPixelOffset: offset(line),
          updateTriggers: {
            getPosition: tick,
            getText: set.version,
            getColor: selectedId,
            getPixelOffset: [...sizing, line],
            getCollisionPriority: selectedId,
          },
        }),
      );
    }
    return layers;
  }

  const BUILDING_LOAD_OPTIONS = { mvt: { layers: ["building"], workerUrl: mvtWorkerUrl } };
  const BUILDING_MATERIAL = {
    ambient: 0.42,
    diffuse: 0.6,
    shininess: 16,
    specularColor: [60, 66, 80] as [number, number, number],
  };
  const BUILDING_COLOR: [number, number, number] = [58, 68, 84];
  const BUILDING_SUBLAYER_PROPS = { "polygons-fill": { _windingOrder: "CCW" } };
  const buildingElevation = (f: { properties: Record<string, unknown> }) => buildingHeight(f.properties);
  /**
   * The sun the building shadows were last cast with. It moves about 0.25°
   * a minute; the shadows are recast once it has moved this far (about every
   * minute), never per frame.
   */
  const SHADOW_SUN_STEP = Math.cos((0.25 * Math.PI) / 180);
  let buildingSun: [number, number, number] | null = null;
  function shadowSun(now: number) {
    const sun = sunDirection(now);
    if (!buildingSun || sun[0] * buildingSun[0] + sun[1] * buildingSun[1] + sun[2] * buildingSun[2] < SHADOW_SUN_STEP) {
      buildingSun = sun;
    }
    return buildingSun;
  }
  const buildingLayer = (url: string, sun: readonly number[]) =>
    new BuildingLayer({
      id: "buildings",
      data: url,
      minZoom: 13,
      maxZoom: 14,
      loadOptions: BUILDING_LOAD_OPTIONS,
      extruded: true,
      filled: true,
      stroked: false,
      getElevation: buildingElevation,
      getFillColor: BUILDING_COLOR,
      material: BUILDING_MATERIAL,
      parameters: NO_CULL,
      // Vector tiles wind outlines clockwise with y pointing down, which in
      // the tile's own numbers is counter-clockwise: said so, the walls get
      // outward normals and are lit from the side the light is on.
      _subLayerProps: BUILDING_SUBLAYER_PROPS,
      // their shadows, on the ground under them and under the aircraft's
      sun,
      shadowColor: SHADOW_COLOR,
      shadowParameters: SHADOW_PARAMETERS,
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
    own: {
      id: string;
      data: PolygonData | ArenaData;
      /** For an arena's data (radar-arena.ts): how much of it to draw, and its version. */
      arena?: { indexCount: number; version: number };
      opacity: number;
      parameters: object;
      z?: number;
      bottom?: number;
    },
  ) =>
    new (own.arena ? ArenaLayer : SolidPolygonLayer)(props ?? {}, {
      ...own.arena,
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
   * The lifted layers' buffers, a slices and a walls arena per height
   * (radar-arena.ts): updated in place as tiles come and go, so a pan uploads
   * only the tiles it brings into view. Freed while the view is flat.
   */
  let arenas: { slices: MeshArena; walls: MeshArena }[] | null = null;
  let arenasVersion = -1;
  /** deck.gl's GPU device, for the arenas' buffers; set once it has one. */
  let device: Device | null = null;
  /** The first thing that stopped the map drawing, shown with a way to report it. */
  let mapError = $state<{ message: string; stack: string } | null>(null);
  function reportMapError(e: unknown) {
    if (mapError) return;
    const err = e instanceof Error ? e : new Error(String(e));
    console.error("map error", err);
    mapError = { message: err.message || String(e), stack: err.stack ?? "" };
  }
  function mapErrorDetails(): string {
    const info = device?.info as Record<string, unknown> | undefined;
    return [
      `RustRadar map error: ${mapError?.message}`,
      `Renderer: ${info ? [info.vendor, info.renderer, info.version, info.shadingLanguageVersion].filter(Boolean).join(" | ") : "no WebGL device"}`,
      `Browser: ${navigator.userAgent}`,
      "",
      mapError?.stack ?? "",
    ].join("\n");
  }
  function retryMap() {
    mapError = null;
    render();
  }
  function liftedArenas(gpu: Device) {
    arenas ??= RADAR_LEVELS_KM.map(() => ({ slices: new MeshArena(gpu, 2), walls: new MeshArena(gpu, 3) }));
    if (arenasVersion !== radarVersion) {
      arenasVersion = radarVersion;
      arenas.forEach(({ slices, walls }, i) => {
        const slicePieces = new Map<RadarTile, ArenaPiece>();
        const wallPieces = new Map<RadarTile, ArenaPiece>();
        for (const tile of radarTiles) {
          // each tile's levels run up from the lowest without gaps, so index i is this height
          const level = tile.levels[i];
          if (!level) continue;
          slicePieces.set(tile, { mesh: tile.slices, fromBand: level.fromBand });
          if (tile.walls) wallPieces.set(tile, { mesh: tile.walls, fromBand: level.fromBand });
        }
        slices.update(slicePieces);
        walls.update(wallPieces);
      });
    }
    return arenas;
  }
  let lifted: { key: string; layers: Layer[] } = { key: "", layers: [] };
  function freeArenas() {
    if (!arenas) return;
    for (const { slices, walls } of arenas) {
      slices.destroy();
      walls.destroy();
    }
    arenas = null;
    arenasVersion = -1;
    lifted = { key: "", layers: [] };
  }
  /** Built again only when the tiles, the height scale or the tilt change. */
  function radarLifted(ex: number, lift: number): Layer[] {
    const scale = Math.round(ex * 10) / 10;
    if (!lift || !device) {
      freeArenas();
      return [];
    }
    const key = `${scale}/${lift}/${radarVersion}`;
    if (lifted.key !== key) {
      const layers: Layer[] = [];
      // higher layers keep only the stronger echoes, so cells rise like towers
      liftedArenas(device).forEach(({ slices, walls }, i) => {
        if (!slices.data) return;
        const km = RADAR_LEVELS_KM[i];
        const opacity = Math.max(0.16, 0.34 - i * 0.03) * lift;
        const z = elevation(km * FEET_PER_KM, scale);
        const { data, indexCount, version } = slices;
        const arena = { indexCount, version };
        layers.push(radarPolygons(null, { id: `radar-lift-${km}`, data, arena, opacity, parameters: RADAR_LIFTED, z }));
        // walled down to the layer below, the lowest to the ground, so the stack has no gaps
        const bottom = i ? elevation(RADAR_LEVELS_KM[i - 1] * FEET_PER_KM, scale) : 0;
        if (walls.data) {
          const { data, indexCount, version } = walls;
          const arena = { indexCount, version };
          layers.push(radarPolygons(null, { id: `radar-wall-${km}`, data, arena, opacity, parameters: RADAR_LIFTED, z, bottom }));
        }
      });
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

  // dashes measured along the whole path, not per segment: the estimated
  // routes are sampled every few kilometres, and per-segment dashes would
  // squeeze into a solid line wherever the segments are short on screen
  const dashed = new PathStyleExtension({ dash: true, highPrecisionDash: true });
  const DASHED = [dashed];
  const ROUTE_AHEAD: [number, number, number, number] = [55, 142, 240, 210];
  const ROUTE_BEFORE: [number, number, number, number] = [241, 238, 231, 110];
  const ROUTE_FILED: [number, number, number, number] = [241, 238, 231, 150];

  /** Route heights (ft MSL) as drawn: airport-relative like the aircraft and its trail, then exaggerated. */
  function drawnRoute(points: readonly PathPoint[], id: number, ex: number): [number, number, number][] {
    return points.map(([lon, lat, alt]) => [lon, lat, elevation(motion.drawAltitude(id, alt, lat, lon), ex)]);
  }

  /** Over this distance the estimate ahead eases from the aircraft's drawn height into its own profile. */
  const AHEAD_BLEND_M = 15_000;

  /**
   * The estimated path ahead, from the aircraft as drawn now: the route is
   * worked out at each report, and the aircraft moves on along it between them.
   */
  function drawnAhead(ahead: readonly PathPoint[], pos: [number, number, number], id: number, ex: number) {
    const path = drawnRoute(trimToPosition(ahead, [pos[0], pos[1]]), id, ex);
    const dz = pos[2] - path[0][2];
    let walked = 0;
    for (let i = 1; i < path.length && walked < AHEAD_BLEND_M; i++) {
      walked += distanceKm([path[i - 1][0], path[i - 1][1]], [path[i][0], path[i][1]]) * 1000;
      path[i][2] = Math.max(0, path[i][2] + dz * Math.max(0, 1 - walked / AHEAD_BLEND_M));
    }
    path[0] = pos;
    return path;
  }

  function routeLayers(r: Route, pos: [number, number, number], id: number, ex: number): Layer[] {
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
          // ends at the trail's first point, at the same height
          data: [{ path: drawnRoute(r.before, id, ex) }],
          getPath: (d) => d.path,
          getColor: ROUTE_BEFORE,
          getWidth: 1.5,
          widthUnits: "pixels",
          getDashArray: [3, 3],
          extensions: DASHED,
          parameters: NO_CULL,
        }),
      );
    }
    if (r.ahead.length > 1) {
      layers.push(
        new PathLayer<{ path: [number, number, number][] }, PathStyleExtensionProps>({
          id: "route-ahead",
          data: [{ path: drawnAhead(r.ahead, pos, id, ex) }],
          getPath: (d) => d.path,
          getColor: ROUTE_AHEAD,
          getWidth: 2,
          widthUnits: "pixels",
          getDashArray: [4, 3],
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

  const MARKER_FILL: [number, number, number, number] = [55, 142, 240, 255];
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

  /** A searched place: quieter than an airport's amber, which means selection. */
  function placeMarker(p: { lon: number; lat: number; name: string }): Layer[] {
    const data = [p];
    const position = (d: typeof p): LonLat => [d.lon, d.lat];
    return [
      new ScatterplotLayer<typeof p>({
        id: "place-dot",
        data,
        getPosition: position,
        getRadius: 4,
        radiusUnits: "pixels",
        getFillColor: [241, 238, 231, 255],
        stroked: true,
        getLineColor: MARKER_LINE,
        getLineWidth: 2,
        lineWidthUnits: "pixels",
        billboard: true,
        parameters: { ...NO_CULL, depthCompare: "always" },
      }),
      new TextLayer<typeof p>({
        id: "place-name",
        data,
        getPosition: position,
        getText: (d) => d.name,
        getSize: 12,
        getColor: [241, 238, 231, 230],
        getPixelOffset: [0, -14],
        fontFamily: LABEL_FONT,
        fontWeight: 600,
        fontSettings: LABEL_FONT_SETTINGS,
        outlineWidth: 3,
        outlineColor: LABEL_OUTLINE,
        billboard: true,
        characterSet: "auto",
        parameters: LABEL_PARAMETERS,
      }),
    ];
  }

  const mix = (a: [number, number, number], b: [number, number, number], t: number): [number, number, number] => [
    a[0] + (b[0] - a[0]) * t,
    a[1] + (b[1] - a[1]) * t,
    a[2] + (b[2] - a[2]) * t,
  ];

  const at = (pose: Pose, ex: number): [number, number, number] => [pose.lon, pose.lat, elevation(pose.altFt, ex)];

  /**
   * deck.gl's `[pitch, yaw, roll]` turns the mesh about its own y, z and x axes.
   * The models put the nose on +y and the right wing on +x, so deck's "pitch"
   * is the aircraft's bank and its "roll" the aircraft's pitch. Both act in the
   * model's own frame, before the yaw, so their signs hold on the globe too.
   */
  const orientationOf = (pose: Pose, yawOffset: number): [number, number, number] => [
    pose.bank,
    yawOffset - pose.heading,
    pose.pitch,
  ];

  /**
   * Where the mesh's up axis points after `orientationOf`: the third column of
   * deck.gl's rotation (`@deck.gl/mesh-layers` utils/matrix.ts). Retracting the
   * gear scales it about its attach plane, whose offset has to follow the tilt.
   */
  function upAxis([p, y, r]: [number, number, number]): [number, number, number] {
    const [sp, cp, sy, cy, sr, cr] = [p, p, y, y, r, r].map((a, i) => (i % 2 ? Math.cos : Math.sin)(a * DEGREES));
    return [sy * sr + cy * sp * cr, -cy * sr + sy * sp * cr, cp * cr];
  }

  // Ships: icons on the water, and close in 3D models at true scale. Their
  // data arrays are built once per ship list; only the icons' screen angles
  // follow the camera, and which ships are big enough for a model is redone
  // in quarter-zoom steps.
  /**
   * From this zoom ships under way (heading known) are drawn as 3D models;
   * it is above FLAT_ABOVE_ZOOM, so always in the flat projection.
   */
  const SHIP_MESH_ZOOM = 12.5;
  /** ...each once it would be this many pixels long; smaller ones stay icons until then. */
  const SHIP_MESH_MIN_PX = 14;
  const SHIP_ALPHA = 235;
  /** Old positions are drawn faded. */
  const SHIP_OLD_ALPHA = 105;
  const SHIP_MESH_COLOR: ShipColor = [255, 255, 255, 255];
  const SHIP_MESH_OLD_COLOR: ShipColor = [255, 255, 255, 120];
  const SHIP_PARAMETERS = { ...NO_CULL, depthWriteEnabled: false } as const;
  /**
   * The aircraft's lights (sun, camera and ambient) on darker, coloured
   * paint: less ambient than the white airframes need, so decks seen from
   * straight above keep their colours instead of washing out.
   */
  const SHIP_MESH_MATERIAL = { ambient: 0.5, diffuse: 0.48, shininess: 24, specularColor: [40, 40, 40] as [number, number, number] };
  type ShipColor = [number, number, number, number];
  /** One ship drawn as a model: where, how big and which way, worked out once. */
  type ShipModel = {
    index: number;
    position: [number, number, number];
    scale: [number, number, number];
    heading: number;
    /** The hull's centre from the reported position, metres east and north. */
    east: number;
    north: number;
    /** The zoom from which it is long enough on screen to be a model. */
    minZoom: number;
  };
  /** How long a ship or train takes to ease onto a new report. */
  const EASE_MS = 2000;
  /**
   * An offset to ease away; or, for a vehicle that would go back to get onto
   * its new frames, the place it holds at (heading `heading`) until they pass it.
   */
  type Ease = { dLat: number; dLon: number; from: number; hold?: { lat: number; lon: number; heading: number } };
  /** How far `p` is ahead of `from` going `heading` (degrees), metres. */
  function aheadOf(p: { lat: number; lon: number }, from: { lat: number; lon: number }, heading: number): number {
    const k = Math.cos((from.lat * Math.PI) / 180);
    const east = (((p.lon - from.lon + 540) % 360) - 180) * k * 111_320;
    const north = (p.lat - from.lat) * 111_320;
    const h = (heading * Math.PI) / 180;
    return east * Math.sin(h) + north * Math.cos(h);
  }
  /**
   * The offset from where a vehicle is now expected to where it was drawn
   * (index `was` in the old positions), to ease away; none for a new one, one
   * that hasn't moved, or one that jumped further than it could have gone.
   * Never back: drawn ahead of where its new frames have it, it holds there.
   */
  function easeFrom(
    was: number | undefined,
    positions: [number, number, number][] | undefined,
    to: { lat: number; lon: number; heading: number | null; frames: [number, number, number][] },
    nowMs: number,
  ): Ease | null {
    if (was === undefined || !positions) return null;
    const [lon, lat] = positions[was];
    const target = positionAt(to, nowMs);
    const dLat = lat - target.lat;
    const dLon = ((lon - target.lon + 540) % 360) - 180;
    if (!(dLat || dLon) || Math.abs(dLat) >= 0.05 || Math.abs(dLon) >= 0.1) return null;
    const heading = headingAt(to, nowMs);
    if (heading !== null && aheadOf({ lat, lon }, target, heading) > 0.05) {
      return { dLat: 0, dLon: 0, from: nowMs, hold: { lat, lon, heading } };
    }
    return { dLat, dLon, from: nowMs };
  }
  /** Where a vehicle is drawn at `nowMs`: along its frames, plus what is left of its easing (smoothstep). */
  function eased(
    v: { lat: number; lon: number; frames: [number, number, number][] },
    blends: Map<number, Ease>,
    i: number,
    nowMs: number,
  ): { lat: number; lon: number } {
    let { lat, lon } = positionAt(v, nowMs);
    const blend = blends.get(i);
    if (blend?.hold) {
      // there until its frames come past
      if (aheadOf({ lat, lon }, blend.hold, blend.hold.heading) < 0) return { lat: blend.hold.lat, lon: blend.hold.lon };
      blends.delete(i);
    } else if (blend) {
      const left = 1 - Math.min(1, (nowMs - blend.from) / EASE_MS);
      const k = left * left * (3 - 2 * left);
      lat += blend.dLat * k;
      lon += blend.dLon * k;
      if (left === 0) blends.delete(i);
    }
    return { lat, lon };
  }
  type ShipData = {
    list: Ship[];
    positions: [number, number, number][];
    icons: string[];
    colors: ShipColor[];
    /** Indices of all ships. */
    all: number[];
    /** Ships that can be drawn as a model, by class. */
    models: Map<ShipClass, ShipModel[]>;
    /** Close in: the models shown and the ships left as icons, for one quarter-zoom step. */
    split: { step: number; models: Map<ShipClass, ShipModel[]>; icons: number[] } | null;
    byMmsi: Map<number, number>;
    /** Ships under way (or easing onto a new report), stepped on every frame. */
    moving: number[];
    blends: Map<number, { dLat: number; dLon: number; from: number }>;
  };
  let shipData: ShipData | null = null;
  /** Bumped whenever the ships' drawn positions change. */
  let shipMoves = 0;
  let shipAtlasData: ReturnType<typeof shipAtlas> | undefined;
  function shipsFor(list: Ship[], nowMs: number): ShipData {
    if (shipData?.list === list) return shipData;
    const old = shipData;
    const data: ShipData = {
      list,
      positions: [],
      icons: [],
      colors: [],
      all: [],
      models: new Map(),
      split: null,
      byMmsi: new Map(),
      moving: [],
      blends: new Map(),
    };
    list.forEach((ship, i) => {
      const color: ShipColor = [...shipColor(ship.kind), ship.old ? SHIP_OLD_ALPHA : SHIP_ALPHA];
      // a new report moves the ship on from where it is drawn, not with a jump
      const blend = easeFrom(old?.byMmsi.get(ship.mmsi), old?.positions, ship, nowMs);
      if (blend) data.blends.set(i, blend);
      data.byMmsi.set(ship.mmsi, i);
      if ((ship.frames?.length ?? 0) > 1 || blend) data.moving.push(i);
      const position: [number, number, number] = [ship.lon, ship.lat, 0];
      data.positions.push(position);
      data.icons.push(shipIcon(ship));
      data.colors.push(color);
      data.all.push(i);
      // a moored ship reports no heading: no telling which way to draw it
      if (ship.heading === null) return;
      const place = shipPlacement(ship);
      const [east, north] = offsetEastNorth(place.right, place.ahead, ship.heading);
      let group = data.models.get(place.cls);
      if (!group) data.models.set(place.cls, (group = []));
      group.push({
        index: i,
        position,
        scale: place.scale,
        heading: ship.heading,
        east,
        north,
        minZoom: Math.log2((metersPerPixel(0, ship.lat) * SHIP_MESH_MIN_PX) / place.length),
      });
    });
    shipData = data;
    return data;
  }
  /** Steps the moving ships on to `nowMs`, in place: the icons and the models share the positions. */
  function moveShips(data: ShipData, nowMs: number) {
    if (!data.moving.length) return;
    for (const i of data.moving) {
      const { lat, lon } = eased(data.list[i], data.blends, i, nowMs);
      data.positions[i][0] = lon;
      data.positions[i][1] = lat;
    }
    shipMoves++;
  }
  /** Which ships are models and which icons at `zoom`, in quarter steps so the split is rarely redone. */
  function shipSplit(data: ShipData, zoom: number) {
    const step = Math.floor(zoom * 4) / 4;
    if (data.split?.step === step) return data.split;
    const models = new Map<ShipClass, ShipModel[]>();
    const shown = new Uint8Array(data.list.length);
    for (const [cls, group] of data.models) {
      const big = group.filter((d) => step >= d.minZoom);
      big.forEach((d) => (shown[d.index] = 1));
      if (big.length) models.set(cls, big);
    }
    const icons = data.all.filter((i) => !shown[i]);
    return (data.split = { step, models, icons });
  }
  /**
   * The models, one instanced layer per class. Each is turned like the
   * aircraft (`orientationOf`) and offset so its AIS reference point lands on
   * the reported position; the globe's local frame is the flat one turned
   * half a turn, so there the offset is too.
   */
  function shipMeshLayers(split: NonNullable<ShipData["split"]>, flat: boolean): Layer[] {
    const meshes = shipMeshes();
    const hoveredShip = hoveredShipIndex();
    const yawOffset = flat ? 0 : 180;
    const sign = flat ? 1 : -1;
    const list = shipData?.list ?? [];
    const layers: Layer[] = [];
    for (const [cls, group] of split.models) {
      layers.push(
        new SimpleMeshLayer<ShipModel>({
          id: `ship-mesh-${cls}`,
          data: group,
          mesh: meshes[cls].geometry,
          getPosition: (d) => d.position,
          getOrientation: (d) => [0, yawOffset - d.heading, 0],
          getScale: (d) => d.scale,
          getTranslation: (d) => [sign * d.east, sign * d.north, 0],
          // the colours are the mesh's own; old positions fade
          getColor: (d) => (list[d.index]?.old ? SHIP_MESH_OLD_COLOR : SHIP_MESH_COLOR),
          material: SHIP_MESH_MATERIAL,
          pickable: true,
          autoHighlight: true,
          highlightColor: [55, 142, 240, 110],
          // a ship hovered in the sidebar's list lights up as if under the pointer
          ...(hoveredShip !== null ? { highlightedObjectIndex: group.findIndex((d) => d.index === hoveredShip) } : {}),
          parameters: NO_CULL,
          updateTriggers: { getPosition: shipMoves, getOrientation: yawOffset, getTranslation: sign },
        }),
      );
    }
    return layers;
  }
  /** The icons' angles on screen, kept while neither the camera nor the ships move. */
  let shipAngles: { key: string; list: Ship[] | null; angles: Float32Array; version: number } = {
    key: "",
    list: null,
    angles: new Float32Array(0),
    version: 0,
  };
  /** The hovered ship's index in the ship list, or null. */
  function hoveredShipIndex(): number | null {
    if (hoveredShipMmsi === null || !shipData) return null;
    const i = shipData.list.findIndex((s) => s.mmsi === hoveredShipMmsi);
    return i === -1 ? null : i;
  }
  function shipLayers(list: Ship[], flat: boolean, mpp: number, nowMs: number): Layer[] {
    const { zoom, bearing, pitch, longitude, latitude } = view;
    const data = shipsFor(list, nowMs);
    if (!list.length) return [];
    moveShips(data, nowMs);
    shipAtlasData ??= shipAtlas();
    const split = zoom >= SHIP_MESH_ZOOM ? shipSplit(data, zoom) : null;
    const layers: Layer[] = split ? shipMeshLayers(split, flat) : [];
    if (!shipAtlasData) return layers;
    const key = `${zoom},${bearing},${pitch},${longitude},${latitude}`;
    if (shipAngles.key !== key || shipAngles.list !== list) {
      const width = container.clientWidth;
      const height = container.clientHeight;
      // the flat projection's viewport above zoom 12, as deck.gl draws it
      const viewport = flat
        ? new WebMercatorViewport({ ...view, width, height })
        : new GlobeViewport({ ...view, width, height });
      const step = ICON_HEADING_STEP_PX * mpp;
      const angles = new Float32Array(list.length);
      const minCos = Math.cos((visibleRadius(zoom, pitch, width, height) * Math.PI) / 180);
      list.forEach((ship, i) => {
        if (ship.heading === null || cosAngle(ship.lon, ship.lat, longitude, latitude) <= minCos) return;
        angles[i] = screenAngle(viewport, data.positions[i], ship.heading, step) ?? bearing - ship.heading;
      });
      shipAngles = { key, list, angles, version: shipAngles.version + 1 };
    }
    const angles = shipAngles.angles;
    const iconData = split ? split.icons : data.all;
    const hovered = hoveredShipIndex();
    layers.push(
      new IconLayer<number>({
        id: "ship-icons",
        data: iconData,
        highlightedObjectIndex: hovered === null ? -1 : split ? iconData.indexOf(hovered) : hovered,
        highlightColor: [55, 142, 240, 200],
        getPosition: (i) => data.positions[i],
        getIcon: (i) => data.icons[i],
        iconAtlas: shipAtlasData.atlas as never,
        iconMapping: shipAtlasData.mapping as never,
        getSize: (i) => shipIconSize(list[i], zoom),
        getAngle: (i) => angles[i],
        getColor: (i) => data.colors[i],
        sizeUnits: "pixels",
        billboard: true,
        pickable: true,
        parameters: SHIP_PARAMETERS,
        updateTriggers: { getPosition: shipMoves, getAngle: shipAngles.version, getSize: shipIconScale(zoom) },
      }),
    );
    return layers;
  }

  // Trains: icons on land, drawn like the ships' icons. Their data arrays are
  // built once per train list; on each render only the trains with frames
  // (expected positions ahead) move along them, and the icons' screen angles
  // follow the camera and those moves.
  const TRAIN_ALPHA = 240;
  const TRAIN_PARAMETERS = { ...NO_CULL, depthWriteEnabled: false } as const;
  type TrainData = {
    list: Train[];
    positions: [number, number, number][];
    /** Direction of travel as drawn, degrees; null when unknown. */
    headings: (number | null)[];
    icons: string[];
    colors: ShipColor[];
    /** Indices of all trains. */
    all: number[];
    /** Indices of the trains with frames to move along. */
    moving: number[];
    byId: Map<string, number>;
    /** Trains easing from where they were drawn onto their new report. */
    blends: Map<number, Ease>;
  };
  let trainData: TrainData | null = null;
  let trainAtlasData: ReturnType<typeof trainAtlas> | undefined;
  /** Bumped whenever the drawn positions change: a new list, or the moving trains stepping on. */
  let trainMoves = 0;
  function trainsFor(list: Train[], nowMs: number): TrainData {
    if (trainData?.list === list) return trainData;
    const old = trainData;
    const data: TrainData = {
      list,
      positions: [],
      headings: [],
      icons: [],
      colors: [],
      all: [],
      moving: [],
      byId: new Map(),
      blends: new Map(),
    };
    list.forEach((train, i) => {
      // a new report moves the train on from where it is drawn, not with a jump
      const blend = easeFrom(old?.byId.get(train.id), old?.positions, train, nowMs);
      if (blend) data.blends.set(i, blend);
      data.positions.push([train.lon, train.lat, 0]);
      data.headings.push(train.heading);
      data.icons.push(trainIcon(train.kind, train.heading, train.operator));
      data.colors.push([...trainColorOf(train), TRAIN_ALPHA]);
      data.all.push(i);
      data.byId.set(train.id, i);
      if ((train.frames?.length ?? 0) > 1 || data.blends.has(i)) data.moving.push(i);
    });
    trainData = data;
    trainMoves++;
    return data;
  }
  /** Steps the trains with frames on to `nowMs`; the rest stay at their reported positions. */
  function moveTrains(data: TrainData, nowMs: number) {
    if (!data.moving.length) return;
    for (const i of data.moving) {
      const train = data.list[i];
      const { lat, lon } = eased(train, data.blends, i, nowMs);
      const heading = headingAt(train, nowMs);
      data.positions[i] = [lon, lat, 0];
      data.headings[i] = heading;
      data.icons[i] = trainIcon(train.kind, heading, train.operator);
    }
    trainMoves++;
  }
  /** The icons' angles on screen: all redone when the camera moves, only the moving trains' otherwise. */
  let trainAngles: { key: string; list: Train[] | null; angles: Float32Array; version: number } = {
    key: "",
    list: null,
    angles: new Float32Array(0),
    version: 0,
  };
  function trainLayers(list: Train[], flat: boolean, mpp: number, nowMs: number): Layer[] {
    const { zoom, bearing, pitch, longitude, latitude } = view;
    const data = trainsFor(list, nowMs);
    if (!list.length) return [];
    trainAtlasData ??= trainAtlas();
    if (!trainAtlasData) return [];
    moveTrains(data, nowMs);
    const key = `${zoom},${bearing},${pitch},${longitude},${latitude}`;
    const whole = trainAngles.key !== key || trainAngles.list !== list;
    if (whole || data.moving.length) {
      const width = container.clientWidth;
      const height = container.clientHeight;
      const viewport = flat
        ? new WebMercatorViewport({ ...view, width, height })
        : new GlobeViewport({ ...view, width, height });
      const step = ICON_HEADING_STEP_PX * mpp;
      const angles = whole ? new Float32Array(list.length) : trainAngles.angles;
      const minCos = Math.cos((visibleRadius(zoom, pitch, width, height) * Math.PI) / 180);
      for (const i of whole ? data.all : data.moving) {
        const heading = data.headings[i];
        const [lon, lat] = data.positions[i];
        if (heading === null || cosAngle(lon, lat, longitude, latitude) <= minCos) continue;
        angles[i] = screenAngle(viewport, data.positions[i], heading, step) ?? bearing - heading;
      }
      trainAngles = { key, list, angles, version: trainAngles.version + 1 };
    }
    const angles = trainAngles.angles;
    return [
      new IconLayer<number>({
        id: "train-icons",
        data: data.all,
        // a train hovered in the sidebar's list
        highlightedObjectIndex: hoveredTrainId !== null ? (data.byId.get(hoveredTrainId) ?? -1) : -1,
        highlightColor: [55, 142, 240, 200],
        getPosition: (i) => data.positions[i],
        getIcon: (i) => data.icons[i],
        iconAtlas: trainAtlasData.atlas as never,
        iconMapping: trainAtlasData.mapping as never,
        getSize: (i) => trainIconSize(list[i].kind, data.headings[i], zoom),
        getAngle: (i) => angles[i],
        getColor: (i) => data.colors[i],
        sizeUnits: "pixels",
        billboard: true,
        pickable: true,
        parameters: TRAIN_PARAMETERS,
        updateTriggers: {
          getPosition: trainMoves,
          getIcon: trainMoves,
          getAngle: trainAngles.version,
          getSize: [trainIconScale(zoom), trainMoves],
        },
      }),
    ];
  }

  /** The rail map is coloured by speed limit in the app (its tiles come through the backend). */
  const railSpeeds = isTauri();
  /** The zoom, for what the page shows over the map (the legend). */
  let shownZoom = $state(0);

  const TRAIN_ROUTE_DASH: [number, number, number, number] = [0, 0, 0, 255];
  const TRAIN_ROUTE_CASING: [number, number, number, number] = [236, 238, 244, 215];
  const TRAIN_ROUTE_DASHES: [number, number] = [2.2, 1.6];
  /** The way to where the selected train stops next. */
  const TRAIN_TO_STOP: [number, number, number, number] = [37, 99, 235, 255];
  /** Where along its route the selected train was last found (searching on from there). */
  let routeCursor: RouteCursor | null = null;

  /**
   * The selected train's way ahead as black dashes on a light casing, from
   * exactly where the train is drawn now along the track, and its stations.
   * The part behind the train is trimmed off as it goes.
   */
  function trainRouteLayers(r: TrainRoute, train: Train): Layer[] {
    const i = trainData?.byId.get(train.id);
    const drawn: LonLat = i !== undefined ? [trainData!.positions[i][0], trainData!.positions[i][1]] : [train.lon, train.lat];
    // where it is drawn going next, along the track
    const nowMs = Date.now();
    const frames = (i !== undefined ? trainData!.list[i] : train).frames ?? [];
    const ahead = frames.filter((f) => f[2] > nowMs).map((f): LonLat => [f[1], f[0]]);
    const way = routeAhead(r, drawn, routeCursor, ahead);
    routeCursor = way.cursor;
    if (!way.on) ontrainrouteoff?.();
    const head = way.head;
    const paths = way.rest;
    const ground = { cullMode: "none", depthWriteEnabled: false, depthCompare: "always" } as const;
    const stretch = (id: string, data: [number, number][][]) => [
      new PathLayer<[number, number][], PathStyleExtensionProps<[number, number][]>>({
        id: `${id}-dashes`,
        data,
        getPath: (p) => p,
        getColor: TRAIN_ROUTE_DASH,
        getWidth: 3,
        widthUnits: "pixels",
        extensions: [dashed],
        getDashArray: TRAIN_ROUTE_DASHES,
        dashJustified: false,
        parameters: ground,
      }),
    ];
    const stations = r.stops.filter((s) => s.stops);
    // where the drawn train is going to stop next: the way there, and the spot
    const toStop = wayToStop(drawn, frames, nowMs);
    return [
      ...stretch("train-route", paths),
      ...(head.length ? stretch("train-route-head", [head]) : []),
      new PathLayer<LonLat[]>({
        id: "train-route-to-stop",
        data: toStop.length >= 2 ? [toStop] : [],
        getPath: (p) => p,
        getColor: TRAIN_TO_STOP,
        getWidth: 4,
        widthUnits: "pixels",
        capRounded: true,
        jointRounded: true,
        parameters: ground,
      }),
      new ScatterplotLayer<LonLat>({
        id: "train-route-stop-spot",
        data: toStop.length >= 2 ? [toStop[toStop.length - 1]] : [],
        getPosition: (p) => p,
        getRadius: 5,
        radiusUnits: "pixels",
        stroked: true,
        getFillColor: [255, 255, 255, 255],
        getLineColor: TRAIN_TO_STOP,
        getLineWidth: 2.5,
        lineWidthUnits: "pixels",
        parameters: ground,
      }),
      new ScatterplotLayer<(typeof stations)[number]>({
        id: "train-route-stops",
        data: stations,
        getPosition: (s) => [s.lon, s.lat],
        getRadius: 4.5,
        radiusUnits: "pixels",
        stroked: true,
        getFillColor: TRAIN_ROUTE_CASING,
        getLineColor: TRAIN_ROUTE_DASH,
        getLineWidth: 2,
        lineWidthUnits: "pixels",
        parameters: ground,
      }),
    ];
  }

  /** Signal blocks show from this zoom (the signals are only mapped close in). */
  const BLOCKS_MIN_ZOOM = 13;
  /** A train drawn this near a block's track is in it, metres. */
  const IN_BLOCK_M = 12;
  const BLOCK_FREE: [number, number, number, number] = [60, 200, 110, 220];
  const BLOCK_TAKEN: [number, number, number, number] = [235, 64, 52, 240];
  /** The blocks fetched, for the area asked (padded round the view), and whether all of it was there. */
  let blockSet: { west: number; south: number; east: number; north: number; list: TrainBlock[]; complete: boolean } | null = null;
  let blocksAsking = false;
  let blocksAskedAt = 0;
  /** Which train is in each block (by block id), worked out now and then. */
  let blocksTaken: { at: number; by: Map<string, string>; version: number } = { at: 0, by: new Map(), version: 0 };

  /** The view's bounds, close in (the flat projection), `[west, south, east, north]`. */
  function viewBounds(): [number, number, number, number] | null {
    if (view.zoom <= FLAT_ABOVE_ZOOM) return null;
    const [width, height] = [container.clientWidth, container.clientHeight];
    const b = new WebMercatorViewport({ ...view, width, height }).getBounds();
    return [b[0], b[1], b[2], b[3]];
  }

  /** Asks for the blocks round the view when it has left those fetched, or they were still loading. */
  function refreshBlocks() {
    const b = viewBounds();
    if (!trainBlocks || !b || view.zoom < BLOCKS_MIN_ZOOM || blocksAsking) return;
    const [west, south, east, north] = b;
    const inside = blockSet && west >= blockSet.west && east <= blockSet.east && south >= blockSet.south && north <= blockSet.north;
    const now = Date.now();
    if (inside && (blockSet!.complete || now - blocksAskedAt < 2_500)) return;
    if (!inside && now - blocksAskedAt < 700) return;
    const [dx, dy] = [(east - west) * 0.15, (north - south) * 0.15];
    const area = { west: west - dx, south: south - dy, east: east + dx, north: north + dy };
    blocksAsking = true;
    blocksAskedAt = now;
    fetchTrainBlocks(area)
      .then((r) => {
        blockSet = { ...area, list: r.blocks, complete: r.complete };
        blocksTaken.at = 0;
      })
      .catch(() => {})
      .finally(() => (blocksAsking = false));
  }

  /** The nearest a point is to a line of `[lon, lat]` points, metres (flat). */
  function metresToLine(p: [number, number], line: [number, number][]): number {
    const k = Math.cos((p[1] * Math.PI) / 180);
    let best = Infinity;
    for (let i = 1; i < line.length; i++) {
      const [ax, ay] = [(line[i - 1][0] - p[0]) * k, line[i - 1][1] - p[1]];
      const [dx, dy] = [(line[i][0] - line[i - 1][0]) * k, line[i][1] - line[i - 1][1]];
      const len2 = dx * dx + dy * dy;
      const t = len2 > 0 ? Math.max(0, Math.min(1, -(ax * dx + ay * dy) / len2)) : 0;
      best = Math.min(best, Math.hypot(ax + dx * t, ay + dy * t) * 111_320);
    }
    return best;
  }

  /** Each block a train is drawn in: the nearest block's track to it, if near enough. */
  function takenBlocks(list: TrainBlock[]): Map<string, string> {
    const by = new Map<string, string>();
    const b = viewBounds();
    if (!trainData || !b) return by;
    const [west, south, east, north] = b;
    trainData.list.forEach((train, i) => {
      const p: [number, number] = [trainData!.positions[i][0], trainData!.positions[i][1]];
      if (p[0] < west || p[0] > east || p[1] < south || p[1] > north) return;
      let best: { id: string; d: number } | null = null;
      for (const block of list) {
        for (const line of block.lines) {
          const d = metresToLine(p, line);
          if (d <= IN_BLOCK_M && (!best || d < best.d)) best = { id: block.id, d };
        }
      }
      if (best) by.set(best.id, by.has(best.id) ? `${by.get(best.id)}, ${train.name}` : train.name);
    });
    return by;
  }

  /** The signal blocks round the view: green free, red taken, with the train taking each. */
  function blockLayers(): Layer[] {
    refreshBlocks();
    if (!trainBlocks || !blockSet || view.zoom < BLOCKS_MIN_ZOOM) return [];
    const list = blockSet.list;
    const now = Date.now();
    if (now - blocksTaken.at > 500) {
      blocksTaken = { at: now, by: takenBlocks(list), version: blocksTaken.version + 1 };
    }
    const taken = blocksTaken.by;
    const lines = list.flatMap((b) => b.lines.map((line) => ({ line, taken: taken.has(b.id) })));
    // each taken block named once, halfway along its first stretch
    const labels = list
      .filter((b) => taken.has(b.id))
      .map((b) => ({ text: taken.get(b.id)!, at: b.lines[0][Math.floor(b.lines[0].length / 2)] }));
    const ground = { cullMode: "none", depthWriteEnabled: false, depthCompare: "always" } as const;
    return [
      new PathLayer<(typeof lines)[number]>({
        id: "train-blocks",
        data: lines,
        getPath: (l) => l.line,
        getColor: (l) => (l.taken ? BLOCK_TAKEN : BLOCK_FREE),
        getWidth: (l) => (l.taken ? 4 : 2.5),
        widthUnits: "pixels",
        capRounded: true,
        parameters: ground,
        updateTriggers: { getColor: blocksTaken.version, getWidth: blocksTaken.version },
      }),
      new TextLayer<(typeof labels)[number]>({
        id: "train-block-labels",
        data: labels,
        getPosition: (l) => l.at,
        getText: (l) => l.text,
        getColor: [255, 255, 255, 255],
        getSize: 11,
        sizeUnits: "pixels",
        fontFamily: "ui-monospace, monospace",
        background: true,
        getBackgroundColor: [150, 30, 24, 230],
        backgroundPadding: [4, 2],
        billboard: true,
        parameters: ground,
        updateTriggers: { getText: blocksTaken.version, getPosition: blocksTaken.version },
      }),
    ];
  }

  /** Metres between two places (flat: fine over a few km). */
  function metresBetween(a: { lat: number; lon: number }, b: { lat: number; lon: number }): number {
    const k = Math.cos((a.lat * Math.PI) / 180);
    return Math.hypot((b.lat - a.lat) * 111_320, (b.lon - a.lon) * 111_320 * k);
  }
  const SIGHT_LATEST: [number, number, number, number] = [240, 60, 60, 255];
  const SIGHT_LABEL_BG: [number, number, number, number] = [12, 14, 20, 225];
  const SIGHT_PREDICTED: [number, number, number, number] = [80, 220, 120, 255];
  /**
   * Dev builds: each train's latest report (red), the selected train's with
   * its age and speed, and where the real train most likely is now (green).
   */
  function sightingLayers(list: Train[], selected: Train | null, nowMs: number): Layer[] {
    const sighted = list.filter((t) => t.reports?.length);
    const sel = selected?.reports?.length ? selected : null;
    if (sel && !sighted.some((t) => t.id === sel.id)) sighted.push(sel);
    const points: Sighting[] = sighted.flatMap((t) => t.reports ?? []).filter((s) => s.latest);
    const age = (s: Sighting) => `${Math.max(0, Math.round((nowMs - s.unixMs) / 1000))} s`;
    const speed = (s: Sighting) => (s.speedKmh === null ? "no speed" : `${Math.round(s.speedKmh)} km/h`);
    // the drawn train against the real one: how far ahead (+) or behind (−) where it most likely is now, and how much faster than reported
    const versus = (() => {
      const i = sel ? trainData?.byId.get(sel.id) : undefined;
      if (!sel || i === undefined || !sel.predicted?.length) return "";
      const [lon, lat] = trainData!.positions[i];
      const drawnSpeed = metresBetween(positionAt(sel, nowMs), positionAt(sel, nowMs + 1000));
      const real = positionAt({ lat: sel.lat, lon: sel.lon, frames: sel.predicted }, nowMs);
      const heading = headingAt({ heading: sel.heading, frames: sel.predicted }, nowMs) ?? sel.heading;
      const off = heading === null ? metresBetween({ lat, lon }, real) : aheadOf({ lat, lon }, real, heading);
      const reported = sel.reports?.find((s) => s.latest)?.speedKmh ?? null;
      const signed = (n: number, unit: string) => `${n >= 0 ? "+" : "−"}${Math.abs(Math.round(n))} ${unit}`;
      const faster = reported === null ? "" : `, ${signed(drawnSpeed * 3.6 - reported, "km/h")}`;
      return ` · drawn ${signed(off, "m")}${faster}`;
    })();
    const labels = (sel?.reports ?? []).filter((s) => s.latest).map((s) => ({ s, text: `live ${age(s)} · ${speed(s)}${versus}` }));
    // where the real trains most likely are now, by their reports
    const predicted = sighted
      .filter((t) => (t.predicted?.length ?? 0) > 0)
      .map((t) => ({ selected: t.id === sel?.id, at: positionAt({ lat: t.lat, lon: t.lon, frames: t.predicted! }, nowMs) }));
    const top = { cullMode: "none", depthWriteEnabled: false, depthCompare: "always" } as const;
    return [
      new ScatterplotLayer<(typeof predicted)[number]>({
        id: "dev-predicted",
        data: predicted,
        getPosition: (p) => [p.at.lon, p.at.lat],
        getRadius: 7,
        radiusUnits: "pixels",
        filled: false,
        stroked: true,
        getLineColor: SIGHT_PREDICTED,
        getLineWidth: 2,
        lineWidthUnits: "pixels",
        parameters: top,
        updateTriggers: { getPosition: nowMs },
      }),
      new TextLayer<(typeof predicted)[number]>({
        id: "dev-predicted-label",
        data: predicted.filter((p) => p.selected),
        getPosition: (p) => [p.at.lon, p.at.lat],
        getText: () => "predicted",
        getColor: SIGHT_PREDICTED,
        getSize: 11,
        sizeUnits: "pixels",
        getPixelOffset: [0, 16],
        fontFamily: "ui-monospace, monospace",
        background: true,
        getBackgroundColor: SIGHT_LABEL_BG,
        backgroundPadding: [4, 2],
        billboard: true,
        parameters: top,
        updateTriggers: { getPosition: nowMs },
      }),
      new ScatterplotLayer<Sighting>({
        id: "dev-sightings",
        data: points,
        getPosition: (s) => [s.lon, s.lat],
        getRadius: 4,
        radiusUnits: "pixels",
        filled: true,
        stroked: true,
        getFillColor: SIGHT_LATEST,
        getLineColor: [255, 255, 255, 255],
        getLineWidth: 2,
        lineWidthUnits: "pixels",
        parameters: top,
      }),
      new TextLayer<(typeof labels)[number]>({
        id: "dev-sighting-labels",
        data: labels,
        getPosition: (l) => [l.s.lon, l.s.lat],
        getText: (l) => l.text,
        getColor: SIGHT_LATEST,
        getSize: 11,
        sizeUnits: "pixels",
        getPixelOffset: [0, -14],
        fontFamily: "ui-monospace, monospace",
        background: true,
        getBackgroundColor: SIGHT_LABEL_BG,
        backgroundPadding: [4, 2],
        billboard: true,
        parameters: top,
        updateTriggers: { getText: Math.floor(nowMs / 1000) },
      }),
    ];
  }

  function buildLayers(): Layer[] {
    // the motion model's clock follows the feed's, not this machine's
    const now = motion.now();
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
      // no ceiling: close in, an aircraft is never drawn smaller than it really is
      return Math.max(floor, meters / mpp);
    };
    const depth = depthAt(zoom);
    const material = meshMaterial(depth);
    // the globe's local frame is rotated half a turn from the flat projection's
    const yawOffset = flat ? 0 : 180;

    // skip aircraft that are off-screen or behind the globe
    const radius = visibleRadius(zoom, pitch, container.clientWidth, container.clientHeight);
    const minCos = Math.cos((radius * Math.PI) / 180);
    // airline colours fade in over zoom 6-8, as the models gain depth
    const liveryAmount = Math.round(clamp01((zoom - 6) / 2) * 10) / 10;
    // Straight down and wide: top-down silhouettes, the right idiom for a map
    // read from above. Tilted, or close in, the real shapes instead.
    const useIcons = pitch <= FLAT_PITCH && zoom < CLOSE_ZOOM && aircraftModels.loaded;
    const detail = zoom >= CLOSE_ZOOM ? 1 : 0;

    type Group = {
      /** The published model for this type, or `null` for the built-in shapes. */
      real: AircraftModel | null;
      kind: ModelKind;
      flights: LiveFlight[];
      poses: Pose[];
      positions: [number, number, number][];
      liveries: Livery[];
      index: Map<number, number>;
    };
    // the aircraft the camera is locked to is always drawn, even if the
    // camera is aimed far enough ahead of it to put it past the cull radius
    const lockedOn = follow ? selected?.id : undefined;
    const groups = new Map<string, Group>();
    const icons: LiveFlight[] = [];
    const iconPoses: Pose[] = [];
    const iconPositions: [number, number, number][] = [];
    const iconNames: string[] = [];
    const iconSizes: number[] = [];
    const iconIndex = new Map<number, number>();

    // close in, the airports aircraft are on the ground at get their taxiways (OpenStreetMap),
    // which taxiing aircraft are then drawn on
    const taxiways = zoom >= TAXIWAY_ZOOM;
    // the aircraft to label, at the very positions they are drawn at; zoomed
    // out, only those well short of the horizon, where the text would pile up
    const labelling = aircraftLabels && fontsReady;
    const labelAll = labelling && zoom >= AIRCRAFT_LABEL_MIN_ZOOM;
    const labelled: LiveFlight[] = [];
    const labelledAt: [number, number, number][] = [];
    const labelledSizes: number[] = [];
    for (const f of showAircraft ? flights : []) {
      const facing = cosAngle(f.lon, f.lat, cLon, cLat);
      if (f.id !== lockedOn && facing <= minCos) continue;
      const real = aircraftModels.forFlight(f.typecode, f.icon);
      const pose = motion.pose(f, now);
      if (taxiways) taxiwaysFor(f.id, pose.phase);
      const position = at(pose, ex);
      if (
        labelling &&
        (labelAll || f.id === selected?.id) &&
        (zoom > 6 || facing > 0.42 || f.id === lockedOn) &&
        labelTextOf(f)
      ) {
        labelled.push(f);
        labelledAt.push(position);
        labelledSizes.push(real?.size ?? sizeOf(f.icon));
      }
      if (useIcons && real) {
        iconIndex.set(f.id, icons.length);
        icons.push(f);
        iconPoses.push(pose);
        iconPositions.push(position);
        iconNames.push(real.name);
        iconSizes.push(real.size);
        continue;
      }
      // balloons, ground vehicles and anything else with no published model
      const [kind] = modelFor(f.icon);
      const id = real ? `t-${real.name}` : `k-${kind}`;
      let group = groups.get(id);
      if (!group) groups.set(id, (group = { real, kind, flights: [], poses: [], positions: [], liveries: [], index: new Map() }));
      group.index.set(f.id, group.flights.length);
      group.flights.push(f);
      group.poses.push(pose);
      // the same array, raised onto its wheels below, so labels follow it there too
      group.positions.push(position);
      if (liveryAmount > 0) group.liveries.push(liveries.forFlight(f));
    }

    // Models are centred on their bounding box, so an aircraft placed by its
    // origin at height 0 would stand with its wheels below the map. Raise each
    // by how far its wheels hang below the origin at the size it's drawn, so
    // one on the ground stands on them (a few metres in the air is invisible).
    for (const group of groups.values()) {
      const real = group.real;
      const mesh = real?.gear?.geometry ?? real?.lods[Math.min(detail, real.lods.length - 1)].combined ?? models()[group.kind].combined;
      const wheels = -lowestZ(mesh);
      const divisor = real?.size ?? MODEL_SIZE[group.kind];
      group.positions.forEach((position, i) => {
        const s = (screenPx(real?.size ?? sizeOf(group.flights[i].icon)) * mpp) / divisor;
        position[2] += wheels * s * Math.max(0.04, depth);
      });
    }

    const layers: Layer[] = [earthLayer(basemap)];
    // shaded relief goes on the ground, under the map's water, borders, roads and runways
    if (basemap === "satellite") {
      layers.push(imageryLayer());
      if (terrain) layers.push(...terrainLayers({ flat, zoom, basemap }));
    } else {
      layers.push(landLayer());
      if (terrain) layers.push(...terrainLayers({ flat, zoom, basemap }));
      if (tileUrl) {
        const trueAeroways = RUNWAY_WIDTH_M / mpp >= RUNWAY_PX;
        layers.push(vectorMapLayer(tileUrl, flat, trueAeroways));
        if (trueAeroways) layers.push(aerowayLayer(tileUrl, flat));
      }
    }
    // the railway network: over the map, under the weather, ships, trains and aircraft
    if (railMap) layers.push(...railLayers({ tileUrl, flat, zoom, workerUrl: mvtWorkerUrl, speeds: railSpeeds }));
    if (weather) {
      if (Date.now() - radarCheckedAt > 600_000) refreshRadar();
      const lift = liftFor(pitch);
      if (radarUrl) layers.push(radarLayer(radarUrl), ...radarLifted(ex, lift));
      else freeArenas();
      // only the sources on screen are worth fetching; each is tens of megabytes
      if (isTauri() && Date.now() - measuredCheckedAt > 120_000) {
        refreshMeasured(viewArea(view, container.clientWidth, container.clientHeight));
      }
      for (const volume of measured) layers.push(...measuredLayers(volume, ex, lift));
    } else freeArenas();
    if (daylight) layers.push(nightLayer(now));
    const showBuildings = Boolean(tileUrl && buildings && flat && zoom >= 13);
    if (showBuildings) layers.push(buildingLayer(tileUrl!, shadowSun(now)));
    // close in over buildings, the key light comes from the real sun (fading
    // back to its usual direction as the sun sets), so walls and shadows agree
    const toSunHere = localSun(sunDirection(now), cLat, cLon);
    aimKeyLight(toSunHere, buildings && flat ? sunlight(toSunHere[2]) * clamp01(zoom - FLAT_ABOVE_ZOOM) : 0);
    // on the water, under the labels and every aircraft
    const shipRing = selectedShip ? shipSelectionLayers(selectedShip, mpp, zoom, cLon, cLat) : null;
    if (shipRing) layers.push(shipRing.halo);
    layers.push(...shipLayers(ships, flat, mpp, Date.now()));
    if (shipRing) layers.push(shipRing.ring);
    // on land, like the ships under the labels and every aircraft; the
    // trains' frames are timed on this machine's clock, not the flight feed's
    const trainRing = selectedTrain ? trainSelectionLayers(selectedTrain, zoom, cLon, cLat) : null;
    const trainsDrawn = trainLayers(trains, flat, mpp, Date.now());
    // the signal blocks, free or taken, under the route and the trains
    layers.push(...blockLayers());
    // the selected train's way ahead, from where it is drawn now, under the trains
    if (trainRoute && selectedTrain) layers.push(...trainRouteLayers(trainRoute, selectedTrain));
    if (trainRing) layers.push(trainRing.halo);
    layers.push(...trainsDrawn);
    // dev builds: the reports each train is drawn from
    if (import.meta.env.DEV) layers.push(...sightingLayers(trains, selectedTrain, Date.now()));
    if (trainRing) layers.push(trainRing.ring);
    if (tileUrl && labels && fontsReady) {
      const marked = [airport, ...(selected && route ? [route.origin, route.destination] : [])]
        .map((a) => a?.iata)
        .filter((code): code is string => Boolean(code));
      layers.push(
        labelTileLayer(tileUrl),
        textLayer(
          withoutMarked(labelsInView(cLon, cLat, zoom), marked),
          labelScale,
        ),
      );
    }

    // with the aircraft off, nothing of the selected one either: no trail, route or ring
    const selectedPose = selected && showAircraft ? motion.pose(selected, now) : null;
    if (selected && selectedPose) {
      const pos = at(selectedPose, ex);
      // the aircraft is drawn a few seconds in the past: trail points newer than that
      // would run on past it and back, so the trail stops at the drawn time
      const drawnS = motion.drawnTime(selected.id, now) / 1000;
      const trail = (details?.trail ?? []).filter((p) => (p.latitude || p.longitude) && !(p.timestamp > drawnS));
      // the same airport-relative heights as the aircraft, so the trail meets it
      const path = trail.map((p) => [
        p.longitude,
        p.latitude,
        elevation(motion.drawAltitude(selected.id, p.altitude, p.latitude, p.longitude), ex),
      ]);
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
      if (route) layers.push(...routeLayers(route, pos, selected.id, ex));
    }
    if (airport) layers.push(...airportMarkers("focus-airport", [airport]));
    if (place) layers.push(...placeMarker(place));

    const meshes = models();
    const selectedId = selected?.id;

    // Shadows on the ground, from the real sun: all of them first, so every
    // aircraft draws over every shadow.
    const shadowFade = clamp01((zoom - SHADOW_ZOOM[0]) / (SHADOW_ZOOM[1] - SHADOW_ZOOM[0]));
    if (shadowFade > 0) {
      const sun = sunDirection(now);
      for (const [id, group] of groups) {
        const real = group.real?.lods[Math.min(detail, group.real.lods.length - 1)];
        const shape = real ?? meshes[group.kind];
        const metres = group.real?.size;
        const divisor = metres ?? MODEL_SIZE[group.kind];
        const n = group.poses.length;
        // per aircraft: toward the sun in the mesh's local frame, and opacity
        const toSun = new Float64Array(3 * n);
        const alpha = new Float32Array(n);
        const casting: number[] = [];
        for (let i = 0; i < n; i++) {
          const pose = group.poses[i];
          const [e, north, up] = localSun(sun, pose.lat, pose.lon, shadowSunOut);
          // the true size and height, so the fade doesn't change with zoom
          const a = shadowOpacity(pose.altFt, metres ?? sizeOf(group.flights[i].icon), up) * shadowFade;
          if (a < SHADOW_MIN_ALPHA) continue;
          // the globe's local frame is the flat one turned half a turn (see `orientationOf`)
          toSun[3 * i] = flat ? e : -e;
          toSun[3 * i + 1] = flat ? north : -north;
          toSun[3 * i + 2] = up;
          alpha[i] = a;
          casting.push(i);
        }
        if (!casting.length) continue;
        layers.push(
          new SimpleMeshLayer<number>({
            id: `shadow-${id}-${detail}`,
            data: casting,
            mesh: shape.combined,
            getPosition: (i) => {
              const [lon, lat] = group.positions[i];
              shadowPositionOut[0] = lon;
              shadowPositionOut[1] = lat;
              return shadowPositionOut;
            },
            // the aircraft's own orientation and scale, squashed onto the
            // ground along the sun's rays from the height it is drawn at
            getTransformMatrix: (i) => {
              const s = (screenPx(metres ?? sizeOf(group.flights[i].icon)) * mpp) / divisor;
              shadowSunOut[0] = toSun[3 * i];
              shadowSunOut[1] = toSun[3 * i + 1];
              shadowSunOut[2] = toSun[3 * i + 2];
              const scale = [s, s, s * Math.max(0.04, depth)];
              const orientation = orientationOf(group.poses[i], yawOffset);
              // aircraft are raised onto their wheels (above), so the ground is the map
              return shadowMatrix(orientation, scale, shadowSunOut, group.positions[i][2], shadowMatrixOut);
            },
            getColor: (i) => {
              shadowColorOut[3] = Math.round(alpha[i] * 255);
              return shadowColorOut;
            },
            material: false,
            pickable: false,
            parameters: SHADOW_PARAMETERS,
            updateTriggers: { getPosition: tick, getTransformMatrix: tick, getColor: tick },
          }),
        );
      }
    }

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
        getOrientation: (_: LiveFlight, { index }: { index: number }) => orientationOf(group.poses[index], yawOffset),
        getScale: (f: LiveFlight): [number, number, number] => {
          const s = (screenPx(metres ?? sizeOf(f.icon)) * mpp) / divisor;
          // flattened into a silhouette when zoomed out
          return [s, s, s * Math.max(0.04, depth)];
        },
        material,
        pickable: true,
        // one highlight across all of an aircraft's parts
        highlightedObjectIndex: hoveredId !== null ? (group.index.get(hoveredId) ?? -1) : -1,
        highlightColor: [55, 142, 240, 150] as [number, number, number, number],
        parameters: NO_CULL,
      };
      const triggers = { getPosition: tick, getOrientation: [tick, yawOffset], getScale: [mpp, minPx, depth] };
      const gear = group.real?.gear;
      // gear only once the models have depth; a silhouette has nowhere to hang it
      if (gear && depth >= GEAR_DEPTH) {
        const down: number[] = [];
        group.poses.forEach((pose, i) => pose.gear > 0 && down.push(i));
        if (down.length) {
          const flatten = Math.max(0.04, depth);
          layers.push(
            new SimpleMeshLayer<number>({
              id: `gear-${id}`,
              data: down,
              mesh: gear.geometry,
              getPosition: (i) => group.positions[i],
              getOrientation: (i) => orientationOf(group.poses[i], yawOffset),
              // retracted by squashing it up into the airframe about its attach plane
              getScale: (i) => {
                const s = (screenPx(metres!) * mpp) / divisor;
                return [s, s, s * flatten * group.poses[i].gear];
              },
              getTranslation: (i) => {
                const s = (screenPx(metres!) * mpp) / divisor;
                const lift = gear.attachZ * (1 - group.poses[i].gear) * s * flatten;
                const [x, y, z] = upAxis(orientationOf(group.poses[i], yawOffset));
                return [x * lift, y * lift, z * lift];
              },
              getColor: GEAR_COLOR,
              material,
              parameters: NO_CULL,
              updateTriggers: {
                getPosition: tick,
                getOrientation: [tick, yawOffset],
                getScale: [tick, mpp, minPx, depth],
                getTranslation: [tick, mpp, minPx, depth, yawOffset],
              },
            }),
          );
        }
      }
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
      // each icon turned to its heading as seen on screen, which away from the
      // middle of the globe is not simply `bearing - heading` (see screenAngle)
      // built from the view state rather than asked of deck.gl, whose
      // `getViewports` can assert while a camera move is under way; icons only
      // show below zoom 11, where the view is always the globe projection
      const viewport = new GlobeViewport({ ...view, width: container.clientWidth, height: container.clientHeight });
      const step = ICON_HEADING_STEP_PX * mpp;
      const iconAngles = iconPoses.map((pose, i) => {
        const angle = screenAngle(viewport, iconPositions[i], pose.heading, step);
        return angle ?? bearing - pose.heading;
      });
      // a thin dark border under every silhouette, so they stand out on bright
      // weather and imagery and from each other where traffic is dense
      if (aircraftModels.outlineAtlas) {
        layers.push(
          new IconLayer<LiveFlight>({
            id: "aircraft-icon-outlines",
            data: icons,
            getPosition: (_, { index }) => iconPositions[index],
            getIcon: (_, { index }) => iconNames[index],
            iconAtlas: aircraftModels.outlineAtlas,
            iconMapping: aircraftModels.outlineMapping,
            // the same silhouette size, in a cell padded for the border
            getSize: (_, { index }) => screenPx(iconSizes[index]) * 1.06 * aircraftModels.outlineScale,
            getAngle: (_, { index }) => iconAngles[index],
            getColor: OUTLINE_COLOR,
            sizeUnits: "pixels",
            billboard: true,
            parameters: NO_CULL,
            updateTriggers: {
              getPosition: tick,
              getAngle: [tick, bearing, pitch, zoom, cLon, cLat],
              getSize: [mpp, minPx],
            },
          }),
        );
      }
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
          getAngle: (_, { index }) => iconAngles[index],
          getColor: (f) => (f.id === selectedId ? SELECTED_COLOR : AIRCRAFT_COLOR),
          sizeUnits: "pixels",
          billboard: true,
          pickable: true,
          highlightedObjectIndex: hoveredId !== null ? (iconIndex.get(hoveredId) ?? -1) : -1,
          highlightColor: [55, 142, 240, 150],
          parameters: NO_CULL,
          updateTriggers: {
            getPosition: tick,
            getAngle: [tick, bearing, pitch, zoom, cLon, cLat],
            getSize: [mpp, minPx],
            getColor: selectedId,
          },
        }),
      );
    }

    // labels over every aircraft, so none is drawn across one
    if (labelling) {
      updateLabelSet(labelled, labelledAt, labelledSizes, zoom > AIRCRAFT_LABEL_HEIGHT_ZOOM);
      layers.push(...aircraftLabelLayers(screenPx, [mpp, minPx], selectedId, labelScale, zoom));
    } else if (labelSet.items.length) {
      // let go of the last feed's labels
      labelSet = emptyLabelSet(labelSet.version + 1);
      labelPositions = [];
    }

    // ring the selection, drawn over neighbouring aircraft but not through the globe
    if (selected && selectedPose && cosAngle(selectedPose.lon, selectedPose.lat, cLon, cLat) > 0) {
      layers.push(
        new ScatterplotLayer({
          id: "selection-ring",
          data: [at(selectedPose, ex)],
          getPosition: (d) => d,
          getRadius: screenPx(sizeOf(selected.icon)) * 0.6 + 4,
          radiusUnits: "pixels",
          stroked: true,
          filled: false,
          getLineColor: [55, 142, 240, 230],
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
  /** Locked on the globe, the aircraft's height may reach this much of the camera's. */
  const LOCK_CAMERA_ABOVE = 0.9;
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
    // the same pose the model is drawn at, or the camera shakes against it
    const pose = motion.pose(flight, motion.now());
    const [lon, lat] = [pose.lon, pose.lat];
    const altitude = elevation(pose.altFt, effectiveExaggeration(exaggeration, v.zoom));
    // Close in, the projection is flat and can raise the camera's target to the
    // aircraft's own height, which centres it at any tilt, bearing and zoom. A
    // ground point aimed along the bearing can't: the camera sits only a few
    // hundred metres out by street level, below a real cruise altitude.
    if (v.zoom > FLAT_ABOVE_ZOOM) {
      lockedMaxPitch = FOLLOW_MAX_PITCH;
      return { ...v, longitude: lon, latitude: lat, position: [0, 0, altitude] };
    }
    // The globe has no such offset, and the camera aims at a point on the
    // ground, so once the view is tilted an aircraft drawn at altitude rides
    // above the centre of the screen. Aiming further along the bearing puts
    // the aircraft itself on the camera's axis.
    const camera = CAMERA_HEIGHTS * container.clientHeight * metersPerPixel(v.zoom, lat);
    // That only works while the camera is above the aircraft. Tilted further,
    // the camera would sink below it and look at the ground under it, and the
    // aircraft would slide up off the screen as the camera seemed to drop. So
    // the tilt stops there instead; zoomed in past the switch to the flat
    // projection it opens up again.
    lockedMaxPitch = Math.min(
      FOLLOW_MAX_PITCH,
      Math.acos(Math.min(1, altitude / (LOCK_CAMERA_ABOVE * camera))) / DEGREES,
    );
    const tilt = Math.min(v.pitch, lockedMaxPitch);
    const pitch = tilt * DEGREES;
    const height = Math.min(altitude, LOCK_CAMERA_ABOVE * camera * Math.cos(pitch));
    const lift = height * Math.tan(pitch);
    const latitude = Math.max(-85, Math.min(85, lat + (lift * Math.cos(v.bearing * DEGREES)) / METRES_PER_DEGREE));
    const longitude =
      lon + (lift * Math.sin(v.bearing * DEGREES)) / (METRES_PER_DEGREE * Math.max(0.05, Math.cos(lat * DEGREES)));
    return { ...v, longitude, latitude, pitch: tilt, position: undefined };
  }

  /** Draws a frame; a failure is reported once and drawing pauses until Retry. */
  function render() {
    if (mapError) return;
    try {
      renderFrame();
    } catch (e) {
      reportMapError(e);
    }
  }

  /** Where the followed train or ship is drawn now, `[lon, lat]`; none when not following one. */
  function vehicleAt(): [number, number] | null {
    if (!followVehicle || (follow && selected)) return null;
    if (selectedTrain) {
      const i = trainData?.byId.get(selectedTrain.id);
      return i === undefined ? [selectedTrain.lon, selectedTrain.lat] : [trainData!.positions[i][0], trainData!.positions[i][1]];
    }
    if (selectedShip) {
      const i = shipData?.byMmsi.get(selectedShip.mmsi);
      return i === undefined ? [selectedShip.lon, selectedShip.lat] : [shipData!.positions[i][0], shipData!.positions[i][1]];
    }
    return null;
  }
  /** Following a train or ship: the camera flies there, then keeps it in the middle. */
  let vehicleLocked = false;
  $effect(() => {
    if (!followVehicle) {
      vehicleLocked = false;
      return;
    }
    const at = untrack(vehicleAt);
    if (!at) return;
    untrack(() => goTo({ longitude: at[0], latitude: at[1], zoom: Math.max(view.zoom, 13) }, 800, ["longitude", "latitude", "zoom"]));
    vehicleLocked = true;
  });

  function renderFrame() {
    tick++;
    // following a train or ship: centred on it every frame (once there)
    const vehicle = vehicleLocked && Date.now() > transitionUntil ? vehicleAt() : null;
    if (vehicle) {
      view = { ...view, longitude: vehicle[0], latitude: vehicle[1] };
      onviewchange?.(view);
      deck?.setProps({ initialViewState: { ...view, ...limits() } });
    }
    // locked to an aircraft, the wheel has to zoom about the centre, or every
    // notch shoves the aircraft aside and the lock drags it straight back; a
    // drag orbits for the same reason
    const wanted = (follow && selected) || vehicleLocked ? "center" : "pointer";
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
      deck?.setProps({ initialViewState: { ...view, ...limits() } });
    } else if (raised && !(follow && selected)) {
      // the lock was released with the camera raised: bring it back to the ground
      raised = false;
      view = { ...view, position: undefined };
      // `position` is the flat projection's; GlobeViewState doesn't declare it
      const grounded: ViewState = { ...view, position: [0, 0, 0] };
      deck?.setProps({ initialViewState: { ...grounded, ...LIMITS } });
    }
    // released from a lock closer or flatter than the free camera allows: ease back within its limits
    const locked = Boolean(follow && selected);
    if (wasLocked && !locked && (view.zoom > LIMITS.maxZoom || view.pitch > LIMITS.maxPitch)) {
      goTo(
        { zoom: Math.min(view.zoom, LIMITS.maxZoom), pitch: Math.min(view.pitch, LIMITS.maxPitch) },
        600,
        ["zoom", "pitch"],
      );
    }
    wasLocked = locked;
    deck?.setProps({ layers: buildLayers() });
  }

  /** Re-render on an interval that depends on zoom: motion is invisible zoomed out. */
  function loop(t: number) {
    // locked on an aircraft the camera and the aircraft must move together
    // every frame, or at street zoom each 50 ms step is many pixels of jitter
    const locked = ((follow && selected) || vehicleLocked) && Date.now() > transitionUntil;
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
        ...limits(),
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
      zoom: Math.min(zoom ?? (near ? Math.max(view.zoom, 8) : 8), maxZoomNow()),
    };
    goTo(target, 1500, ["longitude", "latitude", "zoom"]);
  }

  export function zoomBy(delta: number) {
    const zoom = Math.min(maxZoomNow(), Math.max(LIMITS.minZoom, view.zoom + delta));
    goTo({ zoom }, 250, ["zoom"]);
  }

  /** North up and looking straight down. */
  export function resetOrientation() {
    goTo({ bearing: 0, pitch: 0 }, 400, ["bearing", "pitch"]);
  }

  export function setPitch(pitch: number) {
    goTo({ pitch }, 400, ["pitch"]);
  }

  /**
   * The selected ship: a soft amber halo under it and a ring over it, sized to
   * clear its icon zoomed out and its true-scale hull close in, so it reads at
   * every zoom. Nothing while it is round the back of the globe.
   */
  function shipSelectionLayers(ship: Ship, mpp: number, zoom: number, cLon: number, cLat: number) {
    if (zoom <= FLAT_ABOVE_ZOOM && cosAngle(ship.lon, ship.lat, cLon, cLat) <= 0) return null;
    const hullPx = ship.dims ? Math.max(ship.dims[0], ship.dims[1]) / mpp : 0;
    const radius = Math.max(shipIconSize(ship, zoom) / 2 + 5, hullPx + 6);
    const common = {
      data: [[ship.lon, ship.lat, 0] as [number, number, number]],
      getPosition: (d: [number, number, number]) => d,
      getRadius: radius,
      radiusUnits: "pixels" as const,
      billboard: true,
      parameters: { ...NO_CULL, depthWriteEnabled: false, depthCompare: "always" as const },
      updateTriggers: { getRadius: radius },
    };
    return {
      halo: new ScatterplotLayer({ ...common, id: "ship-selection-halo", filled: true, stroked: false, getFillColor: [55, 142, 240, 56] }),
      ring: new ScatterplotLayer({
        ...common,
        id: "ship-selection-ring",
        filled: false,
        stroked: true,
        getLineColor: [55, 142, 240, 235],
        getLineWidth: 2,
        lineWidthUnits: "pixels",
      }),
    };
  }

  /**
   * The selected train: the ships' halo and ring, round the train where it
   * is drawn now (it may be moving along its frames).
   */
  function trainSelectionLayers(train: Train, zoom: number, cLon: number, cLat: number) {
    const i = trainData?.byId.get(train.id);
    const [lon, lat] = i !== undefined && trainData ? trainData.positions[i] : [train.lon, train.lat];
    if (zoom <= FLAT_ABOVE_ZOOM && cosAngle(lon, lat, cLon, cLat) <= 0) return null;
    const heading = i !== undefined && trainData ? trainData.headings[i] : train.heading;
    const radius = Math.round(trainIconSize(train.kind, heading, zoom) / 2 + 5);
    const common = {
      data: [[lon, lat, 0] as [number, number, number]],
      getPosition: (d: [number, number, number]) => d,
      getRadius: radius,
      radiusUnits: "pixels" as const,
      billboard: true,
      parameters: { ...NO_CULL, depthWriteEnabled: false, depthCompare: "always" as const },
      updateTriggers: { getRadius: radius },
    };
    return {
      halo: new ScatterplotLayer({ ...common, id: "train-selection-halo", filled: true, stroked: false, getFillColor: [55, 142, 240, 56] }),
      ring: new ScatterplotLayer({
        ...common,
        id: "train-selection-ring",
        filled: false,
        stroked: true,
        getLineColor: [55, 142, 240, 235],
        getLineWidth: 2,
        lineWidthUnits: "pixels",
      }),
    };
  }

  /** The train a pick landed on. */
  function pickedTrain(info: PickingInfo): Train | null {
    if (info.layer?.id !== "train-icons" || typeof info.object !== "number") return null;
    return (trainData?.list ?? trains)[info.object] ?? null;
  }

  /** The ship a pick landed on, whichever of the ship layers drew it. */
  function pickedShip(info: PickingInfo): Ship | null {
    const o = info.object as unknown;
    if (!info.layer?.id.startsWith("ship-") || info.layer.id.startsWith("ship-selection") || o == null) return null;
    const list = shipData?.list ?? ships;
    if (typeof o === "number") return list[o] ?? null;
    if (typeof o === "object") {
      if ("mmsi" in o) return o as Ship;
      if ("index" in o && typeof o.index === "number") return list[o.index] ?? null;
    }
    return null;
  }

  const escape = (s: string) => s.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);

  /** Airport labels, ships and trains get deck's tooltip; aircraft get the {@link HoverCard}. */
  function tooltip(info: PickingInfo) {
    // icons and models alike (the models carry their ship's index)
    const ship = pickedShip(info);
    if (ship) {
      const length = ship.dims ? ` · ${ship.dims[0] + ship.dims[1]} m` : "";
      return {
        html: `<div class="tt-head">${escape(ship.name)}</div><div class="tt-sub">${escape(shipKindLabel(ship.kind))}${length}</div><div class="tt-sub">MMSI ${ship.mmsi}${ship.old ? " · old position" : ""}</div>`,
        className: "aircraft-tooltip",
      };
    }
    const train = pickedTrain(info);
    if (train) {
      const kind = [trainLabel(train.kind), train.operator].filter(Boolean).join(" · ");
      const where = [train.destination && `to ${train.destination}`, delayText(train.delayMinutes)].filter(Boolean).join(" · ");
      const age = trainPositionAge(train.positionUnix, Date.now());
      return {
        html: `<div class="tt-head">${escape(train.name)}</div><div class="tt-sub">${escape(kind)}</div>${where ? `<div class="tt-sub">${escape(where)}</div>` : ""}<div class="tt-sub${age.stale ? " tt-warn" : ""}">${escape(age.label)}</div>`,
        className: "aircraft-tooltip",
      };
    }
    if (info.layer?.id !== "labels") return null;
    const p = (info.object as LabelFeature | undefined)?.properties;
    if (p?.layerName !== "aerodrome_label") return null;
    return {
      html: `<div class="tt-head">${escape(nameOf(p))}</div><div class="tt-sub">Open departures and arrivals</div>`,
      className: "aircraft-tooltip",
    };
  }

  // The hover card. The pointer's position is kept apart from the flight, so
  // moving over one aircraft only moves the card and recomputes nothing else.
  let hoverFlight = $state.raw<LiveFlight | null>(null);
  let pointer = $state({ x: 0, y: 0 });
  let areaWidth = $state(0);
  let areaHeight = $state(0);
  /** Bumped when a details request settles, so the card reads the loader again. */
  let hoverVersion = $state(0);
  const hoverLoader = new HoverLoader({ fetch: flightDetails, isRateLimited, onchange: () => hoverVersion++ });
  onDestroy(() => hoverLoader.dispose());

  /** The selected flight's details are already streaming in: no need to ask again. */
  const ownDetails = (id: number) => (id === selected?.id && details ? details : null);

  function setHover(flight: LiveFlight | null, x = 0, y = 0) {
    if (flight) pointer = { x, y };
    if (flight?.id === hoverFlight?.id) return;
    hoverLoader.hover(flight && !ownDetails(flight.id) ? flight.id : null);
    hoverFlight = flight;
  }

  /** The latest report of the hovered flight, as the feed refreshes under a resting pointer. */
  const hovered = $derived.by(() => {
    const id = hoverFlight?.id;
    return id === undefined ? null : (flights.find((f) => f.id === id) ?? hoverFlight);
  });

  const hoverCard = $derived.by(() => {
    if (!hovered) return null;
    void hoverVersion;
    void reference.ready; // airports resolve once the reference data is in
    const own = ownDetails(hovered.id);
    const info = own ? hoverInfo(own) : hoverLoader.info(hovered.id);
    const status = own ? ("ready" as const) : hoverLoader.status(hovered.id);
    const origin = reference.airport(hovered.origin);
    const destination = reference.airport(hovered.destination);
    let progress: { value: number; measured: boolean } | null = null;
    if (info?.progressPct != null) progress = { value: info.progressPct / 100, measured: true };
    else {
      const estimate = routeProgress(hovered, origin, destination);
      if (estimate !== null) progress = { value: estimate, measured: false };
    }
    return {
      flight: hovered,
      from: origin?.city || hovered.origin,
      to: destination?.city || hovered.destination,
      progress,
      info,
      status,
    };
  });

  onMount(() => {
    try {
      createDeck();
    } catch (e) {
      reportMapError(e);
      return;
    }
    // trackpads as in Apple Maps: ahead of deck.gl's own handling (capture), which zooms on any wheel
    container.addEventListener("wheel", onWheel, { capture: true, passive: false });
    for (const [type, handler] of PINCH_HANDLERS) container.addEventListener(type, handler as EventListener, { capture: true });
    // a lost WebGL context draws nothing until it's restored
    container.querySelector("canvas")?.addEventListener("webglcontextlost", () =>
      reportMapError(new Error("The graphics context was lost (WebGL context lost)")),
    );
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

  /** The view as deck.gl draws it: the flat projection above zoom 12, else the globe. */
  function viewportOf(v: ViewState) {
    const [width, height] = [container.clientWidth, container.clientHeight];
    return v.zoom > FLAT_ABOVE_ZOOM ? new WebMercatorViewport({ ...v, width, height }) : new GlobeViewport({ ...v, width, height });
  }
  /** Where a point on screen is on the map, `[lon, lat]`; none off the globe. */
  function mapAt(v: ViewState, at: [number, number]): [number, number] | null {
    const p = viewportOf(v).unproject(at) as number[] | null;
    return p && Number.isFinite(p[0]) && Number.isFinite(p[1]) ? [p[0], p[1]] : null;
  }
  /** Locked on to an aircraft, train or ship: the camera only zooms and turns about it. */
  const cameraLocked = () => Boolean((follow && selected) || vehicleLocked);
  /** The camera moved by a gesture of ours (deck.gl hears only its own). */
  function moveView(next: ViewState) {
    const lim = limits();
    view = {
      ...next,
      zoom: Math.max(lim.minZoom, Math.min(lim.maxZoom, next.zoom)),
      latitude: Math.max(-85, Math.min(85, next.latitude)),
      bearing: wrapBearing(next.bearing),
    };
    if (follow && selected) view = lockedView(view, selected);
    if (Math.floor(view.zoom * 2) !== Math.floor(shownZoom * 2)) shownZoom = view.zoom;
    onviewchange?.(view);
    deck?.setProps({ initialViewState: { ...view, ...lim } });
    render();
  }
  /** Zoomed and turned, keeping the place under `at` (on screen) where it is. */
  function zoomAbout(zoom: number, bearing: number, at: [number, number]) {
    let next = { ...view, zoom, bearing };
    const before = cameraLocked() ? null : mapAt(view, at);
    const after = before && mapAt(next, at);
    if (before && after) next = { ...next, longitude: next.longitude + before[0] - after[0], latitude: next.latitude + before[1] - after[1] };
    moveView(next);
  }
  /** Two fingers moved together on a trackpad: the map moves with them. A mouse wheel is left to deck.gl (zoom). */
  function onWheel(e: WheelEvent) {
    // a pinch while WebKit's own pinch events come: those zoom
    if (e.ctrlKey && pinch) {
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    if (!isTrackpadScroll(e)) return;
    e.preventDefault();
    e.stopPropagation();
    if (cameraLocked() || Date.now() < transitionUntil) return;
    const [dx, dy] = wheelPixels(e, container.clientHeight);
    const centre: [number, number] = [container.clientWidth / 2 + dx, container.clientHeight / 2 + dy];
    const p = mapAt(view, centre);
    if (p) moveView({ ...view, longitude: p[0], latitude: p[1] });
  }
  /** A pinch under way (WebKit): the view it started from, and where between the fingers. */
  let pinch: { zoom: number; bearing: number; at: [number, number] } | null = null;
  const PINCH_HANDLERS: [string, (e: PinchEvent) => void][] = [
    [
      "gesturestart",
      (e) => {
        e.preventDefault();
        const box = container.getBoundingClientRect();
        pinch = { zoom: view.zoom, bearing: view.bearing, at: [e.clientX - box.left, e.clientY - box.top] };
      },
    ],
    [
      "gesturechange",
      (e) => {
        e.preventDefault();
        if (!pinch) return;
        const { zoom, bearing } = pinched(pinch, e);
        const at: [number, number] = cameraLocked() ? [container.clientWidth / 2, container.clientHeight / 2] : pinch.at;
        zoomAbout(zoom, bearing, at);
      },
    ],
    [
      "gestureend",
      (e) => {
        e.preventDefault();
        pinch = null;
      },
    ],
  ];

  function createDeck() {
    deck = new Deck<GlobeView>({
      parent: container,
      views: new GlobeView({ resolution: 5 }),
      initialViewState: { ...view, ...LIMITS },
      controller: controllerFor(false),
      effects: [lighting],
      // the shadows mark it (SHADOW_PARAMETERS); deck.gl clears it each frame
      deviceProps: { webgl: { stencil: true } },
      pickingRadius: 6,
      onDeviceInitialized: (d) => (device = d),
      // a layer that fails (a shader the GPU driver rejects, say) is reported, not just logged
      onError: (error) => reportMapError(error),
      onViewStateChange: ({ viewState }) => {
        const { longitude, latitude, zoom, bearing = 0, pitch = 0 } = viewState as Partial<ViewState>;
        view = { longitude: longitude!, latitude: latitude!, zoom: zoom!, bearing, pitch };
        if (Math.floor(view.zoom * 2) !== Math.floor(shownZoom * 2)) shownZoom = view.zoom;
        // a gesture while locked on is re-aimed in the same frame, not on the next tick
        const locked = follow && selected && Date.now() > transitionUntil;
        if (locked) view = lockedView(view, selected!);
        // following a train or ship, a drag doesn't move it out of the middle
        const vehicle = !locked && vehicleLocked && Date.now() > transitionUntil ? vehicleAt() : null;
        if (vehicle) view = { ...view, longitude: vehicle[0], latitude: vehicle[1] };
        onviewchange?.(view);
        return locked || vehicle ? { ...viewState, ...view } : viewState;
      },
      getTooltip: tooltip,
      getCursor: ({ isDragging, isHovering }) => (isDragging ? "grabbing" : isHovering ? "pointer" : "grab"),
      onHover: (info) => {
        const flight = info.layer?.id.startsWith("aircraft-") ? ((info.object as LiveFlight | undefined) ?? null) : null;
        setHover(flight, info.x, info.y);
        const id = flight?.id ?? null;
        if (id !== hoveredId) {
          hoveredId = id;
          render();
        }
      },
      // hover isn't reported while a button is down: the card would hang over a moving globe
      onDragStart: () => setHover(null),
      onClick: (info) => {
        const ship = pickedShip(info);
        const train = pickedTrain(info);
        if (info.layer?.id.startsWith("aircraft-") && info.object) onselect(info.object as LiveFlight);
        else if (ship) onshipselect?.(ship);
        else if (train) ontrainselect?.(train);
        else if (info.layer?.id === "labels") {
          const p = (info.object as LabelFeature | undefined)?.properties;
          if (p?.layerName === "aerodrome_label" && p.iata) onairport?.(String(p.iata));
        } else if (!info.layer) onselect(null);
      },
      layers: [earthLayer(basemap)],
    });
  }

  onDestroy(() => {
    container?.removeEventListener("wheel", onWheel, { capture: true });
    for (const [type, handler] of PINCH_HANDLERS) container?.removeEventListener(type, handler as EventListener, { capture: true });
    cancelAnimationFrame(frame);
    freeArenas();
    deck?.finalize();
    deck = null;
  });

  // re-render immediately when inputs change, rather than waiting for the next tick
  $effect(() => {
    void [flights, ships, selected, details, route, airport, place, exaggeration, basemap, labels, labelScale, buildings];
    void [showAircraft, aircraftLabels, daylight, weather, follow, selectedShip, terrain, railMap, trains, selectedTrain, trainRoute, trainBlocks];
    void [hoveredTrainId, hoveredShipMmsi];
    render();
  });
</script>

<div class="globe" bind:this={container} bind:clientWidth={areaWidth} bind:clientHeight={areaHeight}>
  {#if hoverCard}
    <HoverCard {...hoverCard} x={pointer.x} y={pointer.y} width={areaWidth} height={areaHeight} />
  {/if}
  {#if railMap && railSpeeds && shownZoom >= RAIL_MIN_ZOOM}
    <div class="speed-legend" aria-label="Track speed limits, km/h">
      <span class="unit">km/h</span>
      {#each SPEED_BANDS as band (band.label)}
        <span class="band" style:--c="rgb({band.color.slice(0, 3).join(' ')})">{band.upTo === Infinity ? `${band.label}+` : band.label}</span>
      {/each}
      <span class="band" style:--c="rgb({SPEED_UNKNOWN.slice(0, 3).join(' ')})" title="Limit not mapped">?</span>
    </div>
  {/if}
  {#if mapError}
    <div class="map-error surface" role="alert">
      <p class="title">The map couldn't draw</p>
      <p class="message">{mapError.message}</p>
      <div class="actions">
        <button class="button" onclick={() => navigator.clipboard.writeText(mapErrorDetails())}>Copy details</button>
        <button class="button" onclick={retryMap}>Retry</button>
      </div>
    </div>
  {/if}
</div>

<style>
  .globe {
    position: absolute;
    inset: 0;
  }
  /* the speed colours, small along the bottom: the lines say the rest close in */
  .speed-legend {
    position: absolute;
    left: 50%;
    bottom: calc(var(--sheet-h, 0px) + 26px);
    z-index: 2;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 3px 6px;
    border-radius: var(--radius-sm);
    background: oklch(0.15 0 0 / 0.8);
    font: 600 10px / 1 var(--font-data);
    color: var(--text-2);
    pointer-events: none;
    white-space: nowrap;
  }
  .speed-legend .unit {
    margin-right: 4px;
    color: var(--text-3);
  }
  .speed-legend .band {
    min-width: 26px;
    padding: 3px 4px;
    border-radius: 2px;
    background: var(--c);
    color: oklch(0.16 0.01 260);
    text-align: center;
  }
  .map-error {
    position: absolute;
    top: 50%;
    left: 50%;
    z-index: 5;
    transform: translate(-50%, -50%);
    width: min(420px, calc(100% - 2 * var(--space-4)));
    padding: var(--space-4);
    border-radius: var(--radius);
  }
  .map-error .title {
    font-weight: 600;
    color: var(--text-1);
  }
  .map-error .message {
    margin: var(--space-2) 0 var(--space-3);
    font-family: var(--font-data);
    font-size: var(--text-sm);
    color: var(--text-2);
    overflow-wrap: anywhere;
  }
  .map-error .actions {
    display: flex;
    gap: var(--space-2);
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
  :global(.aircraft-tooltip .tt-warn) {
    color: var(--warn);
  }
</style>
