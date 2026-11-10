//! RustRadar's pure logic and data types.
//!
//! No I/O, no clock, no async, no GPU, no UI: every function that needs the time takes `now_ms`.
//! This crate depends on nothing else in the workspace.

pub mod adsb;
pub mod airac;
pub mod export;
pub mod filters;
pub mod flightpath;
pub mod freshness;
pub mod geo;
pub mod hover;
pub mod keys_file;
pub mod labels;
pub mod liveries;
pub mod model;
pub mod motion;
pub mod mvt;
pub mod num;
pub mod places;
pub mod readings;
pub mod reference;
pub mod routes;
pub mod settings;
pub mod ship_details;
pub mod ship_list;
pub mod ships;
pub mod signin;
pub mod sun;
pub mod taxinet;
pub mod testing;
pub mod text;
pub mod time;
pub mod train_cars;
pub mod train_list;
pub mod train_route;
pub mod trains;
pub mod weather;
