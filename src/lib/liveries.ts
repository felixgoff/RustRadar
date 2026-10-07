// Airline liveries for the 3D models: a hand-made scheme for major airlines,
// and colours derived from the airline's logo for everyone else.
import { airlineColors, type LiveFlight, type LogoColors } from "./api";
import { airlineIcao, reference } from "./reference.svelte";

export type RGB = [number, number, number];
export type Part = "top" | "belly" | "wings" | "tail" | "engines";
export type Livery = Record<Part, RGB>;

export const PARTS: Part[] = ["top", "belly", "wings", "tail", "engines"];

const hex = (h: string): RGB => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16)) as RGB;

const WHITE = hex("#F5F5F3");
const WING_GREY = hex("#C3C8CF");
const ENGINE_GREY = hex("#B9BEC6");

type Spec = { tail: string; top?: string; belly?: string; wings?: string; engines?: string };

function livery({ tail, top, belly, wings, engines }: Spec): Livery {
  const t = top ? hex(top) : WHITE;
  return {
    top: t,
    belly: belly ? hex(belly) : t,
    wings: wings ? hex(wings) : WING_GREY,
    tail: hex(tail),
    engines: engines ? hex(engines) : ENGINE_GREY,
  };
}

/** Private, general aviation and unknown operators. */
export const DEFAULT_LIVERY: Livery = {
  top: WHITE,
  belly: WHITE,
  wings: WING_GREY,
  tail: WHITE,
  engines: ENGINE_GREY,
};

const MILITARY_LIVERY = livery({
  top: "#8C9298",
  tail: "#8C9298",
  wings: "#80868C",
  engines: "#7A8086",
});

/**
 * Simplified schemes for high-traffic airlines, keyed by ICAO designator:
 * the tail colour, plus the fuselage, belly and engines where they are not
 * the usual white and grey.
 */
const CURATED: Record<string, Livery> = {
  // Europe
  RYR: livery({ tail: "#073590", belly: "#073590", engines: "#073590" }),
  EZY: livery({ tail: "#FF6600", engines: "#FF6600" }),
  WZZ: livery({ tail: "#C6007E", engines: "#3E1A61" }),
  DLH: livery({ tail: "#05164D", engines: "#05164D" }),
  EWG: livery({ tail: "#A6005E" }),
  BAW: livery({ tail: "#1B2A59", belly: "#1B2A59", engines: "#1B2A59" }),
  AFR: livery({ tail: "#002157" }),
  KLM: livery({ top: "#00A1DE", belly: "#F5F5F3", tail: "#00A1DE", engines: "#00A1DE" }),
  IBE: livery({ tail: "#D7192D" }),
  VLG: livery({ tail: "#FFCC00", engines: "#57585A" }),
  SWR: livery({ tail: "#E2001A" }),
  AUA: livery({ tail: "#EE2E24" }),
  AEE: livery({ tail: "#00539F" }),
  THY: livery({ tail: "#C8102E" }),
  FIN: livery({ tail: "#0B1560" }),
  NOZ: livery({ tail: "#D81939" }),
  EIN: livery({ tail: "#006D60", engines: "#006D60" }),
  LOT: livery({ tail: "#0D2C6C" }),
  VIR: livery({ top: "#E6E7EA", tail: "#DA0530" }),
  EXS: livery({ tail: "#E2231A" }),
  AFL: livery({ top: "#E5E7EA", tail: "#0055A5" }),
  // Middle East and Africa
  UAE: livery({ tail: "#D71921" }),
  QTR: livery({ top: "#F0F0F0", belly: "#B9BBBE", tail: "#5C0632", engines: "#B9BBBE" }),
  ETD: livery({ top: "#E1D9C9", belly: "#C5B597", tail: "#B7924D", engines: "#C5B597" }),
  SVA: livery({ belly: "#D9C59B", tail: "#0F5A99" }),
  MSR: livery({ tail: "#003366" }),
  ETH: livery({ tail: "#078930" }),
  RAM: livery({ tail: "#C1272D" }),
  ELY: livery({ tail: "#0C2E66" }),
  // Americas
  DAL: livery({ belly: "#0B2A5B", tail: "#0B2A5B" }),
  AAL: livery({ top: "#C9CED4", belly: "#A9B0B8", wings: "#AEB4BA", tail: "#0B5AB0", engines: "#C9CED4" }),
  UAL: livery({ belly: "#0C2C6B", tail: "#0C2C6B", engines: "#0C2C6B" }),
  SWA: livery({ top: "#304CB2", tail: "#304CB2", engines: "#304CB2" }),
  JBU: livery({ belly: "#0033A0", tail: "#0033A0", engines: "#0033A0" }),
  ASA: livery({ tail: "#01426A" }),
  NKS: livery({ top: "#FFE600", tail: "#FFE600", engines: "#FFE600" }),
  ACA: livery({ belly: "#1A1A1A", tail: "#1A1A1A", engines: "#1A1A1A" }),
  AMX: livery({ tail: "#0B2343" }),
  LAN: livery({ tail: "#1B0088" }),
  GLO: livery({ tail: "#FF6600", engines: "#FF6600" }),
  AZU: livery({ tail: "#1E4FA3", engines: "#1E4FA3" }),
  CMP: livery({ tail: "#002F6C" }),
  // Asia-Pacific
  QFA: livery({ tail: "#E40000" }),
  VOZ: livery({ tail: "#CE0E2D" }),
  JST: livery({ tail: "#FF5C00" }),
  ANZ: livery({ tail: "#111111", engines: "#111111" }),
  SIA: livery({ tail: "#0B2363" }),
  CPA: livery({ tail: "#006564" }),
  CSN: livery({ tail: "#005BAC" }),
  CHH: livery({ tail: "#C8102E" }),
  AXM: livery({ tail: "#E31F26", engines: "#E31F26" }),
  THA: livery({ tail: "#5F2D86" }),
  GIA: livery({ tail: "#00758D" }),
  KAL: livery({ top: "#7FB8E6", belly: "#DDE0E3", tail: "#F2F2F2", engines: "#DDE0E3" }),
  JAL: livery({ tail: "#F5F5F3", engines: "#F5F5F3" }),
  IGO: livery({ belly: "#1F2A72", tail: "#1F2A72", engines: "#1F2A72" }),
  // Cargo
  FDX: livery({ belly: "#9DA3A9", tail: "#4D148C" }),
  UPS: livery({ top: "#F1EFEA", belly: "#351C15", tail: "#351C15", engines: "#351C15" }),
  DHK: livery({ top: "#FFCC00", tail: "#FFCC00", engines: "#FFCC00" }),
};

/** Group callsigns and subsidiaries that fly their parent's colours. */
const ALIASES: Record<string, string> = {
  RUK: "RYR",
  MAY: "RYR",
  EJU: "EZY",
  EZS: "EZY",
  WMT: "WZZ",
  WAZ: "WZZ",
  WUK: "WZZ",
  CLH: "DLH",
  SHT: "BAW",
  CFE: "BAW",
  EFW: "BAW",
  HOP: "AFR",
  KLC: "KLM",
  IBS: "IBE",
  NSZ: "NOZ",
  NAX: "NOZ",
  ENY: "AAL",
  PDT: "AAL",
  JIA: "AAL",
  LPE: "LAN",
  TAM: "LAN",
  QLK: "QFA",
  AIQ: "AXM",
  AWQ: "AXM",
  BCS: "DHK",
  DHX: "DHK",
};

/** Air force and other state callsigns, painted grey. */
const MILITARY = new Set(["RCH", "RRR", "GAF", "IAM", "BAF", "CTM", "CFC", "HKY", "ASY", "PAT", "CNV", "NAF"]);

function fromLogo(colors: LogoColors): Livery {
  return {
    ...DEFAULT_LIVERY,
    tail: colors.primary,
    engines: colors.secondary ?? ENGINE_GREY,
  };
}

/** The airline IATA code from a flight number such as `LH26` or `U25634`. */
function iataOfFlightNumber(flight: string): string | null {
  const m = /^([A-Z0-9]{2})\d/.exec(flight);
  return m ? m[1] : null;
}

class Liveries {
  /** Bumped whenever logo colours arrive, for deck.gl update triggers. */
  version = 0;
  private fromLogos = new Map<string, Livery | null>();
  private queued = new Set<string>();
  private inFlight = new Set<string>();
  private timer: ReturnType<typeof setTimeout> | undefined;

  forFlight(f: LiveFlight): Livery {
    const icao = airlineIcao(f.callsign);
    if (!icao) return DEFAULT_LIVERY;
    const curated = CURATED[ALIASES[icao] ?? icao];
    if (curated) return curated;
    if (MILITARY.has(icao)) return MILITARY_LIVERY;
    const iata = reference.airline(icao)?.iata || iataOfFlightNumber(f.flight);
    if (!iata) return DEFAULT_LIVERY;
    const known = this.fromLogos.get(iata);
    if (known !== undefined) return known ?? DEFAULT_LIVERY;
    this.request(iata);
    return DEFAULT_LIVERY;
  }

  private request(iata: string) {
    if (this.inFlight.has(iata) || this.queued.has(iata)) return;
    this.queued.add(iata);
    this.timer ??= setTimeout(() => this.flush(), 400);
  }

  private flush() {
    this.timer = undefined;
    const batch = [...this.queued].slice(0, 40);
    for (const code of batch) {
      this.queued.delete(code);
      this.inFlight.add(code);
    }
    airlineColors(batch)
      .then((result) => {
        for (const code of batch) {
          const colors = result[code];
          this.fromLogos.set(code, colors ? fromLogo(colors) : null);
        }
        this.version++;
      })
      .catch(() => {
        // try again on a later sighting
      })
      .finally(() => {
        for (const code of batch) this.inFlight.delete(code);
        if (this.queued.size) this.timer ??= setTimeout(() => this.flush(), 400);
      });
  }
}

export const liveries = new Liveries();
