//! High-level canvas facade and shared canvas resources.

use bevy_ecs::{hierarchy::ChildOf, message::Messages, prelude::*};

use crate::{
    document::{DocumentRoot, DocumentRootEntity},
    element::{
        Tool,
        bounds::RectBound,
        image::ImageElement,
        page::{Page, PageConfig},
    },
    error::CanvasError,
    geom::CanvasTransform,
    input::{AddImage, AddPage, AddText, CanvasCommand, CanvasInput},
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
        self.write_command(CanvasCommand::AddPage(AddPage::root(config)));
        self.run_update()?;
        self.take_last_created(CanvasError::PageWasNotCreated)
    }

    /// Add a page under an explicit page/root owner.
    pub fn add_page_to(
        &mut self,
        owner: Option<Entity>,
        config: PageConfig,
        transform: CanvasTransform,
    ) -> Result<Entity, CanvasError> {
        self.write_command(CanvasCommand::AddPage(AddPage {
            owner,
            config,
            transform,
        }));
        self.run_update()?;
        self.take_last_created(CanvasError::PageWasNotCreated)
    }

    /// Add an image entity under a page/root owner.
    pub fn add_image(&mut self, image: AddImage) -> Result<Entity, CanvasError> {
        self.write_command(CanvasCommand::AddImage(image));
        self.run_update()?;
        self.take_last_created(CanvasError::EntityWasNotCreated)
    }

    /// Add a text entity under a page/root owner.
    pub fn add_text(&mut self, text: AddText) -> Result<Entity, CanvasError> {
        self.write_command(CanvasCommand::AddText(text));
        self.run_update()?;
        self.take_last_created(CanvasError::EntityWasNotCreated)
    }

    /// Delete an entity and its hierarchy descendants.
    pub fn delete_entity(&mut self, entity: Entity) -> Result<(), CanvasError> {
        self.command(CanvasCommand::DeleteEntity(entity))
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

    fn take_last_created(&mut self, missing: CanvasError) -> Result<Entity, CanvasError> {
        self.runtime
            .world_mut()
            .resource_mut::<LastAddedEntity>()
            .0
            .take()
            .ok_or(missing)
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

/// Stores the entity created by the most recent document creation command.
#[derive(Resource, Default)]
pub struct LastAddedEntity(pub(crate) Option<Entity>);

/// Query for entities allowed to own pages and drawable elements.
pub type CanvasOwners<'w, 's> = Query<'w, 's, (), Or<(With<DocumentRoot>, With<Page>)>>;

/// Apply high-level canvas commands such as tool changes and page creation.
pub fn apply_canvas_commands(
    mut commands: Commands,
    mut canvas_commands: MessageReader<CanvasCommand>,
    mut mode: ResMut<CanvasMode>,
    root: Res<DocumentRootEntity>,
    owners: CanvasOwners<'_, '_>,
    mut last_added: ResMut<LastAddedEntity>,
    mut errors: ResMut<CanvasErrors>,
) {
    for command in canvas_commands.read() {
        last_added.0 = None;
        match command.clone() {
            CanvasCommand::SetTool(tool) => mode.tool = tool,
            CanvasCommand::AddPage(add) => {
                let Some(owner) = command_owner(add.owner, root.0, &owners, &mut errors) else {
                    continue;
                };
                let entity = commands
                    .spawn((Page::new(add.config), add.transform, ChildOf(owner)))
                    .id();
                last_added.0 = Some(entity);
            }
            CanvasCommand::AddImage(add) => {
                let Some(owner) = command_owner(add.owner, root.0, &owners, &mut errors) else {
                    continue;
                };
                let image = ImageElement::new(add.image, add.size);
                let bounds = RectBound::from_rect(image.rect());
                let entity = commands
                    .spawn((image, bounds, add.transform, ChildOf(owner)))
                    .id();
                last_added.0 = Some(entity);
            }
            CanvasCommand::AddText(add) => {
                let Some(owner) = command_owner(add.owner, root.0, &owners, &mut errors) else {
                    continue;
                };
                let bounds = RectBound::from_rect(add.text.bounds);
                let entity = commands
                    .spawn((add.text, bounds, add.transform, ChildOf(owner)))
                    .id();
                last_added.0 = Some(entity);
            }
            CanvasCommand::DeleteEntity(entity) => commands.entity(entity).despawn(),
        }
    }
}

fn command_owner(
    requested: Option<Entity>,
    root: Entity,
    owners: &CanvasOwners<'_, '_>,
    errors: &mut CanvasErrors,
) -> Option<Entity> {
    let owner = requested.unwrap_or(root);
    if owners.get(owner).is_ok() {
        Some(owner)
    } else {
        errors
            .0
            .push(CanvasError::InvalidOwner { owner: requested });
        None
    }
}

#[cfg(test)]
mod tests {
    use bevy_ecs::hierarchy::{ChildOf, Children};
    use quill_stroke::input::{InputModeler, InputSample, PathPoint, WillInputModeler};

    use super::*;
    use crate::{
        document::DocumentRootEntity,
        element::{
            EraserConfig, EraserShape,
            selection::{SelectionState, SelectionTool},
            stroke::{StrokeBrush, StrokeData, rect_bound_from_path_points},
        },
    };

    #[test]
    fn pixel_eraser_splits_stroke_entities_and_recomputes_bounds() {
        let mut canvas = Canvas::new();
        let root = canvas.world().resource::<DocumentRootEntity>().0;
        let stroke = spawn_stroke(
            &mut canvas,
            root,
            &[(0.0, 0.0), (4.0, 0.0), (6.0, 0.0), (10.0, 0.0)],
        );

        canvas
            .change_tool(Tool::Eraser(EraserConfig::new(EraserShape::Circle, 0.5)))
            .unwrap();
        canvas
            .input(CanvasInput::PointerDown(
                InputSample::new(5.0, -2.0, 0.0).with_pressure(1.0),
            ))
            .unwrap();
        canvas
            .input(CanvasInput::PointerUp(
                InputSample::new(5.0, 2.0, 1.0).with_pressure(1.0),
            ))
            .unwrap();

        let world = canvas.world();
        assert!(world.get::<StrokeData>(stroke).is_none());
        let fragments = world
            .get::<Children>(root)
            .unwrap()
            .iter()
            .filter(|entity| world.get::<StrokeData>(*entity).is_some())
            .collect::<Vec<_>>();
        assert_eq!(fragments.len(), 2);

        let first_bound = world.get::<RectBound>(fragments[0]).unwrap().rect;
        let second_bound = world.get::<RectBound>(fragments[1]).unwrap().rect;
        assert!(first_bound.x1 <= 4.51);
        assert!(second_bound.x0 >= 5.49);

        let selection_tool = SelectionTool {
            hit_radius: 0.1,
            ..Default::default()
        };
        canvas.change_tool(Tool::Selection(selection_tool)).unwrap();
        canvas
            .input(CanvasInput::PointerDown(
                InputSample::new(5.0, 0.0, 2.0).with_pressure(1.0),
            ))
            .unwrap();
        canvas
            .input(CanvasInput::PointerUp(
                InputSample::new(5.0, 0.0, 3.0).with_pressure(1.0),
            ))
            .unwrap();

        let selection = canvas.world().resource::<SelectionState>();
        assert!(selection.selected().is_empty());
    }

    fn spawn_stroke(canvas: &mut Canvas, owner: Entity, coords: &[(f32, f32)]) -> Entity {
        let brush = match Tool::default() {
            Tool::Stroke(brush) => StrokeBrush { brush },
            _ => unreachable!(),
        };
        let points = coords
            .iter()
            .map(|(x, y)| modeled_point(*x, *y))
            .collect::<Vec<_>>();
        let data = StrokeData::from_finished_points(brush, points);
        let bounds = rect_bound_from_path_points(&data.points).unwrap();
        canvas
            .world_mut()
            .spawn((data, bounds, CanvasTransform::IDENTITY, ChildOf(owner)))
            .id()
    }

    fn modeled_point(x: f32, y: f32) -> PathPoint {
        let input = InputSample::new(x, y, 0.0).with_pressure(1.0);
        let mut modeler = WillInputModeler::new();
        modeler.enqueue(&[input], &[]).unwrap().updated[0]
    }
}
