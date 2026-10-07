// Airlines and airports, loaded once from the backend's weekly cache.
import { referenceData, type Airline, type Airport } from "./api";

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
