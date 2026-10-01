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
        kurbo::{Affine, Point, Rect, Size},
    };
    use quill_stroke::input::{Input, InputModeler, PathPoint, WillInputModeler};

    use super::*;
    use crate::{
        asset::{CanvasFont, CanvasImage},
        document::{DocumentRootEntity, root_from_entity},
        element::{
            EraserConfig, EraserMode, EraserShape,
            image::ImageElement,
            selection::{
                SelectionArea, SelectionAreaShape, SelectionMode, SelectionOp, SelectionState,
                SelectionTool,
            },
            stroke::{StrokeBrush, StrokeData, rect_bound_from_path_points},
            text::{CanvasGlyph, TextElement},
        },
    };

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

    #[test]
    fn rect_selection_creates_dashed_area_entity() {
        let mut canvas = Canvas::new();
        let root = canvas.world().resource::<DocumentRootEntity>().0;
        let stroke = spawn_stroke(&mut canvas, root, &[(10.0, 10.0), (20.0, 12.0)]);
        canvas
            .change_tool(Tool::Selection(SelectionTool {
                mode: SelectionMode::Rect,
                ..Default::default()
            }))
            .unwrap();

        canvas
            .input(CanvasInput::PointerDown(
                Input::new(0.0, 0.0, 0.0).with_pressure(1.0),
            ))
            .unwrap();
        canvas
            .input(CanvasInput::PointerMove(
                Input::new(30.0, 30.0, 1.0).with_pressure(1.0),
            ))
            .unwrap();
        canvas
            .input(CanvasInput::PointerUp(
                Input::new(30.0, 30.0, 2.0).with_pressure(1.0),
            ))
            .unwrap();

        let world = canvas.world();
        let selection = world.resource::<SelectionState>();
        assert!(selection.contains(stroke));
        let area = current_selection_area(world);
        assert_eq!(area.owner, root);
        assert!(
            matches!(area.shape, SelectionAreaShape::Rect(rect) if rect == Rect::new(0.0, 0.0, 30.0, 30.0))
        );
    }

    #[test]
    fn lasso_selection_stores_input_path_as_area() {
        let mut canvas = Canvas::new();
        let root = canvas.world().resource::<DocumentRootEntity>().0;
        let stroke = spawn_stroke(&mut canvas, root, &[(10.0, 10.0), (20.0, 12.0)]);
        canvas
            .change_tool(Tool::Selection(SelectionTool {
                mode: SelectionMode::Lasso,
                ..Default::default()
            }))
            .unwrap();

        for input in [
            CanvasInput::PointerDown(Input::new(0.0, 0.0, 0.0).with_pressure(1.0)),
            CanvasInput::PointerMove(Input::new(30.0, 0.0, 1.0).with_pressure(1.0)),
            CanvasInput::PointerMove(Input::new(30.0, 30.0, 2.0).with_pressure(1.0)),
            CanvasInput::PointerUp(Input::new(0.0, 30.0, 3.0).with_pressure(1.0)),
        ] {
            canvas.input(input).unwrap();
        }

        let world = canvas.world();
        assert!(world.resource::<SelectionState>().contains(stroke));
        let area = current_selection_area(world);
        assert!(
            matches!(&area.shape, SelectionAreaShape::Lasso(points) if points.len() == 5 && points.first() == points.last())
        );
    }

    #[test]
    fn dragging_selection_area_moves_and_reparents_selected_elements() {
        let mut canvas = Canvas::new();
        let first = canvas.add_page(PageConfig::default()).unwrap();
        let second = canvas
            .add_page_to(
                None,
                PageConfig::default(),
                CanvasTransform {
                    affine: Affine::translate((200.0, 0.0)),
                },
            )
            .unwrap();
        let stroke = spawn_stroke(&mut canvas, first, &[(10.0, 10.0), (20.0, 12.0)]);
        canvas
            .change_tool(Tool::Selection(SelectionTool::default()))
            .unwrap();

        canvas
            .input(CanvasInput::PointerDown(
                Input::new(10.0, 10.0, 0.0).with_pressure(1.0),
            ))
            .unwrap();
        canvas
            .input(CanvasInput::PointerUp(
                Input::new(10.0, 10.0, 1.0).with_pressure(1.0),
            ))
            .unwrap();

        canvas
            .input(CanvasInput::PointerDown(
                Input::new(10.0, 10.0, 2.0).with_pressure(1.0),
            ))
            .unwrap();
        canvas
            .input(CanvasInput::PointerMove(
                Input::new(210.0, 10.0, 3.0).with_pressure(1.0),
            ))
            .unwrap();
        canvas
            .input(CanvasInput::PointerUp(
                Input::new(210.0, 10.0, 4.0).with_pressure(1.0),
            ))
            .unwrap();

        let world = canvas.world();
        assert_eq!(world.get::<ChildOf>(stroke).unwrap().0, second);
        let root_from_stroke = root_from_entity(world, stroke).unwrap();
        let moved_origin = root_from_stroke * Point::ZERO;
        assert!((moved_origin.x - 200.0).abs() < 0.001);
        assert!(world.resource::<SelectionState>().contains(stroke));
        assert_eq!(current_selection_area(world).owner, second);
        assert!(matches!(
            current_selection_area(world).shape,
            SelectionAreaShape::ItemRect(rect) if rect.x0 >= 210.0
        ));
    }

    #[test]
    fn pixel_eraser_masks_visible_strokes_across_pages() {
        let mut canvas = Canvas::new();
        let first = canvas.add_page(PageConfig::default()).unwrap();
        let second = canvas
            .add_page_to(
                None,
                PageConfig::default(),
                CanvasTransform {
                    affine: Affine::translate((200.0, 0.0)),
                },
            )
            .unwrap();

        let first_stroke = spawn_stroke(&mut canvas, first, &[(10.0, 10.0), (20.0, 10.0)]);
        let second_stroke = spawn_stroke(&mut canvas, second, &[(10.0, 10.0), (20.0, 10.0)]);
        canvas
            .change_tool(Tool::Eraser(EraserConfig::new(EraserShape::Circle, 2.0)))
            .unwrap();
        canvas
            .input(CanvasInput::PointerDown(Input::new(10.0, 10.0, 10.0)))
            .unwrap();
        canvas
            .input(CanvasInput::PointerUp(Input::new(210.0, 10.0, 11.0)))
            .unwrap();

        let world = canvas.world();
        assert!(world.get::<StrokeData>(first_stroke).is_none());
        assert!(world.get::<StrokeData>(second_stroke).is_some());
        assert!(world.get::<RectBound>(second_stroke).unwrap().rect.x0 > 10.0);
    }

    #[test]
    fn eraser_can_mask_page_child_outside_page_rect() {
        let mut canvas = Canvas::new();
        let page = canvas.add_page(PageConfig::default()).unwrap();
        let stroke = spawn_stroke(&mut canvas, page, &[(-20.0, -20.0), (-10.0, -20.0)]);

        canvas
            .change_tool(Tool::Eraser(EraserConfig::new(EraserShape::Circle, 2.0)))
            .unwrap();
        canvas
            .input(CanvasInput::PointerDown(
                Input::new(-15.0, -22.0, 0.0).with_pressure(1.0),
            ))
            .unwrap();
        canvas
            .input(CanvasInput::PointerUp(
                Input::new(-15.0, -18.0, 1.0).with_pressure(1.0),
            ))
            .unwrap();

        let world = canvas.world();
        assert!(world.get::<StrokeData>(stroke).is_none());
    }

    #[test]
    fn pixel_eraser_replaces_hit_strokes_with_fragments() {
        let mut canvas = Canvas::new();
        let root = canvas.world().resource::<DocumentRootEntity>().0;
        let first = spawn_stroke(
            &mut canvas,
            root,
            &[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (3.0, 0.0), (4.0, 0.0)],
        );
        let second = spawn_stroke(
            &mut canvas,
            root,
            &[
                (0.0, 10.0),
                (1.0, 10.0),
                (2.0, 10.0),
                (3.0, 10.0),
                (4.0, 10.0),
            ],
        );

        canvas
            .change_tool(Tool::Eraser(EraserConfig::new(EraserShape::Circle, 0.25)))
            .unwrap();
        canvas
            .input(CanvasInput::PointerDown(
                Input::new(2.0, -1.0, 0.0).with_pressure(1.0),
            ))
            .unwrap();
        canvas
            .input(CanvasInput::PointerUp(
                Input::new(2.0, 11.0, 1.0).with_pressure(1.0),
            ))
            .unwrap();

        let world = canvas.world();
        assert!(world.get::<StrokeData>(first).is_none());
        assert!(world.get::<StrokeData>(second).is_none());
        let fragments = world
            .get::<Children>(root)
            .unwrap()
            .iter()
            .filter(|entity| world.get::<StrokeData>(*entity).is_some())
            .collect::<Vec<_>>();

        assert_eq!(fragments.len(), 4);
        for fragment in fragments {
            assert_eq!(world.get::<ChildOf>(fragment).unwrap().0, root);
            assert!(world.get::<RectBound>(fragment).is_some());
        }
    }

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
                Input::new(5.0, -2.0, 0.0).with_pressure(1.0),
            ))
            .unwrap();
        canvas
            .input(CanvasInput::PointerUp(
                Input::new(5.0, 2.0, 1.0).with_pressure(1.0),
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

        let mut selection_tool = SelectionTool::default();
        selection_tool.hit_radius = 0.1;
        canvas.change_tool(Tool::Selection(selection_tool)).unwrap();
        canvas
            .input(CanvasInput::PointerDown(
                Input::new(5.0, 0.0, 2.0).with_pressure(1.0),
            ))
            .unwrap();
        canvas
            .input(CanvasInput::PointerUp(
                Input::new(5.0, 0.0, 3.0).with_pressure(1.0),
            ))
            .unwrap();

        let selection = canvas.world().resource::<SelectionState>();
        assert!(selection.selected().is_empty());
    }

    #[test]
    fn eraser_replaces_selected_stroke_with_selected_fragments() {
        let mut canvas = Canvas::new();
        let root = canvas.world().resource::<DocumentRootEntity>().0;
        let stroke = spawn_stroke(
            &mut canvas,
            root,
            &[(0.0, 0.0), (4.0, 0.0), (6.0, 0.0), (10.0, 0.0)],
        );
        canvas
            .world_mut()
            .resource_mut::<SelectionState>()
            .apply(SelectionOp::Replace, [stroke]);

        canvas
            .change_tool(Tool::Eraser(EraserConfig::new(EraserShape::Circle, 0.5)))
            .unwrap();
        canvas
            .input(CanvasInput::PointerDown(
                Input::new(5.0, -2.0, 0.0).with_pressure(1.0),
            ))
            .unwrap();
        canvas
            .input(CanvasInput::PointerUp(
                Input::new(5.0, 2.0, 1.0).with_pressure(1.0),
            ))
            .unwrap();

        let world = canvas.world();
        let selection = world.resource::<SelectionState>();
        assert!(!selection.contains(stroke));
        assert_eq!(selection.selected().len(), 2);
        for entity in selection.selected() {
            assert!(world.get::<StrokeData>(*entity).is_some());
            assert!(world.get::<RectBound>(*entity).is_some());
        }
    }

    #[test]
    fn item_eraser_deletes_hit_strokes() {
        let mut canvas = Canvas::new();
        let root = canvas.world().resource::<DocumentRootEntity>().0;
        let hit = spawn_stroke(&mut canvas, root, &[(0.0, 0.0), (4.0, 0.0)]);
        let miss = spawn_stroke(&mut canvas, root, &[(0.0, 10.0), (4.0, 10.0)]);
        let mut config = EraserConfig::new(EraserShape::Circle, 2.0);
        config.mode = EraserMode::ItemWise;
        canvas
            .world_mut()
            .resource_mut::<SelectionState>()
            .apply(SelectionOp::Replace, [hit]);

        canvas.change_tool(Tool::Eraser(config)).unwrap();
        canvas
            .input(CanvasInput::PointerDown(
                Input::new(2.0, -1.0, 0.0).with_pressure(1.0),
            ))
            .unwrap();
        canvas
            .input(CanvasInput::PointerUp(
                Input::new(2.0, 1.0, 1.0).with_pressure(1.0),
            ))
            .unwrap();

        let world = canvas.world();
        assert!(world.get::<StrokeData>(hit).is_none());
        assert!(world.get::<StrokeData>(miss).is_some());
        assert!(!world.resource::<SelectionState>().contains(hit));
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

    fn current_selection_area(world: &World) -> &SelectionArea {
        let entity = world.resource::<SelectionState>().area_entity().unwrap();
        world.get::<SelectionArea>(entity).unwrap()
    }

    fn modeled_point(x: f32, y: f32) -> PathPoint {
        let input = Input::new(x, y, 0.0).with_pressure(1.0);
        let mut modeler = WillInputModeler::new();
        modeler.enqueue(&[input], &[]).unwrap().updated[0]
    }
}
