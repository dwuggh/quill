//! Plain `bevy_ecs` runtime for the canvas scene model.

use bevy_ecs::{
    message::{MessageRegistry, message_update_system},
    prelude::*,
    schedule::{IntoScheduleConfigs, Schedule, SystemSet},
};

use crate::{
    asset::{CanvasFont, CanvasImage},
    canvas::{CanvasErrors, CanvasMode, CanvasOrder, LastAddedEntity, apply_canvas_commands},
    document::init_document_root,
    element::{
        Tool, eraser::register_eraser_systems, route_canvas_input,
        selection::register_selection_systems, stroke::register_stroke_systems,
    },
    input::{CanvasCommand, CanvasInput},
};

/// Ordered canvas update stages shared by element systems.
#[derive(Clone, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum QuillUpdateSet {
    /// Apply high-level state commands such as tool changes.
    Commands,
    /// Route raw canvas input into the active tool's message stream.
    RouteInput,
    /// Let element systems consume their routed messages.
    ToolMessages,
    /// Age message buffers after every tool has read this frame.
    MessageCleanup,
}

/// Self-contained ECS runtime for non-rendering canvas use.
pub struct CanvasRuntime {
    /// ECS world containing canvas resources and entities.
    world: World,
    /// Canvas update schedule.
    schedule: Schedule,
}

impl CanvasRuntime {
    /// Create a runtime with the default canvas resources and systems installed.
    pub fn new() -> Self {
        let mut world = World::new();
        init_canvas_world(&mut world);

        let mut schedule = Schedule::default();
        install_canvas_systems(&mut schedule);

        Self { world, schedule }
    }

    /// Return immutable access to the runtime world.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Return mutable access to the runtime world.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Run one canvas update tick.
    pub fn update(&mut self) {
        self.schedule.run(&mut self.world);
    }
}

impl Default for CanvasRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// Install canvas resources and message types into a world.
pub fn init_canvas_world(world: &mut World) {
    MessageRegistry::register_message::<CanvasInput>(world);
    MessageRegistry::register_message::<CanvasCommand>(world);
    world.init_resource::<bevy_asset::Assets<CanvasImage>>();
    world.init_resource::<bevy_asset::Assets<CanvasFont>>();
    init_document_root(world);
    world.init_resource::<CanvasOrder>();
    world.init_resource::<CanvasErrors>();
    world.init_resource::<LastAddedEntity>();
    world.insert_resource(CanvasMode::new(Tool::default()));

    register_stroke_systems(world);
    register_eraser_systems(world);
    register_selection_systems(world);
}

/// Install the ordered canvas update systems into a schedule.
pub fn install_canvas_systems(schedule: &mut Schedule) {
    schedule.configure_sets(
        (
            QuillUpdateSet::Commands,
            QuillUpdateSet::RouteInput,
            QuillUpdateSet::ToolMessages,
            QuillUpdateSet::MessageCleanup,
        )
            .chain(),
    );

    schedule.add_systems((
        apply_canvas_commands.in_set(QuillUpdateSet::Commands),
        route_canvas_input.in_set(QuillUpdateSet::RouteInput),
        message_update_system.in_set(QuillUpdateSet::MessageCleanup),
    ));

    crate::element::stroke::install_stroke_systems(schedule);
    crate::element::eraser::install_eraser_systems(schedule);
    crate::element::selection::install_selection_systems(schedule);
}
