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
//! airports, T8), [`shell`] and [`sidebar`] (the desktop shell, the page's keys: U6), [`modebar`]
//! and [`attribution`] (the mode bar, the credits and the dev readout: U7), [`search`] (the search
//! box and its dropdown: U8), [`lists`] and [`filters`] (the sidebar's lists and filter panels:
//! U9 for the aircraft), [`panel`] (the detail panels' shared anatomy: U10) and [`flight_panel`]
//! (U10, U11).

pub mod anim;
pub mod attribution;
pub mod filters;
pub mod flight_panel;
pub mod fonts;
pub mod icons;
pub mod lists;
pub mod map_error;
pub mod modebar;
pub mod panel;
pub mod search;
pub mod shell;
pub mod sidebar;
pub mod text;
pub mod theme;
pub mod tooltip;
pub mod widgets;
