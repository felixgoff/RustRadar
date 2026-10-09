// Joining adsb.lol's aircraft to Flightradar24's flights. Flightradar24's
// anonymous feed carries no Mode S address, so the selected flight is matched
// by the address from its details when known, else by callsign or
// registration, checked against its position.
import type { AdsbAircraft } from "./api";

/** Further apart than this, a callsign or registration match is some other aircraft. */
const MAX_MATCH_KM = 150;

const normalise = (s: string) => s.toUpperCase().replace(/[^A-Z0-9]/g, "");

function km(lat1: number, lon1: number, lat2: number, lon2: number): number {
  const rad = Math.PI / 180;
  const x = ((((lon2 - lon1 + 540) % 360) - 180) * Math.cos(((lat1 + lat2) / 2) * rad));
  return Math.hypot(x, lat2 - lat1) * 111.195;
}

export interface FlightKey {
  /** Mode S address, six hex digits, if known. */
  hex: string | null;
  callsign: string;
  reg: string;
  lat: number;
  lon: number;
}

/** The aircraft in `list` that is this flight, or null. */
export function matchAdsb(list: AdsbAircraft[], key: FlightKey): AdsbAircraft | null {
  const hex = key.hex?.toLowerCase();
  if (hex) return list.find((a) => a.hex === hex) ?? null;
  const callsign = normalise(key.callsign);
  const reg = normalise(key.reg);
  let best: AdsbAircraft | null = null;
  let bestKm = MAX_MATCH_KM;
  for (const a of list) {
    const sameCallsign = callsign !== "" && normalise(a.callsign) === callsign;
    const otherReg = reg !== "" && a.reg !== "" && normalise(a.reg) !== reg;
    const sameReg = reg !== "" && normalise(a.reg) === reg;
    if (!(sameCallsign && !otherReg) && !sameReg) continue;
    // a match must also be where the flight is
    if (a.lat === undefined || a.lon === undefined || !Number.isFinite(key.lat) || !Number.isFinite(key.lon)) continue;
    const d = km(key.lat, key.lon, a.lat, a.lon);
    if (d < bestKm) {
      bestKm = d;
      best = a;
    }
  }
  return best;
}

/** Feet per minute: barometric when reported, else geometric. */
export const verticalRate = (a: AdsbAircraft) => a.baroRate ?? a.geomRate;
