//! The `path` pipeline (deck's PathLayer with PathStyleExtension): one instance per segment (previous,
//! start, end, next, width, colour, cumulative length); widths in px or metres with min/max px, miter
//! (limit 4) or round joints, butt or round caps, dashes; borders, roads, rail, trail, route, blocks,
//! ground lines (plan 1.4.6).
//!
//! Stage 2 stub (E4): empty until E6 fills it, with its shader `shaders/path.wgsl`.
