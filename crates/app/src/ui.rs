//! The egui overlay. Placeholder for now: one panel with a search field and a button.

pub mod map_error;

use crate::theme;

/// Everything the UI remembers between frames.
#[derive(Default)]
pub struct UiState {
    pub search: String,
    pub clicks: u32,
}

pub fn draw(ui: &mut egui::Ui, state: &mut UiState) {
    let margin = 4.0 * theme::SPACE;
    egui::Area::new(egui::Id::unique("search_panel"))
        .fixed_pos(egui::pos2(margin, margin))
        // Areas fade in over a few frames by default; a permanent panel should just be there.
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(theme::SURFACE)
                .stroke(egui::Stroke::new(1.0, theme::LINE))
                .corner_radius(theme::RADIUS)
                .inner_margin(3.0 * theme::SPACE)
                .show(ui, |ui| {
                    ui.set_width(280.0);
                    ui.heading("RustRadar");
                    ui.add(
                        egui::TextEdit::singleline(&mut state.search)
                            .hint_text("Search flights, airports")
                            .desired_width(f32::INFINITY),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Search").clicked() {
                            state.clicks += 1;
                        }
                        ui.colored_label(theme::TEXT_3, format!("{} searches", state.clicks));
                    });
                });
        });
}
