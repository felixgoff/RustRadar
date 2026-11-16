//! The bridges' structures on the realistic map's flat ground (`rustradar_scene::bridges`): each
//! tile's slabs, parapets and piers as one lit mesh in the tile's frame, drawn with the mesh
//! pipeline's local entry after the tile's ground lines and before its raised deck surfaces,
//! inside the tile's square (the stencil), depth written so what passes under a deck is hidden
//! by it; and each deck's shadow on the ground, cast away from the key light (the same light that
//! shades the structures) and worked out again on the CPU only when that light turns. Only in the
//! map pass without the 3D terrain: on the terrain the decks' lines are draped into its covers and
//! the bridges lie on the ground with them.

use rustradar_scene::bridges::Structures;

use crate::frame::FrameSlot;
use crate::layers::vector_map::CLIPPED;
use crate::layers::{DrawCx, Init, Pass, PrepareCx};
use crate::pipelines::fill::{self, Fill};
use crate::pipelines::mesh::{Material, MeshPipeline, Shape, Style, StyleUniform, Vertex};
use crate::state::DrawState;

/// Inside the tile's square, unculled, depth written.
pub const STRUCTURES: DrawState = DrawState { depth_write: true, cull: None, ..CLIPPED };

/// The shadows' colour (the building shadows' blue-black), its alpha their strength.
const SHADOW: [u8; 4] = [6, 12, 28, 84];
/// No shadow from a light this close to the horizon (its up component), nor longer than this many
/// times the deck's height.
const LOW_LIGHT: f64 = 0.15;
const LONGEST: f64 = 3.0;

/// A tile's bridges on the GPU: the structures, and the decks' outlines with their shadow for the
/// light it was last cast from.
pub struct TileBridges {
    shape: Shape,
    decks: Vec<Vec<[[f32; 3]; 2]>>,
    metres_per_unit: f64,
    shadow: Option<([f32; 3], fill::Mesh<fill::Vertex>)>,
}

/// A tile's bridges on the GPU, `None` for a tile without.
pub fn upload(device: &wgpu::Device, structures: &Structures) -> Option<TileBridges> {
    (!structures.is_empty()).then(|| {
        let vertices: &[Vertex] = bytemuck::cast_slice(&structures.vertices);
        TileBridges {
            shape: Shape::new(device, "bridge structures", vertices, Some(&structures.indices)),
            decks: structures.decks.clone(),
            metres_per_unit: structures.metres_per_unit,
            shadow: None,
        }
    })
}

/// The decks' shadows on the ground, cast toward `-light` (the key light's direction, east, north,
/// up): each deck's outline moved away from the light by its height, as strips of quads.
fn shadow_mesh(device: &wgpu::Device, b: &TileBridges, light: [f64; 3]) -> fill::Mesh<fill::Vertex> {
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
    if -light[2] > LOW_LIGHT {
        let m = b.metres_per_unit;
        // metres east and north per metre up (no longer than LONGEST), as tile units (v south)
        let (east, north) = (light[0] / -light[2], light[1] / -light[2]);
        let k = (LONGEST / east.hypot(north)).min(1.0);
        let (du, dv) = (east * k / m, -north * k / m);
        let cast = |p: [f32; 3]| {
            let h = f64::from(p[2]).max(0.0);
            fill::Vertex { position: [(f64::from(p[0]) + du * h) as f32, (f64::from(p[1]) + dv * h) as f32, 0.0], color: SHADOW }
        };
        for deck in &b.decks {
            for (i, pair) in deck.iter().enumerate() {
                let base = vertices.len() as u32;
                vertices.extend(pair.map(cast));
                if i > 0 {
                    indices.extend([base - 2, base - 1, base + 1, base - 2, base + 1, base]);
                }
            }
        }
    }
    fill::Mesh::new(device, "bridge shadows", &vertices, &indices)
}

/// The pipelines and style every tile's bridges are drawn with.
pub struct BridgeStructures {
    pipeline: MeshPipeline,
    style: Style,
    shadows: Fill,
    shadow_style: fill::Style,
    /// This frame's key light direction.
    light: [f64; 3],
}

impl BridgeStructures {
    pub fn new(init: &Init<'_>, label: &str) -> Self {
        let device = &init.gpu.device;
        let pipeline = MeshPipeline::local(init, label, STRUCTURES);
        let style = pipeline.style(device, &StyleUniform::new(&Material::DECK_DEFAULT, 1.0, [255; 4]));
        let shadows = Fill::new(init, label, CLIPPED);
        let shadow_style = shadows.style(device, fill::OPAQUE);
        BridgeStructures { pipeline, style, shadows, shadow_style, light: [0.0, 0.0, -1.0] }
    }

    /// This frame's lights.
    pub fn prepare(&mut self, cx: &PrepareCx<'_>) {
        cx.set_lights(&self.pipeline);
        self.light = cx.scene.lights.key_direction;
    }

    /// Casts a tile's shadows again if the light has turned since (a hundredth of a radian).
    pub fn shade(&self, device: &wgpu::Device, bridges: Option<&mut TileBridges>) {
        let Some(b) = bridges else { return };
        let light = self.light.map(|c| c as f32);
        let turned = b.shadow.as_ref().is_none_or(|(was, _)| was.iter().zip(light).any(|(a, b)| (a - b).abs() > 0.01));
        if turned {
            b.shadow = Some((light, shadow_mesh(device, b, self.light)));
        }
    }

    /// One tile's bridges in its frame (with the tile's square marked in the stencil): the decks'
    /// shadows, then the structures.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, slot: FrameSlot, bridges: Option<&TileBridges>) {
        let Some(b) = bridges else { return };
        if cx.pass != Pass::Map || cx.scene.terrain.shown {
            return;
        }
        if let Some((_, shadow)) = &b.shadow {
            self.shadows.draw(pass, cx, slot, &self.shadow_style, shadow);
        }
        self.pipeline.draw_local(pass, cx, slot, &self.style, &b.shape);
    }
}
