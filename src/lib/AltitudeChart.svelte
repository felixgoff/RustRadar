<script lang="ts">
  import type { TrailPoint } from "./api";
  import { clockAt, minutesOfDay, zoneName } from "./time";

  interface Props {
    /** The flight's recorded track, oldest first. */
    trail: TrailPoint[];
    /**
     * IANA zone for the time axis: the departure airport's, so the axis
     * agrees with the departure time shown above it.
     */
    timeZone?: string;
    /** Whose time that is, e.g. `LHR`. */
    place?: string;
  }

  let { trail, timeZone, place }: Props = $props();

  // room for the y tick labels on the left, the end dot on the right, an end
  // label above the ceiling and the time labels below the baseline
  const LEFT = 46;
  const RIGHT = 8;
  const TOP = 18;
  const PLOT = 96;
  const AXIS = 18;
  const HEIGHT = TOP + PLOT + AXIS;
  const TABLE_ROWS = 30;

  let width = $state(0);
  let active = $state<number | null>(null);
  let table = $state(false);
  let svg = $state<SVGSVGElement>();

  const points = $derived(trail.filter((p) => p.timestamp > 0).sort((a, b) => a.timestamp - b.timestamp));
  const t0 = $derived(points[0]?.timestamp ?? 0);
  const t1 = $derived(points.at(-1)?.timestamp ?? 0);
  const drawable = $derived(points.length > 1 && t1 > t0 && width > LEFT + RIGHT);

  /** A clean ceiling with at most four intervals: 0 / 10,000 / 20,000 / 30,000 / 40,000. */
  function niceScale(max: number): { top: number; step: number } {
    for (const step of [250, 500, 1_000, 2_000, 2_500, 5_000, 10_000, 15_000, 20_000]) {
      const top = Math.max(step, Math.ceil(max / step) * step);
      if (top / step <= 4) return { top, step };
    }
    return { top: Math.ceil(max / 20_000) * 20_000, step: 20_000 };
  }

  // headroom above the highest point keeps the end label inside the chart
  const scale = $derived(niceScale(Math.max(1_000, ...points.map((p) => p.altitude)) * 1.12));
  const yTicks = $derived(Array.from({ length: scale.top / scale.step + 1 }, (_, i) => i * scale.step));

  const x = (t: number) => LEFT + ((t - t0) / (t1 - t0)) * (width - LEFT - RIGHT);
  const y = (alt: number) => TOP + PLOT - (Math.max(0, alt) / scale.top) * PLOT;

  const clock = (t: number) => clockAt(t, timeZone);
  const feet = (alt: number) => `${Math.max(0, alt).toLocaleString()} ft`;

  /** Ticks on round local times, at most four across the flight so far. */
  const xTicks = $derived.by(() => {
    if (!drawable) return [];
    const span = t1 - t0;
    const minutes = [5, 10, 15, 30, 60, 120, 180, 360, 720].find((m) => span / (m * 60) <= 4) ?? 720;
    const start = Math.floor(t0 / 60) * 60;
    const local = minutesOfDay(start, timeZone);
    const ticks: number[] = [];
    for (let t = start + (Math.ceil(local / minutes) * minutes - local) * 60; t <= t1; t += minutes * 60) {
      // a label that would hang off either end is dropped, not clipped
      if (x(t) - 18 >= LEFT - 6 && x(t) + 18 <= width) ticks.push(t);
    }
    return ticks;
  });

  const line = $derived(
    drawable ? points.map((p, i) => `${i ? "L" : "M"}${x(p.timestamp).toFixed(1)},${y(p.altitude).toFixed(1)}`).join("") : "",
  );
  const area = $derived(drawable ? `${line}L${x(t1).toFixed(1)},${TOP + PLOT}L${LEFT},${TOP + PLOT}Z` : "");

  const last = $derived(points.at(-1));
  /**
   * Beside the end dot, on the side the line doesn't come from: above when
   * climbing or level, below when descending. Descending close to the ground
   * there's no clear side, and the label is left to the tooltip and table.
   */
  const endLabel = $derived.by(() => {
    if (!drawable || !last) return null;
    const ex = x(last.timestamp);
    const ey = y(last.altitude);
    const earlier = points[Math.max(0, points.findIndex((p) => x(p.timestamp) >= ex - 64))];
    if (earlier.altitude <= last.altitude + 300) return { x: ex - 9, y: ey - 8 };
    return ey + 18 <= TOP + PLOT ? { x: ex - 9, y: ey + 16 } : null;
  });

  function nearest(t: number): number {
    let lo = 0;
    let hi = points.length - 1;
    while (hi - lo > 1) {
      const mid = (lo + hi) >> 1;
      if (points[mid].timestamp < t) lo = mid;
      else hi = mid;
    }
    return t - points[lo].timestamp <= points[hi].timestamp - t ? lo : hi;
  }

  function onpointermove(e: PointerEvent) {
    if (!drawable || !svg) return;
    const px = e.clientX - svg.getBoundingClientRect().left;
    const t = t0 + ((px - LEFT) / (width - LEFT - RIGHT)) * (t1 - t0);
    active = nearest(Math.min(t1, Math.max(t0, t)));
  }

  function onkeydown(e: KeyboardEvent) {
    if (!drawable) return;
    const step = e.shiftKey ? Math.max(1, Math.round(points.length / 10)) : 1;
    const current = active ?? points.length - 1;
    const next =
      e.key === "ArrowLeft" ? current - step
      : e.key === "ArrowRight" ? current + step
      : e.key === "Home" ? 0
      : e.key === "End" ? points.length - 1
      : null;
    if (next === null) return;
    e.preventDefault();
    active = Math.min(points.length - 1, Math.max(0, next));
  }

  const hovered = $derived(active !== null ? points[active] : null);
  const zone = $derived(t0 ? zoneName(t0, timeZone) : "");
  const reading = $derived(hovered ?? points.at(-1));
  const highest = $derived(points.reduce((best, p) => (p.altitude > best.altitude ? p : best), points[0]));
  const summary = $derived(
    drawable && last && highest
      ? `Altitude since ${clock(t0)}: highest ${feet(highest.altitude)} at ${clock(highest.timestamp)}, now ${feet(last.altitude)}.`
      : "",
  );
  const rows = $derived.by(() => {
    const stride = Math.max(1, Math.ceil(points.length / TABLE_ROWS));
    const sampled = points.filter((_, i) => i % stride === 0);
    if (last && sampled.at(-1) !== last) sampled.push(last);
    return sampled;
  });
</script>

<section class="chart" aria-labelledby="altitude-heading">
  <header>
    <h3 id="altitude-heading">Altitude <span>feet</span></h3>
    {#if drawable}
      <span class="tools">
        <span class="zone" title="Times are given in {place ?? 'this'} time ({zone})">{place ? `${place} time` : zone}</span>
        <button class="toggle" aria-pressed={table} onclick={() => (table = !table)}>
          {table ? "Chart" : "Table"}
        </button>
      </span>
    {/if}
  </header>

  {#if points.length < 2 || t1 <= t0}
    <p class="empty">The altitude profile appears once the aircraft has reported a few positions.</p>
  {:else if table}
    <div class="table">
      <table>
        <thead><tr><th scope="col">Time</th><th scope="col">Altitude</th><th scope="col">Speed</th></tr></thead>
        <tbody>
          {#each rows as p (p.timestamp)}
            <tr>
              <td class="data">{clock(p.timestamp)}</td>
              <td class="data">{feet(p.altitude)}</td>
              <td class="data">{p.ground_speed} kt</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else}
    <!-- keyboard readers step through the track like a slider over time -->
    <div
      class="plot"
      bind:clientWidth={width}
      tabindex="0"
      role="slider"
      aria-label="Altitude profile. {summary}"
      aria-valuemin={0}
      aria-valuemax={points.length - 1}
      aria-valuenow={active ?? points.length - 1}
      aria-valuetext={reading ? `${clock(reading.timestamp)}, ${feet(reading.altitude)}` : undefined}
      onpointermove={onpointermove}
      onpointerleave={() => (active = null)}
      onfocus={() => (active = points.length - 1)}
      onblur={() => (active = null)}
      {onkeydown}
    >
      {#if drawable}
        <svg bind:this={svg} {width} height={HEIGHT} aria-hidden="true">
          {#each yTicks as tick (tick)}
            <line class="grid" x1={LEFT} x2={width - RIGHT} y1={y(tick) + 0.5} y2={y(tick) + 0.5} />
            <text class="tick" x={LEFT - 6} y={y(tick) + 4} text-anchor="end">{tick.toLocaleString()}</text>
          {/each}
          {#each xTicks as tick (tick)}
            <text class="tick" x={x(tick)} y={TOP + PLOT + 13} text-anchor="middle">{clock(tick)}</text>
          {/each}

          <path class="area" d={area} />
          <path class="line" d={line} />

          {#if hovered}
            <line class="crosshair" x1={x(hovered.timestamp)} x2={x(hovered.timestamp)} y1={TOP} y2={TOP + PLOT} />
            <circle class="dot" cx={x(hovered.timestamp)} cy={y(hovered.altitude)} r="5" />
          {:else if last}
            <circle class="dot" cx={x(last.timestamp)} cy={y(last.altitude)} r="5" />
            {#if endLabel}
              <text class="end" x={endLabel.x} y={endLabel.y} text-anchor="end">{feet(last.altitude)}</text>
            {/if}
          {/if}
        </svg>

        {#if hovered}
          <div
            class="tooltip"
            class:flip={x(hovered.timestamp) > width / 2}
            style:left="{x(hovered.timestamp)}px"
            aria-hidden="true"
          >
            <span class="value"><span class="key"></span>{feet(hovered.altitude)}</span>
            <span class="label data">{clock(hovered.timestamp)} {zone} · {hovered.ground_speed} kt</span>
          </div>
        {/if}
      {/if}
    </div>
  {/if}
</section>

<style>
  .chart {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
  }
  h3 {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-2);
  }
  h3 span {
    font-weight: 400;
    color: var(--text-3);
  }
  .tools {
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
  }
  .zone {
    font-size: var(--text-xs);
    color: var(--text-3);
  }
  .toggle {
    padding: 0;
    border: 0;
    background: none;
    font-size: var(--text-sm);
    color: var(--text-3);
    cursor: pointer;
  }
  .toggle:hover {
    color: var(--text-1);
  }
  .empty {
    font-size: var(--text-sm);
    color: var(--text-3);
  }

  .plot {
    position: relative;
    border-radius: var(--radius-sm);
    touch-action: pan-y;
  }
  svg {
    display: block;
    overflow: visible;
  }
  .grid {
    stroke: var(--line);
    stroke-width: 1;
  }
  .tick,
  .end {
    font-family: var(--font-data);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
    fill: var(--text-3);
  }
  .end {
    fill: var(--text-2);
  }
  .line {
    fill: none;
    stroke: var(--series-1);
    stroke-width: 2;
    stroke-linejoin: round;
    stroke-linecap: round;
  }
  .area {
    fill: var(--series-1);
    fill-opacity: 0.1;
  }
  .crosshair {
    stroke: var(--line-control);
    stroke-width: 1;
  }
  /* the 2px ring in the surface colour keeps the dot legible on the line */
  .dot {
    fill: var(--series-1);
    stroke: var(--surface);
    stroke-width: 2;
  }

  .tooltip {
    position: absolute;
    top: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
    padding: var(--space-1) var(--space-2);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--surface-raised);
    box-shadow: var(--shadow);
    transform: translateX(10px);
    pointer-events: none;
    white-space: nowrap;
  }
  .tooltip.flip {
    transform: translateX(calc(-100% - 10px));
  }
  .value {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    font-family: var(--font-data);
    font-size: var(--text-md);
    font-weight: 650;
    color: var(--text-1);
  }
  .key {
    width: 10px;
    height: 2px;
    border-radius: 1px;
    background: var(--series-1);
  }
  .label {
    font-size: var(--text-xs);
    color: var(--text-2);
  }

  .table {
    max-height: 180px;
    overflow-y: auto;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-sm);
  }
  th {
    position: sticky;
    top: 0;
    padding: 0 0 var(--space-1);
    background: var(--surface);
    font-weight: 600;
    color: var(--text-3);
    text-align: right;
  }
  td {
    padding: 2px 0;
    border-top: 1px solid var(--line);
    text-align: right;
  }
  th:first-child,
  td:first-child {
    text-align: left;
  }
</style>
