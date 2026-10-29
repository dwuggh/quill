//! Bevy renderer integration for Quill canvas scenes.

// WGPU 29's resource wrappers require deeper auto-trait evaluation.
#![recursion_limit = "256"]

pub mod canvas;
pub mod render;

use bevy::{
    app::{App, Plugin, TaskPoolPlugin, Update},
    asset::AssetPlugin,
    diagnostic::FrameCountPlugin,
    prelude::TransformPlugin,
    time::TimePlugin,
    window::{ExitCondition, WindowPlugin},
};

pub use bevy::render::render_resource::Texture;
pub use canvas::CanvasRenderer;
pub use render::{
    EraserPreviewPipeline, EraserPreviewPipelineKey, EraserRenderPlugin, OffscreenRenderPlugin,
    QuillRenderPlugin, StrokePipeline, StrokePipelineKey, StrokeRenderPlugin, StrokeTipKind,
};

/// Installs Quill scene state and input processing into a Bevy app.
pub struct QuillPlugin;

impl Plugin for QuillPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            TaskPoolPlugin::default(),
            FrameCountPlugin,
            TimePlugin,
            TransformPlugin,
            AssetPlugin::default(),
            WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..Default::default()
            },
        ));

        quill_canvas::runtime::init_canvas_world(app.world_mut());
        app.edit_schedule(Update, quill_canvas::runtime::install_canvas_systems);
    }
}

/// Common imports for Bevy renderer hosts.
pub mod prelude {
    pub use crate::{CanvasRenderer, QuillPlugin, QuillRenderPlugin, Texture};
}
