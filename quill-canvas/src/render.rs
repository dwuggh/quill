//! Bevy render integration for canvas elements.
//!
//! `stroke` owns the stroke draw pipeline. `offscreen` owns the manual target
//! and render-graph handoff used by host UI renderers.

pub mod offscreen;
pub mod stroke;

use bevy::app::{App, Plugin};

pub use offscreen::OffscreenRenderPlugin;
pub(crate) use offscreen::{OffscreenRenderConfig, create_target, install_render_target};
pub use stroke::{StrokePipeline, StrokePipelineKey, StrokeRenderPlugin, StrokeTipKind};

/// Installs all canvas rendering pieces into the Bevy app.
pub struct QuillRenderPlugin;

impl Plugin for QuillRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((OffscreenRenderPlugin, StrokeRenderPlugin));
    }
}
