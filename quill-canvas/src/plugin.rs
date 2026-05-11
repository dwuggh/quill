//! Bevy plugin integration.
//!
//! This module is behind the `bevy` feature so the core scene crate can remain
//! usable with plain `bevy_ecs` and non-Bevy hosts.

use bevy::app::{App, Plugin, PostUpdate, Update};
use bevy_ecs::message::{MessageRegistry, message_update_system};
use bevy_ecs::schedule::IntoScheduleConfigs;

use crate::render::QuillRenderPlugin;
use crate::{
    canvas::{self, CanvasErrors, LastAddedPage},
    input::{CanvasCommand, CanvasInput, CanvasMode, PointerState},
    render, stroke,
    stroke::{ActiveStroke, GlobalInputModeler},
};

/// Installs Quill scene state and stroke GPU preparation into a Bevy app.
///
/// This plugin owns canvas ECS setup and modeling. It does not install a Bevy
/// render graph node or encode draw calls; host applications still own camera,
/// render targets, and render-pass ordering.
pub struct QuillPlugin;

impl Plugin for QuillPlugin {
    fn build(&self, app: &mut App) {
        let world = app.world_mut();
        MessageRegistry::register_message::<CanvasInput>(world);
        MessageRegistry::register_message::<CanvasCommand>(world);
        world.init_resource::<PointerState>();
        world.init_resource::<ActiveStroke>();
        world.init_resource::<CanvasErrors>();
        world.init_resource::<LastAddedPage>();
        world.insert_resource(CanvasMode::new(canvas::default_tool()));
        world.insert_resource(GlobalInputModeler::default());

        app.add_systems(
            Update,
            (
                canvas::apply_canvas_commands,
                stroke::update_pointer_state,
                stroke::process_canvas_input,
                message_update_system,
            )
                .chain(),
        );
        app.add_plugins(QuillRenderPlugin);
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
