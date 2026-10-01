//! Document root, hierarchy ownership, and page hit testing helpers.

use bevy_ecs::{
    hierarchy::{ChildOf, Children},
    prelude::*,
    world::World,
};
use peniko::kurbo::{Affine, Point};

use crate::{element::page::Page, geom::CanvasTransform};

/// Singleton root entity for the whole canvas document.
#[derive(Component, Clone, Copy, Debug)]
pub struct DocumentRoot;

/// Resource pointing at the singleton document root entity.
#[derive(Resource, Clone, Copy, Debug)]
pub struct DocumentRootEntity(pub Entity);

/// Spawn the document root and store its entity id in the world.
pub fn init_document_root(world: &mut World) {
    let root = world.spawn(DocumentRoot).id();
    world.insert_resource(DocumentRootEntity(root));
}

/// Return true when `entity` can own document children.
pub fn is_owner(world: &World, entity: Entity) -> bool {
    world.get::<DocumentRoot>(entity).is_some() || world.get::<Page>(entity).is_some()
}

/// Resolve an optional command owner, using the document root for `None`.
pub fn resolve_owner(world: &World, owner: Option<Entity>) -> Option<Entity> {
    let entity = owner.unwrap_or(world.resource::<DocumentRootEntity>().0);
    is_owner(world, entity).then_some(entity)
}

/// Compose the transform from `entity` local coordinates into root coordinates.
pub fn root_from_entity(world: &World, entity: Entity) -> Option<Affine> {
    let mut current = entity;
    let mut root_from_current = Affine::IDENTITY;

    // Prevent malformed relationship cycles from hanging editing or rendering.
    for _ in 0..512 {
        if let Some(transform) = world.get::<CanvasTransform>(current) {
            root_from_current = transform.affine * root_from_current;
        }

        let Some(parent) = world.get::<ChildOf>(current) else {
            return Some(root_from_current);
        };
        current = parent.0;
    }

    None
}

/// Compose the transform from `entity` local coordinates into root coordinates
/// using normal system queries.
pub fn root_from_entity_query(
    entity: Entity,
    parents: &Query<(Option<&CanvasTransform>, Option<&ChildOf>)>,
) -> Option<Affine> {
    let mut current = entity;
    let mut root_from_current = Affine::IDENTITY;

    for _ in 0..512 {
        let Ok((transform, parent)) = parents.get(current) else {
            return None;
        };
        if let Some(transform) = transform {
            root_from_current = transform.affine * root_from_current;
        }

        let Some(parent) = parent else {
            return Some(root_from_current);
        };
        current = parent.0;
    }

    None
}

/// Convert a root-coordinate point into an owner's local coordinates.
pub fn owner_point_from_root(world: &World, owner: Entity, point: Point) -> Option<Point> {
    root_from_entity(world, owner).map(|root_from_owner| root_from_owner.inverse() * point)
}

/// Convert a root-coordinate point into an owner's local coordinates using
/// normal system queries.
pub fn owner_point_from_root_query(
    owner: Entity,
    point: Point,
    parents: &Query<(Option<&CanvasTransform>, Option<&ChildOf>)>,
) -> Option<Point> {
    root_from_entity_query(owner, parents).map(|root_from_owner| root_from_owner.inverse() * point)
}

/// Return the deepest topmost page under `point`, or the document root.
pub fn target_owner_at(world: &World, point: Point) -> Entity {
    let root = world.resource::<DocumentRootEntity>().0;
    deepest_page_in_owner(world, root, Affine::IDENTITY, point).unwrap_or(root)
}

/// Return the deepest topmost page under `point`, or the document root, using
/// normal system queries.
pub fn target_owner_at_query(
    root: Entity,
    point: Point,
    children: &Query<&Children>,
    pages: &Query<&Page>,
    parents: &Query<(Option<&CanvasTransform>, Option<&ChildOf>)>,
) -> Entity {
    deepest_page_in_owner_query(root, Affine::IDENTITY, point, children, pages, parents)
        .unwrap_or(root)
}

fn deepest_page_in_owner(
    world: &World,
    owner: Entity,
    root_from_owner: Affine,
    point: Point,
) -> Option<Entity> {
    let owner_children = world.get::<Children>(owner)?;

    for child in owner_children.iter().rev() {
        let Some(page) = world.get::<Page>(child) else {
            continue;
        };
        let transform = world
            .get::<CanvasTransform>(child)
            .copied()
            .unwrap_or_default();
        let root_from_page = root_from_owner * transform.affine;
        let page_point = root_from_page.inverse() * point;

        if page.rect().contains(page_point) {
            return deepest_page_in_owner(world, child, root_from_page, point).or(Some(child));
        }
    }

    None
}

fn deepest_page_in_owner_query(
    owner: Entity,
    root_from_owner: Affine,
    point: Point,
    children: &Query<&Children>,
    pages: &Query<&Page>,
    parents: &Query<(Option<&CanvasTransform>, Option<&ChildOf>)>,
) -> Option<Entity> {
    let owner_children = children.get(owner).ok()?;

    for child in owner_children.iter().rev() {
        let Ok(page) = pages.get(child) else {
            continue;
        };
        let Ok((transform, _)) = parents.get(child) else {
            continue;
        };
        let root_from_page = root_from_owner * transform.copied().unwrap_or_default().affine;
        let page_point = root_from_page.inverse() * point;

        if page.rect().contains(page_point) {
            return deepest_page_in_owner_query(
                child,
                root_from_page,
                point,
                children,
                pages,
                parents,
            )
            .or(Some(child));
        }
    }

    None
}
