//! Selection tool state and input handling.
//!
//! Selection is stored as unordered membership. Canvas draw order is only
//! relevant to future hit testing, where topmost elements should win clicks.

use std::collections::HashSet;

use bevy_ecs::message::MessageRegistry;
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule};
use peniko::kurbo::Point;

use crate::{
    canvas::CanvasMode,
    element::{SelectionMessage, Tool},
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

/// Canvas-wide selection state.
#[derive(Resource, Debug, Default)]
pub struct SelectionState {
    selected: HashSet<Entity>,
    primary: Option<Entity>,
    gesture: Option<SelectionGesture>,
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
            SelectionMode::Item => None,
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
    /// Rectangular marquee from `start` to the current pointer position.
    Rect { start: Point, current: Point },
    /// Freeform lasso path.
    Lasso { points: Vec<Point> },
}

pub(crate) fn register_selection_systems(world: &mut World) {
    MessageRegistry::register_message::<SelectionMessage>(world);
    world.init_resource::<SelectionState>();
}

pub(crate) fn install_selection_systems(schedule: &mut Schedule) {
    schedule.add_systems(process_selection_msg.in_set(QuillUpdateSet::ToolMessages));
}

pub(crate) fn process_selection_msg(
    mut messages: MessageReader<SelectionMessage>,
    mode: Res<CanvasMode>,
    mut selection: ResMut<SelectionState>,
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
                selection.begin_gesture(tool.mode, input_to_vec2(input));
            }
            CanvasInput::PointerMove(input) => {
                selection.update_gesture(input_to_vec2(input));
            }
            CanvasInput::PointerUp(_) => {
                let _finished = selection.finish_gesture();
                selection.apply(tool.op, []);
            }
        }
    }
}

fn input_to_vec2(input: Input) -> Point {
    Point::new(f64::from(input.x()), f64::from(input.y()))
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
