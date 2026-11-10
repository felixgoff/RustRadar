//! The `mesh` pipeline (deck's SimpleMeshLayer and extruded polygons): a shared vertex buffer
//! (position, normal, colour), per-instance matrix, colour and highlight, or tile-local positions
//! through a `Frame`; Phong with the light rig of doc 01 §4; aircraft, gear, ship models, train cars,
//! buildings (plan 1.4.6).
//!
//! Stage 2 stub (E4): empty until E7 fills it, with its shader `shaders/mesh.wgsl`.
