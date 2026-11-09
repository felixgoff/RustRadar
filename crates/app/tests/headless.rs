//! Renders a frame offscreen and checks that the map and the UI both drew.
//! Needs any wgpu adapter; in CI without a GPU use SwiftShader or lavapipe via `VK_ICD_FILENAMES`.

use rustradar_app::gpu::GpuError;
use rustradar_app::headless;

#[test]
fn frame_is_not_uniform() {
    let (width, height) = (640, 400);
    let frame = match headless::render_frame(width, height, 1.0) {
        Ok(frame) => frame,
        Err(GpuError::NoAdapter(e)) => {
            eprintln!("skipped: {e}");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    eprintln!("rendered with {}", frame.adapter);
    assert_eq!(frame.rgba.len(), (width * height * 4) as usize);

    let pixel = |x: u32, y: u32| {
        let i = ((y * width + x) * 4) as usize;
        [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2], frame.rgba[i + 3]]
    };
    // Bottom-right corner: nothing but the clear colour, #1c1c1c.
    assert_eq!(pixel(width - 1, height - 1), [0x1c, 0x1c, 0x1c, 0xff]);
    // Centre: the globe, which is blue.
    let [r, _, b, _] = pixel(width / 2, height / 2);
    assert!(b > r + 20, "no globe at the centre: {:?}", pixel(width / 2, height / 2));
    // Inside the panel (16px margin, 12px padding): the panel fill or text, never the canvas or globe.
    let panel = pixel(300, 24);
    assert_eq!(panel, [0x32, 0x32, 0x32, 0xff], "no UI panel");
    // Text was rasterised: some near-white pixels in the title row.
    let bright = (20..300).flat_map(|x| (20..50).map(move |y| (x, y))).filter(|&(x, y)| pixel(x, y)[0] > 0xc0).count();
    assert!(bright > 20, "no text in the panel ({bright} bright pixels)");

    let mut colors = std::collections::HashSet::new();
    for px in frame.rgba.chunks_exact(4) {
        colors.insert([px[0], px[1], px[2]]);
    }
    assert!(colors.len() > 50, "image is nearly uniform: {} colours", colors.len());
}
