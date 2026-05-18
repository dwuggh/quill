//! Bevy plugin integration.
//!
//! This module is behind the `bevy` feature so the core scene crate can remain
//! usable with plain `bevy_ecs` and non-Bevy hosts.

use bevy::{
    app::{App, TaskPoolPlugin, Update},
    asset::AssetPlugin,
    diagnostic::FrameCountPlugin,
    prelude::{Plugin, TransformPlugin},
    time::TimePlugin,
    window::{ExitCondition, WindowPlugin},
};
use bevy_ecs::message::{MessageRegistry, message_update_system};
use bevy_ecs::schedule::{IntoScheduleConfigs, SystemSet};

use crate::canvas::{CanvasErrors, CanvasOrder, LastAddedPage, apply_canvas_commands};
use crate::element::{
    SelectionMessage, Tool, eraser::EraserElementPlugin, route_canvas_input,
    stroke::StrokeElementPlugin,
};
use crate::input::{CanvasCommand, CanvasInput};

/// Ordered canvas update stages shared by element plugins.
#[derive(Clone, Debug, Eq, Hash, PartialEq, SystemSet)]
pub(crate) enum QuillUpdateSet {
    /// Apply high-level state commands such as tool changes.
    Commands,
    /// Route raw canvas input into the active tool's message stream.
    RouteInput,
    /// Let element plugins consume their routed messages.
    ToolMessages,
    /// Age message buffers after every tool has read this frame.
    MessageCleanup,
}

/// Installs Quill scene state and input processing into a Bevy app.
///
/// This plugin owns canvas ECS setup and modeling. It does not install a Bevy
/// render graph node or encode draw calls; host applications still own camera,
/// render targets, and render-pass ordering.
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
        let world = app.world_mut();
        MessageRegistry::register_message::<CanvasInput>(world);
        MessageRegistry::register_message::<CanvasCommand>(world);
        MessageRegistry::register_message::<SelectionMessage>(world);
        world.init_resource::<CanvasErrors>();
        world.init_resource::<LastAddedPage>();
        world.init_resource::<CanvasOrder>();
        world.insert_resource(crate::canvas::CanvasMode::new(Tool::default()));

        app.configure_sets(
            Update,
            (
                QuillUpdateSet::Commands,
                QuillUpdateSet::RouteInput,
                QuillUpdateSet::ToolMessages,
                QuillUpdateSet::MessageCleanup,
            )
                .chain(),
        );
        app.add_plugins((StrokeElementPlugin, EraserElementPlugin));
        app.add_systems(
            Update,
            (
                apply_canvas_commands.in_set(QuillUpdateSet::Commands),
                route_canvas_input.in_set(QuillUpdateSet::RouteInput),
                message_update_system.in_set(QuillUpdateSet::MessageCleanup),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use bevy::app::App;

    use super::QuillPlugin;

    #[test]
    fn plugin_can_be_added_to_app() {
        let mut app = App::new();
        app.add_plugins(QuillPlugin);
    }
}
