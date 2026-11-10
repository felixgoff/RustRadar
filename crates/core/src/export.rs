//! Port of `export.ts` (S1.9): the recorded track of a flight as a file other tools open.
//!
//! KML plays back in Google Earth (a timed, extruded 3D track); GPX goes into mapping and GPS
//! software; CSV into a spreadsheet; GeoJSON into GIS tools and web maps. Built here; saving stays
//! in `backend::export::save` (the TypeScript's browser download branch is dropped).
//!
//! Altitudes are what Flightradar24 reports: pressure altitude in feet (not corrected for the
//! day's QNH), written in metres where a format expects them. Numbers are written as JavaScript
//! writes them (`10668`, not `10668.0`), from the doubles JavaScript read for the trail's `f32`s.

use std::io;

use serde::Serialize;
use serde_json::ser::{Formatter, PrettyFormatter, Serializer};

use crate::model::{TrailPoint, f32_as_js};
use crate::num::{fixed_number, js_round, js_string};

/// The four file formats (`export.ts:13`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportFormat {
    Kml,
    Gpx,
    Csv,
    Geojson,
}

impl ExportFormat {
    /// The name in the TypeScript, which is also the file's extension: `"kml"`, `"gpx"`, …
    pub const fn as_str(self) -> &'static str {
        match self {
            ExportFormat::Kml => "kml",
            ExportFormat::Gpx => "gpx",
            ExportFormat::Csv => "csv",
            ExportFormat::Geojson => "geojson",
        }
    }

    /// The file's media type (`export.ts:213`), for a share sheet that asks for one.
    pub const fn media_type(self) -> &'static str {
        match self {
            ExportFormat::Kml => "application/vnd.google-earth.kml+xml",
            ExportFormat::Gpx => "application/gpx+xml",
            ExportFormat::Csv => "text/csv",
            ExportFormat::Geojson => "application/geo+json",
        }
    }
}

/// A format as the export menu offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportFormatInfo {
    pub format: ExportFormat,
    pub label: &'static str,
    pub hint: &'static str,
}

/// The export menu's formats, in its order (`export.ts:15`).
pub const EXPORT_FORMATS: [ExportFormatInfo; 4] = [
    ExportFormatInfo { format: ExportFormat::Kml, label: "KML", hint: "Google Earth, with a 3D track that plays back in time" },
    ExportFormatInfo { format: ExportFormat::Gpx, label: "GPX", hint: "Mapping and GPS apps" },
    ExportFormatInfo { format: ExportFormat::Csv, label: "CSV", hint: "Spreadsheets: time, position, altitude, speed, track" },
    ExportFormatInfo { format: ExportFormat::Geojson, label: "GeoJSON", hint: "GIS tools and web maps" },
];

/// What the file says about the flight (`export.ts:23`). Empty strings are left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportMeta {
    pub callsign: String,
    pub flight_number: String,
    pub registration: String,
    pub aircraft_type: String,
    pub origin: String,
    pub destination: String,
}

const FT_TO_M: f64 = 0.3048;

const NOTE: &str = "Altitude is pressure altitude as reported. Exported from RustRadar; data from Flightradar24.";

/// The trail in time order, without points that carry no time or position (`export.ts:35`).
/// The sort is stable, as `Array.prototype.sort` is.
pub fn export_points(trail: &[TrailPoint]) -> Vec<TrailPoint> {
    let mut points: Vec<TrailPoint> =
        trail.iter().filter(|p| p.timestamp > 0 && p.latitude.is_finite() && p.longitude.is_finite()).copied().collect();
    points.sort_by_key(|p| p.timestamp);
    points
}

/// The proleptic Gregorian date of a day counted from 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `new Date(unix * 1000).toISOString()` without the milliseconds: `2026-10-12T16:00:00Z`, and
/// `+010000-01-01T00:00:00Z` from the year 10000. (JavaScript throws past +275760-09-13; no
/// trail gets there.)
fn iso(unix: u64) -> String {
    let (days, secs) = (unix / 86_400, unix % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    let year = if year <= 9999 { format!("{year:04}") } else { format!("+{year:06}") };
    format!("{year}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

/// `Math.round(Math.max(0, feet) * 0.3048 * 10) / 10`: metres to a decimetre, none below zero.
fn metres(feet: i32) -> f64 {
    js_round(f64::from(feet.max(0)) * FT_TO_M * 10.0) / 10.0
}

/// `Number(x.toFixed(6))` of a coordinate: six decimals at most, as JavaScript prints it (`4.5`).
fn coord(x: f32) -> f64 {
    fixed_number(f32_as_js(x), 6)
}

/// `export.ts` `xml`: `& < > " '` escaped.
fn xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// `a || b` for strings.
fn or<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() { b } else { a }
}

/// "BAW117 (BA117) LHR–JFK" from whatever is known (`export.ts:51`).
pub fn export_title(meta: &ExportMeta) -> String {
    let id = or(or(or(&meta.callsign, &meta.flight_number), &meta.registration), "Flight");
    let number = if !meta.flight_number.is_empty() && meta.flight_number != id {
        format!(" ({})", meta.flight_number)
    } else {
        String::new()
    };
    let route = if !meta.origin.is_empty() || !meta.destination.is_empty() {
        format!(" {}–{}", or(&meta.origin, "?"), or(&meta.destination, "?"))
    } else {
        String::new()
    };
    format!("{id}{number}{route}")
}

fn description(meta: &ExportMeta, points: &[TrailPoint]) -> String {
    let mut parts = Vec::new();
    if !meta.aircraft_type.is_empty() {
        parts.push(format!("Aircraft {}", meta.aircraft_type));
    }
    if !meta.registration.is_empty() {
        parts.push(format!("registration {}", meta.registration));
    }
    if let (Some(first), Some(last)) = (points.first(), points.last()) {
        parts.push(format!("{} positions, {} to {}", points.len(), iso(first.timestamp), iso(last.timestamp)));
    }
    parts.push(NOTE.to_owned());
    parts.join(". ")
}

/// File name for the export (`export.ts:69`): "BAW117 LHR-JFK 2026-10-12.kml".
pub fn export_file_name(meta: &ExportMeta, points: &[TrailPoint], format: ExportFormat) -> String {
    let id = or(or(or(&meta.callsign, &meta.flight_number), &meta.registration), "flight");
    let route = if !meta.origin.is_empty() && !meta.destination.is_empty() {
        format!(" {}-{}", meta.origin, meta.destination)
    } else {
        String::new()
    };
    let day = match points.first() {
        Some(first) => format!(" {}", &iso(first.timestamp)[..10]),
        None => String::new(),
    };
    format!("{id}{route}{day}.{}", format.as_str())
}

fn lines(points: &[TrailPoint], line: impl Fn(&TrailPoint) -> String) -> String {
    points.iter().map(line).collect::<Vec<_>>().join("\n")
}

fn kml(points: &[TrailPoint], meta: &ExportMeta) -> String {
    let title = xml(&export_title(meta));
    let description = xml(&description(meta, points));
    let when = lines(points, |p| format!("        <when>{}</when>", iso(p.timestamp)));
    let coords = lines(points, |p| {
        let (lon, lat, alt) = (js_string(coord(p.longitude)), js_string(coord(p.latitude)), js_string(metres(p.altitude)));
        format!("        <gx:coord>{lon} {lat} {alt}</gx:coord>")
    });
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://www.opengis.net/kml/2.2" xmlns:gx="http://www.google.com/kml/ext/2.2">
  <Document>
    <name>{title}</name>
    <description>{description}</description>
    <Style id="track">
      <LineStyle><color>ff34a6f0</color><width>3</width></LineStyle>
      <PolyStyle><color>4034a6f0</color></PolyStyle>
      <IconStyle><scale>0.8</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/airports.png</href></Icon></IconStyle>
    </Style>
    <Placemark>
      <name>{title}</name>
      <styleUrl>#track</styleUrl>
      <gx:Track>
        <extrude>1</extrude>
        <altitudeMode>absolute</altitudeMode>
{when}
{coords}
      </gx:Track>
    </Placemark>
  </Document>
</kml>
"##
    )
}

fn gpx(points: &[TrailPoint], meta: &ExportMeta) -> String {
    let title = xml(&export_title(meta));
    let description = xml(&description(meta, points));
    let pts = lines(points, |p| {
        let (lat, lon, ele) = (js_string(coord(p.latitude)), js_string(coord(p.longitude)), js_string(metres(p.altitude)));
        format!("      <trkpt lat=\"{lat}\" lon=\"{lon}\"><ele>{ele}</ele><time>{}</time></trkpt>", iso(p.timestamp))
    });
    let time = match points.first() {
        Some(first) => format!("    <time>{}</time>\n", iso(first.timestamp)),
        None => String::new(),
    };
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="RustRadar" xmlns="http://www.topografix.com/GPX/1/1">
  <metadata>
    <name>{title}</name>
    <desc>{description}</desc>
{time}  </metadata>
  <trk>
    <name>{title}</name>
    <trkseg>
{pts}
    </trkseg>
  </trk>
</gpx>
"##
    )
}

/// `export.ts` `csvField`: quoted, quotes doubled, only when it holds `"`, `,` or a newline.
fn csv_field(s: &str) -> String {
    if s.contains(['"', ',', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_owned() }
}

fn csv(points: &[TrailPoint], meta: &ExportMeta) -> String {
    let header = [
        "time_utc",
        "unix_time",
        "latitude",
        "longitude",
        "altitude_ft",
        "ground_speed_kt",
        "track_deg",
        "vertical_speed_fpm",
        "callsign",
    ]
    .join(",");
    let callsign = csv_field(&meta.callsign);
    let mut rows = vec![header];
    for p in points {
        // altitude, speed, track and vertical speed are whole numbers already: Math.round keeps them
        rows.push(
            [
                iso(p.timestamp),
                p.timestamp.to_string(),
                js_string(coord(p.latitude)),
                js_string(coord(p.longitude)),
                p.altitude.to_string(),
                p.ground_speed.to_string(),
                p.track.to_string(),
                p.vertical_speed.to_string(),
                callsign.clone(),
            ]
            .join(","),
        );
    }
    rows.join("\n") + "\n"
}

/// `JSON.stringify`'s layout with an indent of two (serde_json's pretty printer) and its numbers
/// (`js_string`: `10668`, where serde_json writes `10668.0`).
struct JsFormatter<'a>(PrettyFormatter<'a>);

impl Formatter for JsFormatter<'_> {
    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        // serde_json writes a NaN or an infinity as `null` before it gets here, as JSON.stringify does
        writer.write_all(js_string(value).as_bytes())
    }

    fn begin_array<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_array(writer)
    }

    fn end_array<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_array(writer)
    }

    fn begin_array_value<W: ?Sized + io::Write>(&mut self, writer: &mut W, first: bool) -> io::Result<()> {
        self.0.begin_array_value(writer, first)
    }

    fn end_array_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_array_value(writer)
    }

    fn begin_object<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_object(writer)
    }

    fn end_object<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object(writer)
    }

    fn begin_object_key<W: ?Sized + io::Write>(&mut self, writer: &mut W, first: bool) -> io::Result<()> {
        self.0.begin_object_key(writer, first)
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_object_value(writer)
    }

    fn end_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object_value(writer)
    }
}

/// `JSON.stringify(value, null, 2)`: two-space indents, and numbers as JavaScript writes them
/// (`10668`, `0.3`, `1e-7`; NaN and the infinities as `null`). Objects keep the order of the
/// struct's fields (or of an `IndexMap`), as JavaScript keeps the order of an object's keys.
///
/// # Panics
///
/// When `value`'s `Serialize` fails, which the plain data written here never does (a map with
/// keys that are not strings would).
pub fn stringify_pretty<T: Serialize + ?Sized>(value: &T) -> String {
    let mut out = Vec::new();
    let mut serializer = Serializer::with_formatter(&mut out, JsFormatter(PrettyFormatter::new()));
    value.serialize(&mut serializer).expect("plain data always serialises");
    String::from_utf8(out).expect("serde_json writes UTF-8")
}

/// `meta.field || null`.
fn or_null(s: &str) -> Option<&str> {
    if s.is_empty() { None } else { Some(s) }
}

fn geojson(points: &[TrailPoint], meta: &ExportMeta) -> String {
    #[derive(Serialize)]
    struct Collection<'a> {
        #[serde(rename = "type")]
        kind: &'static str,
        features: [Feature<'a>; 1],
    }
    #[derive(Serialize)]
    struct Feature<'a> {
        #[serde(rename = "type")]
        kind: &'static str,
        geometry: Geometry,
        properties: Properties<'a>,
    }
    #[derive(Serialize)]
    struct Geometry {
        #[serde(rename = "type")]
        kind: &'static str,
        coordinates: Vec<[f64; 3]>,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Properties<'a> {
        name: String,
        callsign: Option<&'a str>,
        flight_number: Option<&'a str>,
        registration: Option<&'a str>,
        aircraft_type: Option<&'a str>,
        origin: Option<&'a str>,
        destination: Option<&'a str>,
        altitude: &'static str,
        times: Vec<String>,
        ground_speed_kt: Vec<u32>,
        source: &'static str,
    }
    let feature = Feature {
        kind: "Feature",
        geometry: Geometry {
            kind: "LineString",
            coordinates: points.iter().map(|p| [coord(p.longitude), coord(p.latitude), metres(p.altitude)]).collect(),
        },
        properties: Properties {
            name: export_title(meta),
            callsign: or_null(&meta.callsign),
            flight_number: or_null(&meta.flight_number),
            registration: or_null(&meta.registration),
            aircraft_type: or_null(&meta.aircraft_type),
            origin: or_null(&meta.origin),
            destination: or_null(&meta.destination),
            altitude: "pressure altitude in metres, as reported",
            times: points.iter().map(|p| iso(p.timestamp)).collect(),
            // whole knots already: Math.round keeps them
            ground_speed_kt: points.iter().map(|p| p.ground_speed).collect(),
            source: "Flightradar24, via RustRadar",
        },
    };
    stringify_pretty(&Collection { kind: "FeatureCollection", features: [feature] }) + "\n"
}

/// The file's contents in `format` (`export.ts:188`), from the trail as [`export_points`] keeps it.
pub fn export_path(trail: &[TrailPoint], meta: &ExportMeta, format: ExportFormat) -> String {
    let points = export_points(trail);
    match format {
        ExportFormat::Kml => kml(&points, meta),
        ExportFormat::Gpx => gpx(&points, meta),
        ExportFormat::Csv => csv(&points, meta),
        ExportFormat::Geojson => geojson(&points, meta),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn point(timestamp: u64, latitude: f32, longitude: f32, altitude: i32) -> TrailPoint {
        TrailPoint { timestamp, latitude, longitude, altitude, ground_speed: 450, track: 270, vertical_speed: 0, source: 0 }
    }

    // 2026-10-12 16:00:00Z onwards, deliberately out of order with a bad point
    fn trail() -> Vec<TrailPoint> {
        vec![
            point(1_791_820_860, 51.5, -0.6, 3_000),
            point(1_791_820_800, 51.47, -0.45, 0),
            point(0, 0.0, 0.0, 0),
            point(1_791_820_920, 51.52, -0.8, 10_000),
        ]
    }

    fn meta() -> ExportMeta {
        ExportMeta {
            callsign: "BAW117".to_owned(),
            flight_number: "BA117".to_owned(),
            registration: "G-VIIL".to_owned(),
            aircraft_type: "B772".to_owned(),
            origin: "LHR".to_owned(),
            destination: "JFK".to_owned(),
        }
    }

    #[test]
    fn points_are_in_time_order_and_timeless_points_are_dropped() {
        let times: Vec<u64> = export_points(&trail()).iter().map(|p| p.timestamp).collect();
        assert_eq!(times, [1_791_820_800, 1_791_820_860, 1_791_820_920]);
    }

    #[test]
    fn titles_and_file_names_say_which_flight_it_is() {
        assert_eq!(export_title(&meta()), "BAW117 (BA117) LHR–JFK");
        let bare = ExportMeta { callsign: String::new(), flight_number: String::new(), origin: String::new(), destination: String::new(), ..meta() };
        assert_eq!(export_title(&bare), "G-VIIL");
        assert_eq!(export_file_name(&meta(), &export_points(&trail()), ExportFormat::Kml), "BAW117 LHR-JFK 2026-10-12.kml");
        assert_eq!(export_file_name(&ExportMeta { callsign: String::new(), ..meta() }, &[], ExportFormat::Csv), "BA117 LHR-JFK.csv");
    }

    #[test]
    fn kml_is_a_timed_extruded_track_in_metres() {
        let kml = export_path(&trail(), &meta(), ExportFormat::Kml);
        assert!(kml.contains("<gx:Track>"));
        assert!(kml.contains("<altitudeMode>absolute</altitudeMode>"));
        assert!(kml.contains("<when>2026-10-12T16:00:00Z</when>"));
        // 10,000 ft
        assert!(kml.contains("<gx:coord>-0.8 51.52 3048</gx:coord>"));
        assert!(!kml.contains("<when>1970"));
    }

    #[test]
    fn gpx_has_one_track_point_per_position_with_height_and_time() {
        let gpx = export_path(&trail(), &meta(), ExportFormat::Gpx);
        assert_eq!(gpx.matches("<trkpt ").count(), 3);
        assert!(gpx.contains(r#"<trkpt lat="51.47" lon="-0.45"><ele>0</ele><time>2026-10-12T16:00:00Z</time></trkpt>"#));
    }

    #[test]
    fn csv_has_a_header_and_a_row_per_position() {
        let csv = export_path(&trail(), &meta(), ExportFormat::Csv);
        let lines: Vec<&str> = csv.trim().split('\n').collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "time_utc,unix_time,latitude,longitude,altitude_ft,ground_speed_kt,track_deg,vertical_speed_fpm,callsign");
        assert_eq!(lines[3], "2026-10-12T16:02:00Z,1791820920,51.52,-0.8,10000,450,270,0,BAW117");
    }

    #[test]
    fn geojson_is_a_line_with_a_time_per_vertex() {
        let json: Value = serde_json::from_str(&export_path(&trail(), &meta(), ExportFormat::Geojson)).unwrap();
        let feature = &json["features"][0];
        assert_eq!(feature["geometry"]["type"], "LineString");
        assert_eq!(feature["geometry"]["coordinates"][1], json!([-0.6, 51.5, 914.4]));
        assert_eq!(feature["properties"]["times"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn names_with_markup_are_escaped_in_xml() {
        let kml = export_path(&trail(), &ExportMeta { callsign: "A&B<1>".to_owned(), ..meta() }, ExportFormat::Kml);
        assert!(kml.contains("A&amp;B&lt;1&gt;"));
        assert!(!kml.contains("A&B<1>"));
    }

    #[test]
    fn whole_numbers_are_written_as_javascript_writes_them() {
        let at_35000 = [point(1_791_830_000, 59.0, 18.0, 35_000)];
        let json = export_path(&at_35000, &meta(), ExportFormat::Geojson);
        assert!(json.contains("            18,\n            59,\n            10668\n"), "{json}");
        assert!(!json.contains(".0,") && !json.contains(".0\n"));
        assert_eq!(stringify_pretty(&json!({ "a": [1.0, -0.0, 0.5, 1e21, 1e-7] })), "{\n  \"a\": [\n    1,\n    0,\n    0.5,\n    1e+21,\n    1e-7\n  ]\n}");
        assert_eq!(stringify_pretty(&[f64::NAN, f64::INFINITY]), "[\n  null,\n  null\n]");
        assert_eq!(stringify_pretty(&Vec::<f64>::new()), "[]");
    }

    #[test]
    fn iso_writes_dates_as_to_iso_string_does() {
        // new Date(s * 1000).toISOString(), less ".000", from node
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_791_820_800), "2026-10-12T16:00:00Z");
        assert_eq!(iso(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso(253_402_300_799), "9999-12-31T23:59:59Z");
        assert_eq!(iso(253_402_300_800), "+010000-01-01T00:00:00Z");
        assert_eq!(iso(8_640_000_000_000), "+275760-09-13T00:00:00Z");
    }

    #[test]
    fn the_formats_are_named_as_in_export_ts() {
        let names: Vec<_> = EXPORT_FORMATS.iter().map(|f| (f.format.as_str(), f.label)).collect();
        assert_eq!(names, [("kml", "KML"), ("gpx", "GPX"), ("csv", "CSV"), ("geojson", "GeoJSON")]);
        assert_eq!(EXPORT_FORMATS[2].hint, "Spreadsheets: time, position, altitude, speed, track");
        assert_eq!(ExportFormat::Geojson.media_type(), "application/geo+json");
    }

    // exportPath's output, printed by bun from export.ts: a trail with rounding ties, a point
    // below zero, a callsign that needs quoting, and one end of the route
    fn odd() -> (Vec<TrailPoint>, ExportMeta) {
        let at = |timestamp, latitude, longitude, altitude, ground_speed, track, vertical_speed| TrailPoint {
            timestamp,
            latitude,
            longitude,
            altitude,
            ground_speed,
            track,
            vertical_speed,
            source: 0,
        };
        let trail = vec![
            at(1_791_830_000, 59.12346, -0.4512345, 35_000, 0, 0, -1500),
            at(1_791_830_010, -33.94612, 151.1772, -200, 12, 359, 64),
            at(1_791_830_020, 0.0000005, -179.99998, 37_000, 450, 270, 2048),
            at(1_791_830_005, 1e-7, 0.1234565, 1, 450, 270, -1),
        ];
        let meta = ExportMeta { callsign: "BA\"1,2".to_owned(), destination: "SYD".to_owned(), ..ExportMeta::default() };
        (trail, meta)
    }

    #[test]
    fn every_format_is_written_byte_for_byte_as_export_ts_writes_it() {
        let (trail, meta) = odd();
        // the doubles the TypeScript was given are what JavaScript reads for these f32s
        let read: Vec<[f64; 2]> = trail.iter().map(|p| [f32_as_js(p.latitude), f32_as_js(p.longitude)]).collect();
        assert_eq!(read, [[59.12346, -0.4512345], [-33.94612, 151.1772], [0.0000005, -179.99998], [1e-7, 0.1234565]]);
        let formats = [ExportFormat::Kml, ExportFormat::Gpx, ExportFormat::Csv, ExportFormat::Geojson];
        for (format, want) in formats.into_iter().zip([ODD_KML, ODD_GPX, ODD_CSV, ODD_GEOJSON]) {
            assert_eq!(export_path(&trail, &meta, format), want, "{format:?}");
        }
        for (format, want) in formats.into_iter().zip([EMPTY_KML, EMPTY_GPX, EMPTY_CSV, EMPTY_GEOJSON]) {
            assert_eq!(export_path(&[], &ExportMeta::default(), format), want, "{format:?}");
        }
        assert_eq!(export_title(&meta), "BA\"1,2 ?–SYD");
        assert_eq!(export_file_name(&meta, &export_points(&trail), ExportFormat::Gpx), "BA\"1,2 2026-10-12.gpx");
    }

    const ODD_KML: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://www.opengis.net/kml/2.2" xmlns:gx="http://www.google.com/kml/ext/2.2">
  <Document>
    <name>BA&quot;1,2 ?–SYD</name>
    <description>4 positions, 2026-10-12T18:33:20Z to 2026-10-12T18:33:40Z. Altitude is pressure altitude as reported. Exported from RustRadar; data from Flightradar24.</description>
    <Style id="track">
      <LineStyle><color>ff34a6f0</color><width>3</width></LineStyle>
      <PolyStyle><color>4034a6f0</color></PolyStyle>
      <IconStyle><scale>0.8</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/airports.png</href></Icon></IconStyle>
    </Style>
    <Placemark>
      <name>BA&quot;1,2 ?–SYD</name>
      <styleUrl>#track</styleUrl>
      <gx:Track>
        <extrude>1</extrude>
        <altitudeMode>absolute</altitudeMode>
        <when>2026-10-12T18:33:20Z</when>
        <when>2026-10-12T18:33:25Z</when>
        <when>2026-10-12T18:33:30Z</when>
        <when>2026-10-12T18:33:40Z</when>
        <gx:coord>-0.451234 59.12346 10668</gx:coord>
        <gx:coord>0.123456 0 0.3</gx:coord>
        <gx:coord>151.1772 -33.94612 0</gx:coord>
        <gx:coord>-179.99998 0 11277.6</gx:coord>
      </gx:Track>
    </Placemark>
  </Document>
</kml>
"##;

    const ODD_GPX: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="RustRadar" xmlns="http://www.topografix.com/GPX/1/1">
  <metadata>
    <name>BA&quot;1,2 ?–SYD</name>
    <desc>4 positions, 2026-10-12T18:33:20Z to 2026-10-12T18:33:40Z. Altitude is pressure altitude as reported. Exported from RustRadar; data from Flightradar24.</desc>
    <time>2026-10-12T18:33:20Z</time>
  </metadata>
  <trk>
    <name>BA&quot;1,2 ?–SYD</name>
    <trkseg>
      <trkpt lat="59.12346" lon="-0.451234"><ele>10668</ele><time>2026-10-12T18:33:20Z</time></trkpt>
      <trkpt lat="0" lon="0.123456"><ele>0.3</ele><time>2026-10-12T18:33:25Z</time></trkpt>
      <trkpt lat="-33.94612" lon="151.1772"><ele>0</ele><time>2026-10-12T18:33:30Z</time></trkpt>
      <trkpt lat="0" lon="-179.99998"><ele>11277.6</ele><time>2026-10-12T18:33:40Z</time></trkpt>
    </trkseg>
  </trk>
</gpx>
"##;

    const ODD_CSV: &str = r##"time_utc,unix_time,latitude,longitude,altitude_ft,ground_speed_kt,track_deg,vertical_speed_fpm,callsign
2026-10-12T18:33:20Z,1791830000,59.12346,-0.451234,35000,0,0,-1500,"BA""1,2"
2026-10-12T18:33:25Z,1791830005,0,0.123456,1,450,270,-1,"BA""1,2"
2026-10-12T18:33:30Z,1791830010,-33.94612,151.1772,-200,12,359,64,"BA""1,2"
2026-10-12T18:33:40Z,1791830020,0,-179.99998,37000,450,270,2048,"BA""1,2"
"##;

    const ODD_GEOJSON: &str = r##"{
  "type": "FeatureCollection",
  "features": [
    {
      "type": "Feature",
      "geometry": {
        "type": "LineString",
        "coordinates": [
          [
            -0.451234,
            59.12346,
            10668
          ],
          [
            0.123456,
            0,
            0.3
          ],
          [
            151.1772,
            -33.94612,
            0
          ],
          [
            -179.99998,
            0,
            11277.6
          ]
        ]
      },
      "properties": {
        "name": "BA\"1,2 ?–SYD",
        "callsign": "BA\"1,2",
        "flightNumber": null,
        "registration": null,
        "aircraftType": null,
        "origin": null,
        "destination": "SYD",
        "altitude": "pressure altitude in metres, as reported",
        "times": [
          "2026-10-12T18:33:20Z",
          "2026-10-12T18:33:25Z",
          "2026-10-12T18:33:30Z",
          "2026-10-12T18:33:40Z"
        ],
        "groundSpeedKt": [
          0,
          450,
          12,
          450
        ],
        "source": "Flightradar24, via RustRadar"
      }
    }
  ]
}
"##;

    const EMPTY_KML: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://www.opengis.net/kml/2.2" xmlns:gx="http://www.google.com/kml/ext/2.2">
  <Document>
    <name>Flight</name>
    <description>Altitude is pressure altitude as reported. Exported from RustRadar; data from Flightradar24.</description>
    <Style id="track">
      <LineStyle><color>ff34a6f0</color><width>3</width></LineStyle>
      <PolyStyle><color>4034a6f0</color></PolyStyle>
      <IconStyle><scale>0.8</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/airports.png</href></Icon></IconStyle>
    </Style>
    <Placemark>
      <name>Flight</name>
      <styleUrl>#track</styleUrl>
      <gx:Track>
        <extrude>1</extrude>
        <altitudeMode>absolute</altitudeMode>


      </gx:Track>
    </Placemark>
  </Document>
</kml>
"##;

    const EMPTY_GPX: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="RustRadar" xmlns="http://www.topografix.com/GPX/1/1">
  <metadata>
    <name>Flight</name>
    <desc>Altitude is pressure altitude as reported. Exported from RustRadar; data from Flightradar24.</desc>
  </metadata>
  <trk>
    <name>Flight</name>
    <trkseg>

    </trkseg>
  </trk>
</gpx>
"##;

    const EMPTY_CSV: &str = r##"time_utc,unix_time,latitude,longitude,altitude_ft,ground_speed_kt,track_deg,vertical_speed_fpm,callsign
"##;

    const EMPTY_GEOJSON: &str = r##"{
  "type": "FeatureCollection",
  "features": [
    {
      "type": "Feature",
      "geometry": {
        "type": "LineString",
        "coordinates": []
      },
      "properties": {
        "name": "Flight",
        "callsign": null,
        "flightNumber": null,
        "registration": null,
        "aircraftType": null,
        "origin": null,
        "destination": null,
        "altitude": "pressure altitude in metres, as reported",
        "times": [],
        "groundSpeedKt": [],
        "source": "Flightradar24, via RustRadar"
      }
    }
  ]
}
"##;
}
