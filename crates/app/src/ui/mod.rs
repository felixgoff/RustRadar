//! The egui UI over the map (plan 1.5; doc 06): the design system and one module per component.
//!
//! Design system (U1): [`theme`] (the tokens of `app.css` and [`theme::apply`]), [`fonts`] (Overpass
//! and Overpass Mono, one family per weight), [`text`] (every UI text as a `LayoutJob`: the middle
//! dot's own face, truncation with an ellipsis) and [`anim`] (the `--ease-out` curve, the
//! durations of doc 06 §27, reduced motion).
//!
//! Icons (U2): [`icons`] (the Lucide icons of doc 06 §26, rasterised with resvg per size, stroke
//! and scale, cached as egui textures and tinted when drawn).
//!
//! Components: [`map_error`] (B9), [`tooltip`] (the map's tooltip over ships, trains, signals and
//! airports, T8).

pub mod anim;
pub mod fonts;
pub mod icons;
pub mod map_error;
pub mod text;
pub mod theme;
pub mod tooltip;
pub mod widgets;
