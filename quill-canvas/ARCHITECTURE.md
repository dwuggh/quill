# Quill Canvas Architecture

This document is the implementation contract for the `quill-canvas` crate.
It describes the canvas scene model, input routing, ownership, ordering,
assets, rendering boundary, and undo/redo behavior expected by renderer and app
crates.

The design follows Bevy ECS conventions. Scene data is stored as entities,
components, resources, typed asset collections, and Bevy hierarchy
relationships. Renderer crates read this model and translate it to their own
draw APIs.

## Goals

- Keep `quill-canvas` as the authoritative document scene model.
- Keep renderer-specific APIs out of element data, except for shared data types
  already used across the workspace such as `peniko`.
- Use Bevy's existing hierarchy and asset patterns instead of custom ownership
  and resource maps.
- Keep v1 focused on pages, strokes, images, laid-out glyph text, selection,
  eraser, page backgrounds, and command-log undo/redo.
- Keep the current message-based input routing for v1.

## Coordinate System

All canvas coordinate systems use the normal document/UI convention:

- origin at the top-left corner;
- positive X goes right;
- positive Y goes down.

`CanvasInput` carries root/document coordinates. App crates are responsible for
converting host widget/window coordinates into root coordinates before sending
input to `quill-canvas`.

Each owned entity stores geometry in its owner's local coordinate system. Its
`CanvasTransform` maps from entity-local coordinates into owner-local
coordinates. Pages also use `CanvasTransform`, so nested pages and moved pages
work through the same transform path as other elements.

Page-local coordinates are also top-left, Y-down. A page's local bounds are:

```rust
Rect::from_origin_size((0.0, 0.0), page.size)
```

The target architecture does not use the current centered, Y-up widget transform
for document data. Renderer/app code should not flip canvas Y globally.

## ECS Ownership And Order

The canvas world contains exactly one document root entity.

```rust
#[derive(Component, Debug)]
pub struct DocumentRoot;

#[derive(Resource, Clone, Copy, Debug)]
pub struct DocumentRootEntity(pub Entity);
```

`init_canvas_world` spawns the root entity and stores it in
`DocumentRootEntity`. Systems and renderers use the resource instead of querying
for a singleton root every time.

Ownership and draw order use Bevy hierarchy:

```rust
use bevy_ecs::hierarchy::{ChildOf, Children};
```

Rules:

- `DocumentRoot` and `Page` are owners.
- Pages and drawable elements are children of exactly one owner.
- The owner's `Children` order is the document order for its direct children.
- New children append to the end of the owner list in v1.
- Page deletion cascades through Bevy's `linked_spawn` hierarchy behavior.
- Systems should modify ownership by inserting/removing `ChildOf`, not by
  directly editing `Children`.

The old `CanvasOrder { strokes: Vec<Entity> }` resource is a temporary v0
structure and should be removed when this architecture is implemented. Rendering
and hit testing must walk `DocumentRootEntity -> Children`, not stroke-specific
order lists.

## Assets

`quill-canvas` should add direct dependencies on Bevy asset support rather than
depending on the full `bevy` crate.

Expected crate dependencies:

```toml
bevy_asset.workspace = true
bevy_reflect.workspace = true
bitflags = "2"
```

Asset resources:

```rust
use bevy_asset::{Asset, Assets, Handle};
use bevy_reflect::TypePath;
use peniko::{FontData, ImageData};

#[derive(Asset, TypePath, Clone, Debug)]
pub struct CanvasImage {
    pub data: ImageData,
}

#[derive(Asset, TypePath, Clone, Debug)]
pub struct CanvasFont {
    pub data: FontData,
}
```

`init_canvas_world` initializes:

```rust
world.init_resource::<Assets<CanvasImage>>();
world.init_resource::<Assets<CanvasFont>>();
```

Elements store `Handle<CanvasImage>` or `Handle<CanvasFont>`. This keeps image
and font bytes shared while entities remain small.

## Pages And Backgrounds

`PageConfig` must contain an explicit size and background:

```rust
pub struct PageConfig {
    pub size: Size,
    pub background: PageBackground,
}

#[derive(Component, Clone, Debug)]
pub struct Page {
    pub size: Size,
    pub background: PageBackground,
}
```

`PageBackground` is the page template. Do not introduce a separate "template"
concept. A blank page is a background with only a base fill.

The background owns the paper fill brush and optional background primitives:

```rust
pub enum PageBackground {
    Blank {
        fill: peniko::Brush,
    },
    DisplayList {
        fill: peniko::Brush,
        items: Vec<PageBackgroundItem>,
    },
    Image {
        fill: peniko::Brush,
        image: Handle<CanvasImage>,
        transform: Affine,
    },
    Procedural {
        fill: peniko::Brush,
        source: Arc<dyn PageBackgroundSource>,
    },
}

pub trait PageBackgroundSource: std::fmt::Debug + Send + Sync + 'static {
    fn layout(&self, size: Size) -> Vec<PageBackgroundItem>;
}
```

`PageBackgroundSource` returns a display list. It must not receive a renderer or
draw directly.

For the `Image` variant, `transform` maps the image's natural pixel rectangle
into page-local coordinates. Stretch-to-page is expressed as a scale transform
from `(image.width, image.height)` to `page.size`.

Background primitives are page-local:

```rust
pub enum PageBackgroundItem {
    Line {
        from: Point,
        to: Point,
        stroke: kurbo::Stroke,
        brush: peniko::Brush,
    },
    Rect {
        rect: Rect,
        brush: peniko::Brush,
        fill_rule: peniko::Fill,
    },
    Path {
        path: BezPath,
        brush: peniko::Brush,
        style: peniko::Style,
    },
    Image {
        image: Handle<CanvasImage>,
        transform: Affine,
    },
}
```

The page background is rendered as part of the page, before the page's ordinary
children. It is not a child entity and does not participate in child order.

Editing view does not clip page children by default. Export or presentation
modes may add clipping later.

## Drawable Elements

V1 drawable entities are strokes, images, and laid-out glyph text.

### Strokes

Strokes keep the existing `StrokeData` centerline model and outline generation
in `quill-stroke`. The stroke's points are stored in the local coordinate space
of its owner page/root.

When a stroke is created by pointer input:

1. hit-test pages in root coordinates on pointer down;
2. latch the deepest hit page, or root if no page is hit;
3. convert each root-coordinate input sample into the latched owner's local
   coordinates;
4. append the stroke entity under that owner using `ChildOf`.

### Images

```rust
#[derive(Component, Clone, Debug)]
pub struct ImageElement {
    pub image: Handle<CanvasImage>,
    pub size: Size,
}
```

The image's local content rectangle is:

```rust
Rect::from_origin_size((0.0, 0.0), image.size)
```

`CanvasTransform` places, scales, and rotates the image in owner-local
coordinates.

### Text

Text is not semantic source text in v1. It is an already-laid-out glyph run that
aligns with imaging's glyph API. No Parley, Masonry text context, shaping
callback, or text layout engine belongs in `quill-canvas` or
`quill-render-imaging` for this v1 model.

Use a local glyph type instead of importing imaging into `quill-canvas`:

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasGlyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

pub type CanvasNormalizedCoord = i16;
```

The text element stores the full glyph-run state needed by imaging:

```rust
#[derive(Component, Clone, Debug)]
pub struct TextElement {
    pub font: Handle<CanvasFont>,
    pub glyphs: Vec<CanvasGlyph>,
    pub font_size: f32,
    pub brush: peniko::Brush,
    pub style: peniko::Style,
    pub hint: bool,
    pub font_embolden: Vec2,
    pub glyph_transform: Option<Affine>,
    pub normalized_coords: Vec<CanvasNormalizedCoord>,
    pub bounds: Rect,
}
```

`bounds` is caller-provided local bounds for hit testing and selection. The
canvas does not infer accurate glyph bounds from glyph ids.

`CanvasTransform` places the glyph run in owner-local coordinates. The renderer
maps `CanvasGlyph` to `imaging::record::Glyph` at draw time.

## Bounds And Hit Testing

Use the existing `RectBound` and `RoundBound` broad-phase types.

- Pages have rectangular bounds from their explicit size.
- Images have rectangular bounds from their explicit size.
- Text uses caller-provided local bounds.
- Strokes keep point-block sub-bounds for eraser acceleration.

Hit testing works in root coordinates:

1. traverse from `DocumentRoot` through ordered `Children`;
2. compose transforms from root to each child;
3. test pages first for tool target selection;
4. choose the topmost/deepest page under the pointer;
5. if no page is hit, use the root as the operation target.

"No cross-page operation" means selection and eraser systems only inspect direct
drawable children of the latched target page/root. They do not affect sibling
pages or descendants inside nested child pages.

## Commands

Keep the current message-based `CanvasCommand` path. Expand it so document
mutations happen through commands that maintain hierarchy and undo records.

Expected v1 command shape:

```rust
pub enum CanvasCommand {
    SetTool(Tool),
    AddPage(AddPage),
    AddImage(AddImage),
    AddText(AddText),
    DeleteEntity(Entity),
    Undo,
    Redo,
}

pub struct AddPage {
    pub owner: Option<Entity>,
    pub config: PageConfig,
    pub transform: CanvasTransform,
}

pub struct AddImage {
    pub owner: Option<Entity>,
    pub image: Handle<CanvasImage>,
    pub size: Size,
    pub transform: CanvasTransform,
}

pub struct AddText {
    pub owner: Option<Entity>,
    pub text: TextElement,
    pub transform: CanvasTransform,
}
```

`owner: None` means `DocumentRootEntity`. Commands append new entities to the
owner's `Children` list by inserting `ChildOf(owner)`.

Direct ECS edits are still allowed for internal systems, but mutations that
change document content should go through helpers that update undo/redo and use
Bevy hierarchy APIs consistently.

## Input Routing And Tools

Keep v1 routing as typed Bevy messages:

- raw `CanvasInput`;
- active `CanvasMode { tool }`;
- `InputMessage<T>` routed to the active tool stream;
- tool systems consume their routed messages in `QuillUpdateSet::ToolMessages`.

The tool gesture is not an owned scene object. On pointer down, tool systems
hit-test pages to choose the operation target. The target page/root is latched
for the gesture so moves and pointer-up do not drift into another page.

### Stroke Tool

The stroke tool creates a new stroke child under the latched target owner.
Samples are converted into that owner's local coordinate space before they are
fed to the input modeler or stored as path points.

### Selection Tool

Keep the current selection enum:

```rust
pub enum SelectionMode {
    Item,
    Rect,
    Lasso,
}
```

All modes operate only on direct drawable children of the latched deepest page
or root. Selection state is UI state and is not recorded in undo/redo.

Item selection tests direct children in reverse child order so the topmost
drawable wins. Rect and lasso selection test direct children whose bounds
intersect the gesture region.

### Eraser Tool

`EraserConfig` includes target filtering:

```rust
bitflags::bitflags! {
    pub struct EraserTargets: u8 {
        const STROKES = 0b0000_0001;
        const IMAGES  = 0b0000_0010;
        const TEXT    = 0b0000_0100;
    }
}

pub struct EraserConfig {
    pub shape: EraserShape,
    pub size: f32,
    pub targets: EraserTargets,
}
```

Behavior:

- strokes use the existing point erase and split behavior;
- enabled image/text elements are deleted as whole objects when hit;
- erasing is limited to direct drawable children of the latched target page/root.

An eraser gesture records one undo command covering all edits produced by that
gesture.

## Rendering Boundary

`quill-render-imaging` reads the `quill-canvas` ECS world and emits imaging draw
commands.

Traversal:

1. get `DocumentRootEntity`;
2. traverse the root's `Children` in order;
3. when a page is visited:
   - compose its transform;
   - draw page fill/background in page-local coordinates;
   - traverse that page's `Children` in order;
4. when a drawable is visited:
   - draw strokes, images, or text using the composed transform;
5. drop renderer caches for entities no longer reachable from the root.

Text rendering is direct glyph-run rendering:

- look up `CanvasFont` from `Assets<CanvasFont>`;
- map `CanvasGlyph` to `imaging::record::Glyph`;
- call `Painter::glyphs(...).font_size(...).hint(...).glyph_transform(...).draw(...)`.

Image rendering:

- look up `CanvasImage` from `Assets<CanvasImage>`;
- use `Painter::draw_image(...)`;
- apply the composed transform and local element size.

Stroke rendering:

- continue using `SimpleStrokeOutline`;
- cache by entity;
- finished strokes can use `new_with_path`;
- active strokes must update from real `ModelUpdate` state.

## Undo And Redo

Undo/redo is a document-edit command log. Selection changes are not undoable.

```rust
#[derive(Resource, Default)]
pub struct UndoStack {
    undo: Vec<DocumentEdit>,
    redo: Vec<DocumentEdit>,
}

pub struct DocumentEdit {
    pub label: &'static str,
    pub before: Vec<EntitySnapshot>,
    pub after: Vec<EntitySnapshot>,
}
```

An `EntitySnapshot` stores the entity id and the document components needed to
restore that entity:

- `ChildOf`;
- `Children` when the entity is an owner;
- `CanvasTransform`;
- `Page`;
- `StrokeData`;
- `ImageElement`;
- `TextElement`;
- bounds components used for hit testing.

The command log preserves Bevy `Entity` ids within a live canvas session. When
undo restores a despawned entity, use identity-preserving Bevy APIs such as
`World::spawn_at` where practical. If an entity id cannot be restored, that is a
hard error for v1 rather than a silent remap.

Undo application:

1. capture the current state if needed for redo;
2. apply the `before` snapshots;
3. restore hierarchy by inserting `ChildOf` relationships;
4. remove entities that exist in `after` but not `before`;
5. push the edit to redo.

Redo applies `after` snapshots and removes entities that exist in `before` but
not `after`.

Gesture-level edits, such as stroke erasing, are recorded as one
`DocumentEdit`.

## Migration From Current Code

Implementation should proceed in these steps:

1. Add Bevy asset dependencies and define canvas image/font asset types.
2. Create `DocumentRoot` and `DocumentRootEntity` during world initialization.
3. Replace `CanvasOrder` insertion paths with Bevy `ChildOf` ownership.
4. Update stroke creation to latch the deepest hit page/root and store points in
   owner-local coordinates.
5. Add page size/background data and page hit testing.
6. Add `ImageElement` and `TextElement` components plus insertion commands.
7. Update selection and eraser systems to use deepest-page direct-child scope.
8. Rewrite `quill-render-imaging` traversal from root/children and remove the
   global center/y-flip transform.
9. Add command-log undo/redo around document edit commands and eraser gestures.
10. Remove v0-only code paths after their replacements are covered by tests.

## Required Tests

Unit tests should cover:

- canvas init creates exactly one `DocumentRoot` and `DocumentRootEntity`;
- `AddPage`, `AddImage`, and `AddText` append to the owner's `Children`;
- Bevy hierarchy cascades page deletion to its children;
- page hit testing chooses the topmost/deepest page;
- root-coordinate input is converted into target page-local stroke points;
- selection item/rect/lasso operate only on direct children of the latched page;
- eraser does not cross page boundaries;
- eraser target filtering splits strokes and deletes enabled images/text;
- text uses caller-provided bounds for hit testing;
- renderer traverses mixed pages/strokes/images/text in child order;
- page backgrounds render fill before background primitives and children;
- renderer no longer applies the old centered Y-up canvas transform;
- undo/redo preserves entity ids, hierarchy, order, and components for create,
  delete, and erase operations.
