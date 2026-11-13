//! Where the camera puts the map at the reference views, against deck.gl (plan item F4; the "V3 placement" of
//! the stages 8-10 status). `tests/data/deck/placement.json` is written by `scripts/port/deck-placement.mjs`,
//! which documents its fields: deck's CPU viewport at V2, along the way to V3, at V3 and on to the flat map, at
//! the reference pictures' two canvases; and where Chromium on SwiftShader, which drew `docs/port/reference/`,
//! actually draws deck's globe at V2, V3 and V7.
//!
//! The finding these tests pin: the port's camera puts every point where deck's own `viewport.project` does, to
//! far below a hundredth of a pixel, at every zoom from V2 to the flat map. Today's reference pictures of the
//! globe at V3 stand 4.7 px higher (and 0.7 px further right) than that because deck's vertex shader takes sin
//! and cos on the GPU in f32 and SwiftShader's are off by up to 2e-4; at zoom 9 that is several pixels, at V2's
//! zoom 4.5 a fraction of one, and the flat map is not affected. So the map's offset against those pictures at V3
//! is the capture's renderer, not the camera; `crates/map/tests/reference_placement.rs` shows the port's V3
//! drawn and moved by these recorded shifts matches today's pictures at 0 px.

use rustradar_scene::camera::{ViewState, viewport_for};
use serde_json::Value;

fn data() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/deck/placement.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("placement.json")).expect("placement.json parses")
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

fn pair(v: &Value) -> [f64; 2] {
    let a = v.as_array().unwrap_or_else(|| panic!("not an array: {v}"));
    [f(&a[0]), f(&a[1])]
}

fn view_state(v: &Value) -> ViewState {
    let s = &v["viewState"];
    ViewState {
        longitude: f(&s["longitude"]),
        latitude: f(&s["latitude"]),
        zoom: f(&s["zoom"]),
        bearing: f(&s["bearing"]),
        pitch: f(&s["pitch"]),
        position: None,
    }
}

/// The recorded SwiftShader view called `name` (e.g. `v3-1440x900`).
fn swiftshader<'a>(data: &'a Value, name: &str) -> &'a Value {
    data["swiftshader"]["views"].as_array().unwrap().iter().find(|v| v["name"] == name).unwrap_or_else(|| panic!("no {name}"))
}

/// V2, three steps to V3, V3 and on to zoom 12.1, at 1440 × 900 and 390 × 844: every grid point projects to deck's
/// pixel within a hundredth of a pixel (in practice within 1e-6), and its pixel back to its place.
#[test]
fn the_camera_puts_the_map_where_deck_does_from_v2_to_v3_and_the_flat_map() {
    let data = data();
    let views = data["placement"].as_array().unwrap();
    assert_eq!(views.len(), 20);
    let (mut worst_px, mut worst_deg) = (0.0f64, 0.0f64);
    for v in views {
        let name = v["name"].as_str().unwrap();
        let view = view_state(v);
        let size = [f(&v["width"]), f(&v["height"])];
        let viewport = viewport_for(&view, size);
        assert_eq!(viewport.is_globe(), v["viewport"] == "globe", "{name}: the viewport");
        let points = v["points"].as_array().unwrap();
        assert_eq!(points.len(), 81, "{name}");
        for (k, p) in points.iter().enumerate() {
            let [lng, lat] = pair(&p["in"]);
            let want = pair(&p["px"]);
            let [x, y, _] = viewport.project([lng, lat, 0.0]);
            let err = (x - want[0]).hypot(y - want[1]);
            assert!(err < 0.01, "{name} point {k}: ({x}, {y}) against deck's ({}, {}), {err} px off", want[0], want[1]);
            worst_px = worst_px.max(err);
            let back = viewport.unproject(want, 0.0);
            let err = (back[0] - lng).abs().max((back[1] - lat).abs());
            assert!(err < 1e-7, "{name} point {k}: unprojected to {back:?}, deck's ground point [{lng}, {lat}]");
            worst_deg = worst_deg.max(err);
        }
    }
    eprintln!("worst against deck: {worst_px:e} px, {worst_deg:e}° back");
    assert!(worst_px < 1e-6, "{worst_px} px");
}

/// The view's centre is the canvas's centre at V3 on both canvases (no offset, no rounding of the canvas).
#[test]
fn v3_s_centre_is_the_canvas_centre() {
    let v3 = ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, bearing: 0.0, pitch: 0.0, position: None };
    for size in [[1440.0, 900.0], [390.0, 844.0]] {
        let [x, y, _] = viewport_for(&v3, size).project([18.07, 59.33, 0.0]);
        assert!((x - size[0] / 2.0).abs() < 1e-9 && (y - size[1] / 2.0).abs() < 1e-9, "{size:?}: ({x}, {y})");
    }
}

/// What the recording says about today's pictures: Chromium on SwiftShader draws deck's globe at V3 4.3 to 5.1 px
/// higher on average (3.5 to 6 px across the canvas) on both canvases, a fraction of a pixel at V2, and deck's
/// own layer (a ScatterplotLayer) where the recorded shader puts it.
#[test]
fn swiftshader_draws_deck_s_globe_higher_at_v3_and_in_place_at_v2() {
    let data = data();
    assert!(data["swiftshader"]["renderer"].as_str().unwrap().contains("SwiftShader"));
    for size in ["1440x900", "390x844"] {
        let v3 = swiftshader(&data, &format!("v3-{size}"));
        let [dx, dy] = pair(&v3["mean"]);
        let ([_, low], [_, high]) = (pair(&v3["min"]), pair(&v3["max"]));
        assert!((-5.1..-4.3).contains(&dy) && (0.3..1.2).contains(&dx), "V3 {size}: drawn {dx}, {dy} px from deck's project");
        assert!(low > -6.5 && high < -3.0, "V3 {size}: {low} to {high} px");
        let v2 = swiftshader(&data, &format!("v2-{size}"));
        let [dx, dy] = pair(&v2["mean"]);
        assert!(dx.abs() < 0.3 && dy.abs() < 0.3, "V2 {size}: drawn {dx}, {dy} px from deck's project");
        // the grid has every point
        let [nx, ny] = pair(&v3["grid"]);
        assert_eq!(v3["shift"].as_array().unwrap().len(), (nx * ny) as usize);
    }
    for dots in data["swiftshader"]["dots"].as_array().unwrap() {
        assert!(f(&dots["worst"]) < 0.35, "{dots}");
    }
}
