//! Aircraft: shadows, landing gear and models (per group and part), icon outlines and icons, labels
//! (doc 03 §4–7).
//!
//! The scene builder hands over each refresh's [`AircraftScene`] (item A3) in [`Input::scene`]. The
//! layer owns the one [`assets::AircraftAssets`] (item A2) every part draws with: made in `new`,
//! polled in `prepare` (an atlas uploads once it is transcoded), and pending while an atlas the
//! scene's icons need still transcodes (a scene without icons does not wait for them). Its parts draw in the order of doc 03 §3, rows 25–29: shadows ([`shadows`], A6), gear and
//! models ([`meshes`], A5), icon outlines and icons ([`icons`], A4), labels (A7).

pub mod assets;
pub mod icons;
pub mod labels;
pub mod meshes;
pub mod shadows;

use rustradar_scene::aircraft::AircraftScene;

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// This scene refresh's aircraft (`AircraftBuilder::build`): mesh groups with their gear,
    /// icons, label candidates. Empty when no aircraft are drawn.
    pub scene: AircraftScene,
    /// This scene refresh's aircraft labels (`aircraft_labels::Labeller::build`, item A7): empty
    /// when none are drawn.
    pub labels: labels::Input,
}

/// Rows 25–29: shadows, gear and models, icon outlines, icons, aircraft labels.
pub struct Aircraft {
    assets: assets::AircraftAssets,
    /// Row 25: every group's shadows, before any aircraft.
    shadows: shadows::Shadows,
    /// Row 26: every group's gear and airframes.
    meshes: meshes::Meshes,
    /// Rows 27 and 28: icon outlines, icons.
    icons: icons::Icons,
    /// Row 29: aircraft labels.
    labels: labels::Labels,
}

impl Aircraft {
    pub fn new(init: &Init<'_>) -> Self {
        Aircraft {
            assets: assets::AircraftAssets::new(init),
            shadows: shadows::Shadows::new(init),
            meshes: meshes::Meshes::new(init),
            icons: icons::Icons::new(init),
            labels: labels::Labels::new(init),
        }
    }

    /// The aircraft labels drawn this frame.
    pub fn labels(&self) -> &[labels::Shown] {
        self.labels.shown()
    }

    /// The gear and airframes drawn this frame.
    pub fn meshes(&self) -> &meshes::Meshes {
        &self.meshes
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
        self.meshes.prepare(cx, &self.assets, scene);
        self.icons.prepare(cx, &self.assets, scene);
        self.labels.prepare(cx, &cx.scene.aircraft.labels);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        self.shadows.draw(pass, cx, &self.assets);
        self.meshes.draw(pass, cx, &self.assets);
        self.icons.draw(pass, cx);
        self.labels.draw(pass, cx);
    }

    fn pending(&self) -> bool {
        self.icons.waiting(&self.assets)
    }
}
