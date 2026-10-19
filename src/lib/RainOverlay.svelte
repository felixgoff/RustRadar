<script lang="ts">
  // Rain falling in front of the camera: streaks over the map, in three
  // depths (the nearest longest and fastest), slanting a little in the wind.
  // Drawn only while it rains; none for those who would rather not see motion.

  let {
    amount,
    pitch,
    night = 0,
  }: {
    /** How hard it rains, 0 to 1 (none: nothing drawn). */
    amount: number;
    /** The camera's tilt, degrees: looking down, the drops are seen end on, short. */
    pitch: number;
    /** 0 by day to 1 at night: the drops catch less light. */
    night?: number;
  } = $props();

  let canvas: HTMLCanvasElement | undefined = $state();
  let width = $state(0);
  let height = $state(0);

  /** Each depth: drops per million square pixels at full rain, length and speed (px, px/s), line width, opacity. */
  const DEPTHS = [
    { per: 300, length: 12, speed: 900, line: 0.8, alpha: 0.22 },
    { per: 190, length: 22, speed: 1400, line: 1, alpha: 0.3 },
    { per: 80, length: 38, speed: 2100, line: 1.4, alpha: 0.4 },
  ];
  /** Sideways per pixel down: the wind. */
  const SLANT = 0.14;

  type Drop = { x: number; y: number; k: number };

  const reduced = typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;

  $effect(() => {
    if (!canvas || reduced || amount < 0.02 || width === 0 || height === 0) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const ratio = Math.min(2, devicePixelRatio || 1);
    canvas.width = Math.round(width * ratio);
    canvas.height = Math.round(height * ratio);
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    // looking straight down rain is seen end on: short streaks
    const lengthen = 0.3 + 0.7 * Math.sin((Math.min(Math.max(pitch, 0), 85) * Math.PI) / 180);
    const drops = DEPTHS.map((d) =>
      Array.from({ length: Math.round((d.per * amount * width * height) / 1e6) }, (): Drop => ({ x: Math.random() * width, y: Math.random() * height, k: 0.75 + Math.random() * 0.5 })),
    );
    const shade = Math.round(215 - 120 * night);
    let last = performance.now();
    let frame = 0;
    const draw = (now: number) => {
      const dt = Math.min(0.05, (now - last) / 1000);
      last = now;
      ctx.clearRect(0, 0, width, height);
      DEPTHS.forEach((d, i) => {
        const length = d.length * lengthen;
        ctx.strokeStyle = `rgba(${shade}, ${shade + 8}, ${shade + 20}, ${d.alpha * (0.6 + 0.4 * amount) * (1 - 0.5 * night)})`;
        ctx.lineWidth = d.line;
        ctx.beginPath();
        for (const drop of drops[i]) {
          const fall = d.speed * drop.k * dt;
          drop.y += fall;
          drop.x += fall * SLANT;
          if (drop.y - length > height) {
            drop.y = -Math.random() * 40;
            drop.x = Math.random() * (width + height * SLANT) - height * SLANT;
          }
          ctx.moveTo(drop.x, drop.y);
          ctx.lineTo(drop.x - length * SLANT * drop.k, drop.y - length * drop.k);
        }
        ctx.stroke();
      });
      frame = requestAnimationFrame(draw);
    };
    frame = requestAnimationFrame(draw);
    return () => {
      cancelAnimationFrame(frame);
      ctx.clearRect(0, 0, width, height);
    };
  });
</script>

{#if amount >= 0.02 && !reduced}
  <canvas class="rain" bind:this={canvas} bind:clientWidth={width} bind:clientHeight={height} aria-hidden="true"></canvas>
{/if}

<style>
  .rain {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    z-index: 1;
    pointer-events: none;
  }
</style>
