//!include lib/project.wgsl
// earth.wgsl: the earth sphere (doc 01 §5.4; `layers/earth.rs`), deck's SimpleMeshLayer of a luma.gl
// SphereGeometry in CARTESIAN coordinates, unlit (`material: false`), in one colour. It reaches the
// poles, where local Mercator cannot go, so its vertices are unit directions in deck's common space
// (the sphere's centre at the origin), scaled here to its radius and placed as a common offset:
// `earth.centre` is the origin minus the view centre the offsets are measured from, worked out on
// the CPU in f64. Globe only.

// `earth::EarthUniform`.
struct Earth {
    // the sphere's centre as a common offset
    centre: vec3<f32>,
    // its radius in common units (6.3e6 m below deck's 6 370 972 m surface of 256 units)
    radius: f32,
    // its colour, RGBA 0–1
    color: vec4<f32>,
}

@group(2) @binding(0) var<uniform> earth: Earth;

@vertex
fn vs(@location(0) direction: vec3<f32>) -> @builtin(position) vec4<f32> {
    return project_common(direction * earth.radius + earth.centre);
}

@fragment
fn fs() -> @location(0) vec4<f32> {
    return earth.color;
}
