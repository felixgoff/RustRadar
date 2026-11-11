//! The CPU side of RustRadar's map: camera, tiles, tessellation, meshing, label layout, picking
//! and the per-kind instance builders.
//!
//! No GPU and no I/O; like `rustradar-core`, nothing here reads a clock.

pub mod aircraft;
pub mod aircraft_assets;
pub mod aircraft_labels;
pub mod aircraft_models;
pub mod atlas;
pub mod blocks;
pub mod bridges;
pub mod building_shadow;
pub mod camera;
pub mod canopy;
pub mod clouds;
pub mod controller;
pub mod declutter;
pub mod geom;
pub mod gpu_types;
pub mod ground;
pub mod hillshade;
pub mod labels_map;
pub mod landcover;
pub mod landcover_ground;
pub mod lod;
pub mod look;
pub mod overture;
pub mod pick;
pub mod radar;
pub mod rail;
pub mod rail_overview;
pub mod road_widths;
pub mod roofs;
pub mod screen;
pub mod sdf;
pub mod ship_models;
pub mod ships;
pub mod street_trees;
pub mod streets_gl;
pub mod terrain;
pub mod text;
pub mod tiles;
pub mod train_models;
pub mod trains;
pub mod trees;
pub mod tess;
