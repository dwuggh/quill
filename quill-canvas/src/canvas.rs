//! High-level canvas facade and shared canvas resources.

use bevy_ecs::{message::Messages, prelude::*};

use crate::{
    element::{
        Tool,
        page::{Page, PageConfig},
    },
    error::CanvasError,
    input::{CanvasCommand, CanvasInput},
    runtime::CanvasRuntime,
};

/// Global canvas mode resource.
///
/// For now the mode is just the current tool. Brush style lives inside
/// `Tool::Stroke`, so a pointer-down event can snapshot the whole tool state
/// onto the new stroke entity.
#[derive(Resource, Clone, Copy, Debug)]
pub struct CanvasMode {
    /// Tool used by future pointer input.
    pub tool: Tool,
}

impl CanvasMode {
    /// Create a mode resource with the given active tool.
    pub fn new(tool: Tool) -> Self {
        Self { tool }
    }
}

/// Draw order for canvas elements.
#[derive(Resource, Default)]
pub struct CanvasOrder {
    /// Stroke entities in document draw order.
    pub strokes: Vec<Entity>,
}

/// The abstract drawable canvas.
pub struct Canvas {
    /// Private ECS runtime that owns scene resources, entities, and messages.
    runtime: CanvasRuntime,
}

impl Canvas {
    /// Create an empty canvas with default tool state and input modeler.
    pub fn new() -> Self {
        Self {
            runtime: CanvasRuntime::new(),
        }
    }

    /// Return immutable access to the underlying ECS world.
    pub fn world(&self) -> &World {
        self.runtime.world()
    }

    /// Return mutable access to the underlying ECS world.
    pub fn world_mut(&mut self) -> &mut World {
        self.runtime.world_mut()
    }

    /// Feed one pointer/stylus input message into the canvas.
    ///
    /// The message is handled immediately by the canvas update schedule.
    pub fn input(&mut self, input: CanvasInput) -> Result<(), CanvasError> {
        self.write_input(input);
        self.run_update()
    }

    /// Change the active canvas tool used by future input messages.
    pub fn change_tool(&mut self, tool: Tool) -> Result<(), CanvasError> {
        self.command(CanvasCommand::SetTool(tool))
    }

    /// Add a page entity to the canvas world.
    pub fn add_page(&mut self, config: PageConfig) -> Result<Entity, CanvasError> {
        self.write_command(CanvasCommand::AddPage(config));
        self.run_update()?;
        self.runtime
            .world()
            .resource::<LastAddedPage>()
            .0
            .ok_or(CanvasError::PageWasNotCreated)
    }

    fn command(&mut self, command: CanvasCommand) -> Result<(), CanvasError> {
        self.write_command(command);
        self.run_update()
    }

    fn write_command(&mut self, command: CanvasCommand) {
        self.runtime
            .world_mut()
            .resource_mut::<Messages<CanvasCommand>>()
            .write(command);
    }

    fn write_input(&mut self, input: CanvasInput) {
        self.runtime
            .world_mut()
            .resource_mut::<Messages<CanvasInput>>()
            .write(input);
    }

    fn run_update(&mut self) -> Result<(), CanvasError> {
        self.runtime.update();
        self.take_errors()
    }

    fn take_errors(&mut self) -> Result<(), CanvasError> {
        let mut errors = self.runtime.world_mut().resource_mut::<CanvasErrors>();
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
/// ECS systems cannot directly return `Result`, so systems push failures here
/// and the facade/plugin owner can decide when to inspect or surface them.
#[derive(Resource, Default)]
pub struct CanvasErrors(pub(crate) Vec<CanvasError>);

/// Stores the entity created by the most recent `AddPage` command.
#[derive(Resource, Default)]
pub struct LastAddedPage(pub(crate) Option<Entity>);

/// Apply high-level canvas commands such as tool changes and page creation.
pub fn apply_canvas_commands(
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
