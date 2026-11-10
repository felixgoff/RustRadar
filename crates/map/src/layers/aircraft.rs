//! Aircraft: shadows, landing gear and models (per group and part), icon outlines and icons, labels
//! (doc 03 §4–7).
//!
//! The scene builder hands over each refresh's [`AircraftScene`] (item A3) in [`Input::scene`]. The
//! layer owns the one [`assets::AircraftAssets`] (item A2) every part draws with: made in `new`,
//! polled in `prepare` (an atlas uploads once it is transcoded), and pending while an atlas the
//! scene's icons need still transcodes (a scene without icons does not wait for them). Its parts draw in the order of doc 03 §3, rows 25–29: shadows ([`shadows`], A6), gear and
//! models (A5), icon outlines and icons ([`icons`], A4), labels (A7).

pub mod assets;
pub mod icons;
pub mod shadows;

use rustradar_scene::aircraft::AircraftScene;

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// This scene refresh's aircraft (`AircraftBuilder::build`): mesh groups with their gear,
    /// icons, label candidates. Empty when no aircraft are drawn.
    pub scene: AircraftScene,
}

/// Rows 25–29: shadows, gear and models, icon outlines, icons, aircraft labels.
pub struct Aircraft {
    assets: assets::AircraftAssets,
    /// Row 25: every group's shadows, before any aircraft.
    shadows: shadows::Shadows,
    /// Rows 27 and 28: icon outlines, icons.
    icons: icons::Icons,
}

impl Aircraft {
    pub fn new(init: &Init<'_>) -> Self {
        Aircraft { assets: assets::AircraftAssets::new(init), shadows: shadows::Shadows::new(init), icons: icons::Icons::new(init) }
    }

    /// The meshes and atlases every part draws with.
    pub fn assets(&self) -> &assets::AircraftAssets {
        &self.assets
    }
}

impl Layer for Aircraft {
    fn name(&self) -> &'static str {
        "aircraft"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.assets.poll(cx.gpu);
        let scene = &cx.scene.aircraft.scene;
        self.shadows.prepare(cx, &self.assets, scene);
        self.icons.prepare(cx, &self.assets, scene);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        self.shadows.draw(pass, cx, &self.assets);
        self.icons.draw(pass, cx);
    }

    fn pending(&self) -> bool {
        self.icons.waiting(&self.assets)
    }
}
