//! GPU shader sources for stroke rendering.
//!
//! This module intentionally exposes shader text only. Backend-specific GPU
//! resources live in renderer crates such as `quill-canvas`.

/// Analytic round-tip stroke shader.
pub const ROUND_SHADER: &str = include_str!("shader/circle.wgsl");

/// Path point overlay shader.
pub const POINT_SHADER: &str = include_str!("shader/point.wgsl");
