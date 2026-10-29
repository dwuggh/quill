//! Selection tool state and input handling.
//!
//! Selection is stored as unordered membership. Canvas draw order is only
//! relevant to future hit testing, where topmost elements should win clicks.

use std::collections::HashSet;

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule};
use bevy_ecs::{
    hierarchy::{ChildOf, Children},
    message::MessageRegistry,
};
use peniko::kurbo::{Affine, Point, Rect, Vec2};

use crate::{
    canvas::CanvasMode,
    document::{DocumentRootEntity, root_from_entity_query, target_owner_at_query},
    element::{
        SelectionMessage, Tool,
        bounds::{RectBound, RoundBound, rect_from_min_max},
        image::ImageElement,
        page::Page,
        stroke::StrokeData,
        text::TextElement,
    },
    geom::CanvasTransform,
    input::CanvasInput,
    runtime::QuillUpdateSet,
};
use quill_stroke::input::Input;

/// Selection tool configuration carried by `Tool::Selection`.
#[derive(Clone, Copy, Debug)]
pub struct SelectionTool {
    /// Shape of the selection gesture.
    pub mode: SelectionMode,
    /// Extra radius used by point-style hit tests.
    pub hit_radius: f32,
    /// How hits from the next gesture combine with current selection.
    pub op: SelectionOp,
}

impl Default for SelectionTool {
    fn default() -> Self {
        Self {
            mode: SelectionMode::Item,
            hit_radius: 4.0,
            op: SelectionOp::Replace,
        }
    }
}

/// Shape of the current selection gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionMode {
    /// Click or tap a semantic item.
    Item,
    /// Drag a rectangular marquee.
    Rect,
    /// Drag a freeform lasso path.
    Lasso,
}

/// How a hit result is merged into existing selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionOp {
    /// Clear current selection and select the hit result.
    Replace,
    /// Keep current selection and add the hit result.
    Add,
    /// Selected hits become unselected, unselected hits become selected.
    Toggle,
    /// Remove the hit result from current selection.
    Subtract,
}

/// Marker on entities that are currently selected.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Selected;

/// Editor-only dashed selection area used for overlay rendering and drag hits.
#[derive(Component, Clone, Debug)]
pub struct SelectionArea {
    /// Owner subtree used when the area was created.
    pub owner: Entity,
    /// Root-space area shape.
    pub shape: SelectionAreaShape,
}

/// Root-space selection area geometry.
#[derive(Clone, Debug)]
pub enum SelectionAreaShape {
    /// Item-selection rectangle.
    ItemRect(Rect),
    /// Rectangle-selection area.
    Rect(Rect),
    /// Lasso-selection input path, closed by repeating the first point at the end.
    Lasso(Vec<Point>),
}

/// Canvas-wide selection state.
#[derive(Resource, Debug, Default)]
pub struct SelectionState {
    selected: HashSet<Entity>,
    primary: Option<Entity>,
    gesture: Option<SelectionGesture>,
    owner: Option<Entity>,
    area_entity: Option<Entity>,
    drag: Option<SelectionDrag>,
}

impl SelectionState {
    /// Return the current unordered selected entity set.
    pub fn selected(&self) -> &HashSet<Entity> {
        &self.selected
    }

    /// Return the focused selected entity, when one exists.
    pub fn primary(&self) -> Option<Entity> {
        self.primary
    }

    /// Return the transient selection gesture, when one is active.
    pub fn gesture(&self) -> Option<&SelectionGesture> {
        self.gesture.as_ref()
    }

    /// Return the current editor selection-area entity.
    pub fn area_entity(&self) -> Option<Entity> {
        self.area_entity
    }

    /// Clear selection membership and the primary selected entity.
    pub fn clear(&mut self) {
        self.selected.clear();
        self.primary = None;
    }

    /// Return whether an entity is selected.
    pub fn contains(&self, entity: Entity) -> bool {
        self.selected.contains(&entity)
    }

    /// Remove one entity from the current selection and repair the primary.
    pub fn remove_entity(&mut self, entity: Entity) {
        self.selected.remove(&entity);
        self.repair_primary();
    }

    /// Replace one selected entity with replacement entities.
    ///
    /// This is used by tools that structurally rewrite a drawable, such as a
    /// stroke eraser splitting one stroke into several fragment entities.
    pub fn replace_entity(&mut self, old: Entity, replacements: impl IntoIterator<Item = Entity>) {
        let was_selected = self.selected.remove(&old);
        let was_primary = self.primary == Some(old);
        if !was_selected {
            self.repair_primary();
            return;
        }

        let mut first = None;
        for entity in replacements {
            first.get_or_insert(entity);
            self.selected.insert(entity);
        }

        if was_primary {
            self.primary = first;
        }
        self.repair_primary();
    }

    /// Apply hit results to the current selection with the requested operation.
    pub fn apply(&mut self, op: SelectionOp, hits: impl IntoIterator<Item = Entity>) {
        match op {
            SelectionOp::Replace => {
                self.clear();
                for entity in hits {
                    self.primary.get_or_insert(entity);
                    self.selected.insert(entity);
                }
            }
            SelectionOp::Add => {
                for entity in hits {
                    self.primary = Some(entity);
                    self.selected.insert(entity);
                }
            }
            SelectionOp::Toggle => {
                for entity in hits {
                    if self.selected.remove(&entity) {
                        continue;
                    }

                    self.selected.insert(entity);
                    self.primary = Some(entity);
                }
                self.repair_primary();
            }
            SelectionOp::Subtract => {
                for entity in hits {
                    self.selected.remove(&entity);
                }
                self.repair_primary();
            }
        }
    }

    /// Begin a transient gesture from a pointer-down position.
    pub fn begin_gesture(&mut self, mode: SelectionMode, position: Point) {
        self.gesture = match mode {
            SelectionMode::Item => Some(SelectionGesture::Item { position }),
            SelectionMode::Rect => Some(SelectionGesture::Rect {
                start: position,
                current: position,
            }),
            SelectionMode::Lasso => Some(SelectionGesture::Lasso {
                points: vec![position],
            }),
        };
    }

    /// Update the active transient gesture from a pointer-move position.
    pub fn update_gesture(&mut self, position: Point) {
        match self.gesture.as_mut() {
            Some(SelectionGesture::Item { position: item }) => *item = position,
            Some(SelectionGesture::Rect { current, .. }) => *current = position,
            Some(SelectionGesture::Lasso { points }) => points.push(position),
            None => {}
        }
    }

    /// Finish and return the active transient gesture.
    pub fn finish_gesture(&mut self) -> Option<SelectionGesture> {
        self.gesture.take()
    }

    /// Cancel the active transient gesture without changing selected entities.
    pub fn cancel_gesture(&mut self) {
        self.gesture = None;
        self.owner = None;
        self.drag = None;
    }

    fn repair_primary(&mut self) {
        if self
            .primary
            .is_some_and(|entity| self.selected.contains(&entity))
        {
            return;
        }

        self.primary = self.selected.iter().next().copied();
    }
}

/// Transient pointer gesture owned by the selection tool.
#[derive(Clone, Debug)]
pub enum SelectionGesture {
    /// Point item selection.
    Item { position: Point },
    /// Rectangular marquee from `start` to the current pointer position.
    Rect { start: Point, current: Point },
    /// Freeform lasso path.
    Lasso { points: Vec<Point> },
}

#[derive(Clone, Debug)]
struct SelectionDrag {
    start: Point,
    current: Point,
    phase: DragPhase,
    area_owner: Entity,
    area_shape: SelectionAreaShape,
    entities: Vec<DraggedEntity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragPhase {
    Pending,
    Dragging,
}

#[derive(Clone, Debug)]
struct DraggedEntity {
    entity: Entity,
    owner: Entity,
    root_from_entity: Affine,
}

pub(crate) fn register_selection_systems(world: &mut World) {
    MessageRegistry::register_message::<SelectionMessage>(world);
    world.init_resource::<SelectionState>();
}

pub(crate) fn install_selection_systems(schedule: &mut Schedule) {
    schedule.add_systems(process_selection_msg.in_set(QuillUpdateSet::ToolMessages));
}

/// Query for selectable leaf elements in the document.
type Drawables<'w, 's> =
    Query<'w, 's, (), Or<(With<StrokeData>, With<ImageElement>, With<TextElement>)>>;

#[expect(
    clippy::too_many_arguments,
    reason = "Bevy injects independent resource and component borrows into this system"
)]
pub(crate) fn process_selection_msg(
    mut commands: Commands,
    mut messages: MessageReader<SelectionMessage>,
    mode: Res<CanvasMode>,
    root: Res<DocumentRootEntity>,
    mut selection: ResMut<SelectionState>,
    children: Query<&Children>,
    pages: Query<&Page>,
    parents: Query<(Option<&CanvasTransform>, Option<&ChildOf>)>,
    bounds: Query<&RectBound>,
    areas: Query<&SelectionArea>,
    drawables: Drawables<'_, '_>,
) {
    let Tool::Selection(tool) = mode.tool else {
        for _ in messages.read() {}
        selection.cancel_gesture();
        return;
    };

    for message in messages.read() {
        match message.input {
            CanvasInput::PointerEnter(_) => {}
            CanvasInput::PointerLeave => selection.cancel_gesture(),
            CanvasInput::PointerDown(input) => {
                let root_position = input_to_vec2(input);
                if begin_drag_if_area_hit(
                    &mut selection,
                    root_position,
                    &areas,
                    &parents,
                    &drawables,
                ) {
                    continue;
                }

                let owner =
                    target_owner_at_query(root.0, root_position, &children, &pages, &parents);
                selection.owner = Some(owner);
                selection.begin_gesture(tool.mode, root_position);
                update_selection_area_for_gesture(
                    &mut commands,
                    &mut selection,
                    owner,
                    tool,
                    &children,
                    &pages,
                    &parents,
                    &bounds,
                    &drawables,
                );
            }
            CanvasInput::PointerMove(input) => {
                let root_position = input_to_vec2(input);
                if update_drag(&mut commands, &mut selection, tool, root_position, &parents) {
                    continue;
                }

                if let Some(owner) = selection.owner {
                    selection.update_gesture(root_position);
                    update_selection_area_for_gesture(
                        &mut commands,
                        &mut selection,
                        owner,
                        tool,
                        &children,
                        &pages,
                        &parents,
                        &bounds,
                        &drawables,
                    );
                }
            }
            CanvasInput::PointerUp(input) => {
                let root_position = input_to_vec2(input);
                if finish_drag(
                    &mut commands,
                    root.0,
                    &mut selection,
                    root_position,
                    &children,
                    &pages,
                    &parents,
                ) {
                    continue;
                }

                if let Some(owner) = selection.owner {
                    selection.update_gesture(root_position);
                    let Some(gesture) = selection.finish_gesture() else {
                        selection.owner = None;
                        continue;
                    };
                    let hits = hit_selection(
                        owner,
                        &gesture,
                        tool.hit_radius,
                        &children,
                        &pages,
                        &parents,
                        &bounds,
                        &drawables,
                    );
                    selection.apply(tool.op, hits.iter().copied());
                    finalize_selection_area(
                        &mut commands,
                        &mut selection,
                        owner,
                        tool,
                        &gesture,
                        &hits,
                        &parents,
                        &bounds,
                    );
                }
                selection.owner = None;
            }
        }
    }
}

fn begin_drag_if_area_hit(
    selection: &mut SelectionState,
    position: Point,
    areas: &Query<&SelectionArea>,
    parents: &Query<(Option<&CanvasTransform>, Option<&ChildOf>)>,
    drawables: &Drawables<'_, '_>,
) -> bool {
    if selection.selected.is_empty() {
        return false;
    }
    let Some(area_entity) = selection.area_entity else {
        return false;
    };
    let Ok(area) = areas.get(area_entity) else {
        return false;
    };
    if !area.shape.contains(position) {
        return false;
    }

    let entities = selection
        .selected
        .iter()
        .copied()
        .filter(|entity| drawables.get(*entity).is_ok())
        .filter_map(|entity| {
            let root_from_entity = root_from_entity_query(entity, parents)?;
            let owner = parents
                .get(entity)
                .ok()
                .and_then(|(_, parent)| parent.map(|parent| parent.0))?;
            Some(DraggedEntity {
                entity,
                owner,
                root_from_entity,
            })
        })
        .collect::<Vec<_>>();
    if entities.is_empty() {
        return false;
    }

    selection.gesture = None;
    selection.owner = None;
    selection.drag = Some(SelectionDrag {
        start: position,
        current: position,
        phase: DragPhase::Pending,
        area_owner: area.owner,
        area_shape: area.shape.clone(),
        entities,
    });
    true
}

fn update_drag(
    commands: &mut Commands,
    selection: &mut SelectionState,
    tool: SelectionTool,
    position: Point,
    parents: &Query<(Option<&CanvasTransform>, Option<&ChildOf>)>,
) -> bool {
    let Some(drag) = selection.drag.as_mut() else {
        return false;
    };

    drag.current = position;
    let delta = position - drag.start;
    if drag.phase == DragPhase::Pending && delta.hypot() < f64::from(tool.hit_radius.max(1.0)) {
        return true;
    }

    drag.phase = DragPhase::Dragging;
    move_dragged_entities(commands, drag, delta, parents);
    update_drag_area(
        commands,
        selection.area_entity,
        drag.area_owner,
        &drag.area_shape,
        delta,
    );
    true
}

fn finish_drag(
    commands: &mut Commands,
    root: Entity,
    selection: &mut SelectionState,
    position: Point,
    children: &Query<&Children>,
    pages: &Query<&Page>,
    parents: &Query<(Option<&CanvasTransform>, Option<&ChildOf>)>,
) -> bool {
    let Some(drag) = selection.drag.take() else {
        return false;
    };

    if drag.phase == DragPhase::Dragging {
        let delta = position - drag.start;
        let target_owner = target_owner_at_query(root, position, children, pages, parents);
        finalize_dragged_entities(commands, &drag, delta, target_owner, parents);
        update_drag_area(
            commands,
            selection.area_entity,
            target_owner,
            &drag.area_shape,
            delta,
        );
    }
    true
}

fn move_dragged_entities(
    commands: &mut Commands,
    drag: &SelectionDrag,
    delta: Vec2,
    parents: &Query<(Option<&CanvasTransform>, Option<&ChildOf>)>,
) {
    for dragged in &drag.entities {
        let Some(root_from_owner) = root_from_entity_query(dragged.owner, parents) else {
            continue;
        };
        let root_from_entity = Affine::translate(delta) * dragged.root_from_entity;
        let owner_from_entity = root_from_owner.inverse() * root_from_entity;
        commands.entity(dragged.entity).insert(CanvasTransform {
            affine: owner_from_entity,
        });
    }
}

fn finalize_dragged_entities(
    commands: &mut Commands,
    drag: &SelectionDrag,
    delta: Vec2,
    target_owner: Entity,
    parents: &Query<(Option<&CanvasTransform>, Option<&ChildOf>)>,
) {
    let Some(root_from_target) = root_from_entity_query(target_owner, parents) else {
        return;
    };
    for dragged in &drag.entities {
        let root_from_entity = Affine::translate(delta) * dragged.root_from_entity;
        let owner_from_entity = root_from_target.inverse() * root_from_entity;
        commands.entity(dragged.entity).insert((
            ChildOf(target_owner),
            CanvasTransform {
                affine: owner_from_entity,
            },
        ));
    }
}

fn update_drag_area(
    commands: &mut Commands,
    area_entity: Option<Entity>,
    owner: Entity,
    shape: &SelectionAreaShape,
    delta: Vec2,
) {
    let Some(area_entity) = area_entity else {
        return;
    };
    commands.entity(area_entity).insert(SelectionArea {
        owner,
        shape: shape.translated(delta),
    });
}

#[allow(clippy::too_many_arguments)]
fn update_selection_area_for_gesture(
    commands: &mut Commands,
    selection: &mut SelectionState,
    owner: Entity,
    tool: SelectionTool,
    children: &Query<&Children>,
    pages: &Query<&Page>,
    parents: &Query<(Option<&CanvasTransform>, Option<&ChildOf>)>,
    bounds: &Query<&RectBound>,
    drawables: &Drawables<'_, '_>,
) {
    let Some(gesture) = selection.gesture.as_ref() else {
        return;
    };
    let shape = match gesture {
        SelectionGesture::Item { .. } => hit_selection(
            owner,
            gesture,
            tool.hit_radius,
            children,
            pages,
            parents,
            bounds,
            drawables,
        )
        .first()
        .and_then(|entity| bounds.get(*entity).ok().map(|bound| (*entity, bound)))
        .and_then(|(entity, bound)| drawable_root_bound(entity, parents, bound))
        .map(SelectionAreaShape::ItemRect),
        SelectionGesture::Rect { start, current } => {
            Some(SelectionAreaShape::Rect(rect_from_points(*start, *current)))
        }
        SelectionGesture::Lasso { points } => {
            Some(SelectionAreaShape::Lasso(enclosed_lasso_points(points)))
        }
    };

    if let Some(shape) = shape {
        set_selection_area(commands, selection, owner, shape);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Selection finalization combines the gesture, hits, and owner-space bounds"
)]
fn finalize_selection_area(
    commands: &mut Commands,
    selection: &mut SelectionState,
    owner: Entity,
    tool: SelectionTool,
    gesture: &SelectionGesture,
    hits: &[Entity],
    parents: &Query<(Option<&CanvasTransform>, Option<&ChildOf>)>,
    bounds: &Query<&RectBound>,
) {
    if tool.op != SelectionOp::Replace || hits.is_empty() {
        clear_selection_area(commands, selection);
        return;
    }

    let shape = match gesture {
        SelectionGesture::Item { .. } => hits
            .first()
            .and_then(|entity| bounds.get(*entity).ok().map(|bound| (*entity, bound)))
            .and_then(|(entity, bound)| drawable_root_bound(entity, parents, bound))
            .map(SelectionAreaShape::ItemRect),
        SelectionGesture::Rect { start, current } => {
            Some(SelectionAreaShape::Rect(rect_from_points(*start, *current)))
        }
        SelectionGesture::Lasso { points } => {
            Some(SelectionAreaShape::Lasso(enclosed_lasso_points(points)))
        }
    };

    if let Some(shape) = shape {
        set_selection_area(commands, selection, owner, shape);
    } else {
        clear_selection_area(commands, selection);
    }
}

fn set_selection_area(
    commands: &mut Commands,
    selection: &mut SelectionState,
    owner: Entity,
    shape: SelectionAreaShape,
) {
    let area = SelectionArea { owner, shape };
    if let Some(entity) = selection.area_entity {
        commands.entity(entity).insert(area);
    } else {
        selection.area_entity = Some(commands.spawn(area).id());
    }
}

fn clear_selection_area(commands: &mut Commands, selection: &mut SelectionState) {
    if let Some(entity) = selection.area_entity.take() {
        commands.entity(entity).despawn();
    }
}

fn drawable_root_bound(
    entity: Entity,
    parents: &Query<(Option<&CanvasTransform>, Option<&ChildOf>)>,
    bound: &RectBound,
) -> Option<Rect> {
    let root_from_entity = root_from_entity_query(entity, parents)?;
    Some(
        bound
            .world(CanvasTransform {
                affine: root_from_entity,
            })
            .rect,
    )
}

impl SelectionAreaShape {
    fn contains(&self, point: Point) -> bool {
        match self {
            Self::ItemRect(rect) | Self::Rect(rect) => rect.contains(point),
            Self::Lasso(points) => point_in_polygon(point, points),
        }
    }

    fn translated(&self, delta: Vec2) -> Self {
        match self {
            Self::ItemRect(rect) => Self::ItemRect(translate_rect(*rect, delta)),
            Self::Rect(rect) => Self::Rect(translate_rect(*rect, delta)),
            Self::Lasso(points) => Self::Lasso(points.iter().map(|point| *point + delta).collect()),
        }
    }
}

fn translate_rect(rect: Rect, delta: Vec2) -> Rect {
    Rect::new(
        rect.x0 + delta.x,
        rect.y0 + delta.y,
        rect.x1 + delta.x,
        rect.y1 + delta.y,
    )
}

fn input_to_vec2(input: Input) -> Point {
    Point::new(f64::from(input.x()), f64::from(input.y()))
}

#[expect(
    clippy::too_many_arguments,
    reason = "Selection traversal reads hierarchy, transforms, bounds, and drawable membership"
)]
fn hit_selection(
    owner: Entity,
    gesture: &SelectionGesture,
    hit_radius: f32,
    children: &Query<&Children>,
    pages: &Query<&Page>,
    parents: &Query<(Option<&CanvasTransform>, Option<&ChildOf>)>,
    bounds: &Query<&RectBound>,
    drawables: &Drawables<'_, '_>,
) -> Vec<Entity> {
    let mut candidates = Vec::new();
    collect_drawable_bounds(
        owner,
        children,
        pages,
        parents,
        bounds,
        drawables,
        &mut candidates,
    );

    match gesture {
        SelectionGesture::Item { position } => candidates
            .iter()
            .rev()
            .find(|(_, bound)| {
                bound
                    .intersects_round(RoundBound::new(*position, f64::from(hit_radius.max(0.0))))
                    .is_some()
            })
            .map(|(entity, _)| *entity)
            .into_iter()
            .collect(),
        SelectionGesture::Rect { start, current } => {
            let rect = rect_from_points(*start, *current);
            candidates
                .into_iter()
                .filter(|(_, bound)| bound.intersects_rect(rect).is_some())
                .map(|(entity, _)| entity)
                .collect()
        }
        SelectionGesture::Lasso { points } => {
            let Some(lasso_bounds) = bounds_for_points(points) else {
                return Vec::new();
            };
            candidates
                .into_iter()
                .filter(|(_, bound)| {
                    bound.intersects_rect(lasso_bounds).is_some()
                        && lasso_intersects_rect(points, bound.rect)
                })
                .map(|(entity, _)| entity)
                .collect()
        }
    }
}

fn collect_drawable_bounds(
    owner: Entity,
    children: &Query<&Children>,
    pages: &Query<&Page>,
    parents: &Query<(Option<&CanvasTransform>, Option<&ChildOf>)>,
    bounds: &Query<&RectBound>,
    drawables: &Drawables<'_, '_>,
    out: &mut Vec<(Entity, RectBound)>,
) {
    let Ok(owner_children) = children.get(owner) else {
        return;
    };

    for child in owner_children.iter() {
        if pages.get(child).is_ok() {
            collect_drawable_bounds(child, children, pages, parents, bounds, drawables, out);
            continue;
        }

        if drawables.get(child).is_ok()
            && let Ok(bound) = bounds.get(child)
            && let Some(root_bound) = drawable_root_bound(child, parents, bound)
        {
            out.push((child, RectBound::from_rect(root_bound)));
        }
    }
}

fn rect_from_points(a: Point, b: Point) -> Rect {
    rect_from_min_max(
        Point::new(a.x.min(b.x), a.y.min(b.y)),
        Point::new(a.x.max(b.x), a.y.max(b.y)),
    )
}

fn bounds_for_points(points: &[Point]) -> Option<Rect> {
    let mut points = points.iter().copied();
    let first = points.next()?;
    Some(points.fold(Rect::from_points(first, first), |rect, point| {
        rect.union_pt(point)
    }))
}

fn lasso_intersects_rect(points: &[Point], rect: Rect) -> bool {
    if points.len() < 3 {
        return false;
    }
    let closed_points = enclosed_lasso_points(points);
    rect_corners(rect)
        .into_iter()
        .any(|corner| point_in_polygon(corner, points))
        || points.iter().copied().any(|point| rect.contains(point))
        || closed_points
            .windows(2)
            .any(|segment| segment_intersects_rect(segment[0], segment[1], rect))
}

fn enclosed_lasso_points(points: &[Point]) -> Vec<Point> {
    let mut enclosed = points.to_vec();
    if let (Some(first), Some(last)) = (enclosed.first().copied(), enclosed.last().copied())
        && first != last
    {
        enclosed.push(first);
    }
    enclosed
}

fn rect_corners(rect: Rect) -> [Point; 4] {
    [
        Point::new(rect.x0, rect.y0),
        Point::new(rect.x0, rect.y1),
        Point::new(rect.x1, rect.y0),
        Point::new(rect.x1, rect.y1),
    ]
}

fn point_in_polygon(point: Point, polygon: &[Point]) -> bool {
    let mut inside = false;
    let mut previous = *polygon.last().unwrap_or(&point);
    for current in polygon.iter().copied() {
        let crosses = (current.y > point.y) != (previous.y > point.y);
        if crosses {
            let x = (previous.x - current.x) * (point.y - current.y) / (previous.y - current.y)
                + current.x;
            if point.x < x {
                inside = !inside;
            }
        }
        previous = current;
    }
    inside
}

fn segment_intersects_rect(a: Point, b: Point, bounds: Rect) -> bool {
    let delta = b - a;
    let mut t_min: f64 = 0.0;
    let mut t_max: f64 = 1.0;

    slab_intersect(a.x, delta.x, bounds.x0, bounds.x1, &mut t_min, &mut t_max)
        && slab_intersect(a.y, delta.y, bounds.y0, bounds.y1, &mut t_min, &mut t_max)
}

fn slab_intersect(
    start: f64,
    delta: f64,
    min: f64,
    max: f64,
    t_min: &mut f64,
    t_max: &mut f64,
) -> bool {
    if delta.abs() <= f64::EPSILON {
        return start >= min && start <= max;
    }
    let inv_delta = 1.0 / delta;
    let mut near = (min - start) * inv_delta;
    let mut far = (max - start) * inv_delta;
    if near > far {
        std::mem::swap(&mut near, &mut far);
    }

    *t_min = (*t_min).max(near);
    *t_max = (*t_max).min(far);
    *t_min <= *t_max
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_clears_old_selection_and_sets_primary() {
        let old = Entity::from_raw_u32(1).unwrap();
        let new = Entity::from_raw_u32(2).unwrap();
        let mut state = SelectionState::default();
        state.apply(SelectionOp::Add, [old]);

        state.apply(SelectionOp::Replace, [new]);

        assert!(!state.contains(old));
        assert!(state.contains(new));
        assert_eq!(state.primary(), Some(new));
    }

    #[test]
    fn add_preserves_old_selection_and_updates_primary() {
        let first = Entity::from_raw_u32(1).unwrap();
        let second = Entity::from_raw_u32(2).unwrap();
        let mut state = SelectionState::default();
        state.apply(SelectionOp::Replace, [first]);

        state.apply(SelectionOp::Add, [second]);

        assert!(state.contains(first));
        assert!(state.contains(second));
        assert_eq!(state.primary(), Some(second));
    }

    #[test]
    fn toggle_removes_selected_hits_and_adds_unselected_hits() {
        let first = Entity::from_raw_u32(1).unwrap();
        let second = Entity::from_raw_u32(2).unwrap();
        let mut state = SelectionState::default();
        state.apply(SelectionOp::Replace, [first]);

        state.apply(SelectionOp::Toggle, [first, second]);

        assert!(!state.contains(first));
        assert!(state.contains(second));
        assert_eq!(state.primary(), Some(second));
    }

    #[test]
    fn subtract_removes_hits_and_repairs_primary() {
        let first = Entity::from_raw_u32(1).unwrap();
        let second = Entity::from_raw_u32(2).unwrap();
        let mut state = SelectionState::default();
        state.apply(SelectionOp::Add, [first]);
        state.apply(SelectionOp::Add, [second]);

        state.apply(SelectionOp::Subtract, [second]);

        assert!(state.contains(first));
        assert!(!state.contains(second));
        assert_eq!(state.primary(), Some(first));
    }

    #[test]
    fn replace_entity_transfers_selected_membership() {
        let old = Entity::from_raw_u32(1).unwrap();
        let first = Entity::from_raw_u32(2).unwrap();
        let second = Entity::from_raw_u32(3).unwrap();
        let mut state = SelectionState::default();
        state.apply(SelectionOp::Replace, [old]);

        state.replace_entity(old, [first, second]);

        assert!(!state.contains(old));
        assert!(state.contains(first));
        assert!(state.contains(second));
        assert_eq!(state.primary(), Some(first));
    }
}
