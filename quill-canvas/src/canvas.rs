//! High-level canvas facade and schedules.
//!
//! `Canvas` is a small owner around an internal ECS world. It is useful for
//! tests, tools, and non-Bevy hosts. Bevy hosts should usually install
//! `QuillPlugin` instead.

use bevy_ecs::{
    message::{MessageRegistry, Messages},
    prelude::*,
    schedule::Schedule,
};
use quill_stroke::brush::{Brush, BrushTip, StrokePaint};

use crate::{
    error::CanvasError,
    input::{CanvasCommand, CanvasInput, CanvasMode, PointerState, Tool},
    page::{Page, PageConfig},
    stroke::{self, ActiveStroke, GlobalInputModeler},
};

/// The abstract drawable canvas.
///
/// `Canvas` owns its ECS world and schedules. External code interacts with it
/// through canvas operations instead of reaching into the world directly.
pub struct Canvas {
    world: World,
    update_schedule: Schedule,
}

impl Canvas {
    /// Create an empty canvas with default tool state and input modeler.
    pub fn new() -> Self {
        let mut world = World::new();
        MessageRegistry::register_message::<CanvasInput>(&mut world);
        MessageRegistry::register_message::<CanvasCommand>(&mut world);

        world.init_resource::<PointerState>();
        world.init_resource::<ActiveStroke>();
        world.init_resource::<CanvasErrors>();
        world.init_resource::<LastAddedPage>();
        world.insert_resource(CanvasMode::new(default_tool()));
        world.insert_resource(GlobalInputModeler::default());

        let mut update_schedule = Schedule::default();
        update_schedule.add_systems(
            (
                apply_canvas_commands,
                stroke::update_pointer_state,
                stroke::process_canvas_input,
                bevy_ecs::message::message_update_system,
            )
                .chain(),
        );

        Self {
            world,
            update_schedule,
        }
    }

    /// Feed one pointer/stylus input message into the canvas.
    ///
    /// The message is handled immediately by the canvas update schedule.
    pub fn input(&mut self, input: CanvasInput) -> Result<(), CanvasError> {
        self.world
            .resource_mut::<Messages<CanvasInput>>()
            .write(input);
        self.run_update()
    }

    /// Change the active canvas tool used by future input messages.
    pub fn change_tool(&mut self, tool: Tool) -> Result<(), CanvasError> {
        self.command(CanvasCommand::SetTool(tool))
    }

    /// Add a page entity to the canvas world.
    ///
    /// Page layout is still intentionally minimal; this gives later app code a
    /// stable insertion point without exposing the internal ECS world.
    pub fn add_page(&mut self, config: PageConfig) -> Result<Entity, CanvasError> {
        self.world
            .resource_mut::<Messages<CanvasCommand>>()
            .write(CanvasCommand::AddPage(config));
        self.run_update()?;
        self.world
            .resource::<LastAddedPage>()
            .0
            .ok_or(CanvasError::PageWasNotCreated)
    }

    fn command(&mut self, command: CanvasCommand) -> Result<(), CanvasError> {
        self.world
            .resource_mut::<Messages<CanvasCommand>>()
            .write(command);
        self.run_update()
    }

    fn run_update(&mut self) -> Result<(), CanvasError> {
        self.update_schedule.run(&mut self.world);
        self.take_errors()
    }

    fn take_errors(&mut self) -> Result<(), CanvasError> {
        let mut errors = self.world.resource_mut::<CanvasErrors>();
        match errors.0.pop() {
            Some(error) => {
                errors.0.clear();
                Err(error)
            }
            None => Ok(()),
        }
    }
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-frame/system error sink.
///
/// Bevy systems cannot directly return `Result`, so systems push failures here
/// and the facade/plugin owner can decide when to inspect or surface them.
#[derive(Resource, Default)]
pub(crate) struct CanvasErrors(pub Vec<CanvasError>);

/// Stores the entity created by the most recent `AddPage` command.
#[derive(Resource, Default)]
pub(crate) struct LastAddedPage(Option<Entity>);

pub(crate) fn apply_canvas_commands(
    mut commands: Commands,
    mut canvas_commands: MessageReader<CanvasCommand>,
    mut mode: ResMut<CanvasMode>,
    mut last_added_page: ResMut<LastAddedPage>,
) {
    for command in canvas_commands.read() {
        match *command {
            CanvasCommand::SetTool(tool) => mode.tool = tool,
            CanvasCommand::AddPage(config) => {
                let entity = commands.spawn(Page::new(config)).id();
                last_added_page.0 = Some(entity);
            }
        }
    }
}

pub(crate) fn default_tool() -> Tool {
    Tool::Stroke(Brush {
        tip: BrushTip::round(2.0),
        paint: StrokePaint {
            color: [0.0, 0.0, 0.0, 1.0],
            width: 2.0,
            sharpness: 1.0,
        },
    })
}
