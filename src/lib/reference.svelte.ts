// Airlines and airports, loaded once from the backend's weekly cache, and
// runways per airport as routes need them.
import { SvelteMap } from "svelte/reactivity";
import { airportRunways, isTauri, referenceData, type Airline, type Airport, type Runway } from "./api";

class Reference {
  ready = $state(false);
  error = $state<string | null>(null);
  airlines = $state.raw<Airline[]>([]);
  airports = $state.raw<Airport[]>([]);

  private airlinesByIcao = new Map<string, Airline>();
  private airportsById = new Map<number, Airport>();
  private airportsByCode = new Map<string, Airport>();
  private loading: Promise<void> | null = null;

  load(): Promise<void> {
    this.loading ??= referenceData()
      .then((data) => {
        for (const a of data.airlines) this.airlinesByIcao.set(a.icao, a);
        for (const a of data.airports) {
          this.airportsById.set(a.id, a);
          if (a.icao) this.airportsByCode.set(a.icao, a);
          // IATA after ICAO: the two never collide in length
          if (a.iata) this.airportsByCode.set(a.iata, a);
        }
        this.airlines = data.airlines;
        this.airports = data.airports;
        this.ready = true;
      })
      .catch((e) => {
        this.error = String(e);
        this.loading = null;
      });
    return this.loading;
  }

  /** Runways by ICAO code, filled in as they are asked for; `null` while unknown or unavailable. */
  private runwaysByIcao = new SvelteMap<string, Runway[]>();
  private runwaysAsked = new Set<string>();

  /**
   * The airport's runways, or `undefined` until they have loaded (they then
   * arrive reactively, so this can be read inside `$derived`). An empty list
   * means none are known.
   */
  runways(airport: Airport | undefined): Runway[] | undefined {
    if (!airport) return undefined;
    const code = airport.icao?.toUpperCase();
    if (!code || !isTauri()) return [];
    const known = this.runwaysByIcao.get(code);
    if (known) return known;
    if (!this.runwaysAsked.has(code)) {
      this.runwaysAsked.add(code);
      airportRunways([code])
        .then((found) => this.runwaysByIcao.set(code, found[code] ?? []))
        // offline, or the download failed: ask again later
        .catch(() => setTimeout(() => this.runwaysAsked.delete(code), 60_000));
    }
    return undefined;
  }

  airline(icao: string | null | undefined): Airline | undefined {
    return icao ? this.airlinesByIcao.get(icao) : undefined;
  }

  airportById(id: number | null | undefined): Airport | undefined {
    return id ? this.airportsById.get(id) : undefined;
  }

  /** By IATA (3 letters) or ICAO (4 letters) code. */
  airport(code: string | null | undefined): Airport | undefined {
    return code ? this.airportsByCode.get(code.toUpperCase()) : undefined;
  }
}

export const reference = new Reference();

/**
 * The airline's ICAO designator from an airline-style callsign (`DLH4XK` →
 * `DLH`); private and military callsigns usually don't follow the pattern.
 */
export function airlineIcao(callsign: string): string | null {
  const m = /^([A-Z]{3})\d/.exec(callsign);
  return m ? m[1] : null;
}
