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

/// Apply high-level canvas commands such as tool changes and page creation.
pub fn apply_canvas_commands(
    mut commands: Commands,
    mut canvas_commands: MessageReader<CanvasCommand>,
    mut mode: ResMut<CanvasMode>,
    root: Res<DocumentRootEntity>,
    owners: Query<(), Or<(With<DocumentRoot>, With<Page>)>>,
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
    owners: &Query<(), Or<(With<DocumentRoot>, With<Page>)>>,
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
    use bevy_asset::Handle;
    use bevy_ecs::hierarchy::{ChildOf, Children};
    use peniko::{
        Brush, Fill, Style,
        kurbo::{Affine, Rect, Size},
    };

    use super::*;
    use crate::{
        asset::{CanvasFont, CanvasImage},
        element::{
            image::ImageElement,
            stroke::StrokeData,
            text::{CanvasGlyph, TextElement},
        },
    };
    use quill_stroke::input::Input;

    #[test]
    fn init_creates_document_root_and_asset_collections() {
        let canvas = Canvas::new();
        let world = canvas.world();
        let root = world.resource::<DocumentRootEntity>().0;

        assert!(world.get::<DocumentRoot>(root).is_some());
        assert!(
            world
                .get_resource::<bevy_asset::Assets<CanvasImage>>()
                .is_some()
        );
        assert!(
            world
                .get_resource::<bevy_asset::Assets<CanvasFont>>()
                .is_some()
        );
    }

    #[test]
    fn commands_append_children_and_delete_page_descendants() {
        let mut canvas = Canvas::new();
        let page = canvas.add_page(PageConfig::default()).unwrap();
        let image = canvas
            .add_image(AddImage {
                owner: Some(page),
                image: Handle::default(),
                size: Size::new(32.0, 24.0),
                transform: CanvasTransform::IDENTITY,
            })
            .unwrap();
        let text = canvas
            .add_text(AddText {
                owner: Some(page),
                text: test_text(),
                transform: CanvasTransform::IDENTITY,
            })
            .unwrap();

        let world = canvas.world();
        let page_children = world
            .get::<Children>(page)
            .unwrap()
            .iter()
            .collect::<Vec<_>>();
        assert_eq!(page_children, vec![image, text]);
        assert_eq!(world.get::<ChildOf>(image).unwrap().0, page);
        assert!(world.get::<ImageElement>(image).is_some());

        canvas.delete_entity(page).unwrap();
        let world = canvas.world();
        assert!(world.get::<Page>(page).is_none());
        assert!(world.get::<ImageElement>(image).is_none());
        assert!(world.get::<TextElement>(text).is_none());
    }

    #[test]
    fn stroke_input_inside_page_is_stored_in_page_local_coordinates() {
        let mut canvas = Canvas::new();
        let page = canvas
            .add_page_to(
                None,
                PageConfig::default(),
                CanvasTransform {
                    affine: Affine::translate((100.0, 200.0)),
                },
            )
            .unwrap();

        canvas
            .input(CanvasInput::PointerDown(Input::new(110.0, 220.0, 0.0)))
            .unwrap();
        canvas
            .input(CanvasInput::PointerUp(Input::new(120.0, 230.0, 1.0)))
            .unwrap();

        let world = canvas.world();
        let stroke_entity = world
            .get::<Children>(page)
            .unwrap()
            .iter()
            .find(|entity| world.get::<StrokeData>(*entity).is_some())
            .unwrap();
        let stroke = world.get::<StrokeData>(stroke_entity).unwrap();

        assert!((stroke.points[0].x() - 10.0).abs() < 0.001);
        assert!((stroke.points[0].y() - 20.0).abs() < 0.001);
    }

    fn test_text() -> TextElement {
        TextElement {
            font: Handle::default(),
            glyphs: vec![CanvasGlyph {
                id: 1,
                x: 0.0,
                y: 0.0,
            }],
            font_size: 16.0,
            brush: Brush::Solid(peniko::Color::BLACK),
            style: Style::Fill(Fill::NonZero),
            hint: false,
            font_embolden: Default::default(),
            glyph_transform: None,
            normalized_coords: Vec::new(),
            bounds: Rect::new(0.0, 0.0, 16.0, 16.0),
        }
    }
}
