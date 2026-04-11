# Quill Render Engine Design

## Introduction

Quill is a calligraphy render engine. Its core job is to turn a live stream of
pen, touch, or mouse input into a smoothed, brush-shaped, mesh-based stroke that
can be updated incrementally while drawing.

The engine should follow the architecture described by Google's Ink libraries,
but the implementation should be native to this crate:

```text
raw input
  -> modeled input
  -> brush behavior
  -> tip state
  -> analytic tip shape
  -> triangle mesh
  -> wgpu render
```

The current code can be treated as outdated scaffolding. The useful parts to
preserve are the Rust workspace layout, `kurbo` geometry types, and the idea of a
small playground app. Vello should not be used for now; rendering should go
through Quill's own mesh output and a direct `wgpu` renderer.

## Public Concepts

The v1 engine should expose a small set of durable concepts:

- `RawInput`: one raw sample from a device. It contains position, timestamp,
  pressure, tilt, orientation, tool type, and optional physical scale.
- `ModeledInput`: one smoothed sample on the modeled path. It contains position,
  timestamp, traveled distance, pressure, tilt, orientation, velocity,
  acceleration, and stable/unstable status.
- `Brush`: user-facing brush definition. It owns size, color, base tip
  parameters, and a deterministic behavior program.
- `BrushTip`: base brush footprint settings such as width scale, height scale,
  rotation, slant, pinch, and roundness.
- `TipState`: the concrete brush footprint at one modeled input. It contains
  center, dimensions, transform parameters, color/opacity, and is emitted
  through either the fixed or volatile tip-state stream.
- `TipShape`: analytic geometry derived from a `TipState`.
- `MutableStrokeMesh`: CPU-side live triangle mesh with stable geometry and a
  volatile tail.
- `PartitionedStrokeMesh`: immutable finished mesh split into render groups and
  partitions.
- `InProgressStroke`: the live incremental stroke object.
- `FinishedStroke`: an immutable packaged stroke ready for rendering, storage,
  and hit testing.

The main live API should be shaped around `InProgressStroke`:

```rust
impl InProgressStroke {
    pub fn enqueue_inputs(
        &mut self,
        real: InputBatch,
        predicted: InputBatch,
    ) -> Result<(), StrokeError>;

    pub fn update_shape(&mut self, current_time: f64) -> Result<StrokeUpdate, StrokeError>;

    pub fn finish(self) -> Result<FinishedStroke, StrokeError>;
}
```

`predicted_inputs` is replaceable on each update. The engine must be able to
roll back geometry affected by old predictions and rebuild the volatile tail
without changing already committed stable mesh partitions.

## Concrete Type Design

This type design is based on the local `../ink` reference repository while
staying idiomatic for Quill.

Defaults:

- Use `f64` for stroke-space geometry and time because Quill already uses
  `kurbo::Point` and `kurbo::Vec2`.
- Use `f32` for GPU vertex payloads, colors, packed labels, and texture data.
- Use `Option<T>` for optional input data instead of sentinel values.
- Use `lyon` for polygon fill triangulation and fallback geometry, not as the
  whole stroke extrusion model.

Current implementation map:

- `src/lib.rs`: crate module declarations and public re-exports.
- `src/error.rs`: `StrokeError` and error conversions.
- `src/input.rs`: `ToolType`, `RawInput`, `InputBatch`, and input validation.
- `src/model.rs`: `InputModelConfig`, `ModeledInput`, `InputModelerState`, and
  the minimal passthrough/sliding-window input modeler.
- `src/brush.rs`: brush, family, coat, paint, tip, color, and validation types.
- `src/shape.rs`: `TipState`, `TipShape`, circle representation, and initial
  input-to-tip-state mapping.
- `src/mesh.rs`: `StrokeVertex`, live/partitioned mesh containers, swept
  outline generation, and `lyon` tessellation.
- `src/stroke.rs`: `InProgressStroke`, `StrokeShapeBuilder`, `FinishedStroke`,
  `StrokeUpdate`, and the sample input generator used by tests and examples.
- `bin/src/main.rs`: binary entrypoint that starts the playground app.
- `bin/src/app.rs`: Winit application lifecycle, window events, resize, redraw,
  and surface recovery.
- `bin/src/input.rs`: pointer-state tracking and conversion from cursor events
  into `RawInput` batches.
- `bin/src/scene.rs`: the live playground scene graph, including one active
  `InProgressStroke` and completed `FinishedStroke` values.
- `bin/src/renderer.rs`: direct `wgpu` upload and draw path for finished
  partitions and live mutable meshes.

### Input Types

Raw input should be stored in a compact batch that validates all samples added
to a single stroke segment.

```rust
pub enum ToolType {
    Unknown,
    Mouse,
    Touch,
    Stylus,
}

pub struct RawInput {
    pub tool: ToolType,
    pub position: Point,
    /// Seconds since the start of the stroke.
    pub time: f64,
    /// Physical distance represented by one stroke unit, when known.
    pub stroke_unit_to_physical: Option<f64>,
    /// Normalized [0, 1] pressure, when reported.
    pub pressure: Option<f64>,
    /// Stylus tilt in radians, in [0, pi / 2], when reported.
    pub tilt: Option<f64>,
    /// Stylus orientation in radians, normalized to [0, 2pi), when reported.
    pub orientation: Option<f64>,
}

pub struct InputBatch {
    pub inputs: Vec<RawInput>,
    pub noise_seed: u32,
}

pub enum InputModelConfig {
    Passthrough,
    SlidingWindow {
        /// Symmetric smoothing window size, in seconds.
        window: f64,
        /// Maximum desired modeled input period, in seconds.
        upsample_period: f64,
        /// Minimum distance between consecutive modeled positions.
        position_epsilon: f64,
        /// Hard cap on modeled subdivisions per raw input interval.
        max_upsample_divisions: usize,
    },
}

pub struct InputMetrics {
    pub traveled_distance: f64,
    pub elapsed_time: f64,
}

pub struct ModeledInput {
    pub position: Point,
    pub velocity: Vec2,
    pub acceleration: Vec2,
    pub traveled_distance: f64,
    pub time: f64,
    pub pressure: Option<f64>,
    pub tilt: Option<f64>,
    pub orientation: Option<f64>,
}

pub struct InputModelerState {
    pub tool: ToolType,
    pub stroke_unit_to_physical: Option<f64>,
    pub stable_input_count: usize,
    pub real_input_count: usize,
    pub real_metrics: InputMetrics,
    pub full_metrics: InputMetrics,
    pub complete_time: f64,
    pub inputs_finished: bool,
}
```

`InputBatch` validation should require finite numeric values, nondecreasing
time within a batch, no repeated x/y/time triplets, a consistent tool and
optional-property format, pressure in `[0, 1]`, tilt in `[0, pi / 2]`,
orientation in `[0, 2pi)`, and positive physical scale when present.

### Brush Types

The brush model follows Ink's `Brush -> BrushFamily -> BrushCoat -> BrushTip`
split. Quill v1 can start with a single coat by default, but the data model
should support multiple coats because the mesh pipeline naturally builds one
render group per coat.

```rust
pub struct ColorRgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

pub struct Brush {
    pub family: BrushFamily,
    pub color: ColorRgba,
    pub size: f64,
    pub epsilon: f64,
}

pub struct BrushFamily {
    pub coats: Vec<BrushCoat>,
    pub input_model: InputModelConfig,
}

pub struct BrushCoat {
    pub tip: BrushTip,
    pub paint: BrushPaint,
}

pub struct BrushPaint {
    pub blend_mode: BlendMode,
}

pub enum BlendMode {
    SourceOver,
}

pub struct BrushTip {
    pub scale: Vec2,
    pub corner_rounding: f64,
    pub slant: f64,
    pub pinch: f64,
    pub rotation: f64,
    pub particle_gap_distance_scale: f64,
    pub particle_gap_duration: f64,
    pub behaviors: Vec<BrushBehavior>,
}
```

Validation should require positive finite `Brush::size`, positive finite
`Brush::epsilon`, `epsilon <= size`, finite tip values, nonnegative tip scales
with at least one nonzero component, `corner_rounding` and `pinch` in `[0, 1]`,
and `slant` in `[-pi / 2, pi / 2]`.

### Behavior Types

Brush behavior should be stored as a flattened post-order graph. Each node
consumes and produces nullable scalar values, except terminal nodes that apply
modifiers to target accumulators.

```rust
pub struct BrushBehavior {
    pub nodes: Vec<BehaviorNode>,
    pub developer_comment: String,
}

pub enum BehaviorNode {
    Source(SourceNode),
    Constant(f64),
    Noise(NoiseNode),
    ToolTypeFilter(ToolTypeFilterNode),
    Damping(DampingNode),
    Response(ResponseNode),
    Integral(IntegralNode),
    Binary(BinaryOp),
    Interpolation(Interpolation),
    Target(TargetNode),
    PolarTarget(PolarTargetNode),
}

pub struct SourceNode {
    pub source: BehaviorSource,
    pub out_of_range: OutOfRange,
    pub value_range: [f64; 2],
}

pub struct NoiseNode {
    pub seed: u32,
    pub vary_over: ProgressDomain,
    pub base_period: f64,
}

pub struct ToolTypeFilterNode {
    pub enabled: EnabledToolTypes,
}

pub struct DampingNode {
    pub damping_source: ProgressDomain,
    pub damping_gap: f64,
}

pub struct ResponseNode {
    pub curve: EasingCurve,
}

pub struct IntegralNode {
    pub integrate_over: ProgressDomain,
    pub out_of_range: OutOfRange,
    pub value_range: [f64; 2],
}

pub struct TargetNode {
    pub target: BehaviorTarget,
    pub modifier_range: [f64; 2],
}

pub struct PolarTargetNode {
    pub target: PolarBehaviorTarget,
    pub angle_range: [f64; 2],
    pub magnitude_range: [f64; 2],
}
```

Supporting behavior enums:

```rust
pub struct EnabledToolTypes {
    pub unknown: bool,
    pub mouse: bool,
    pub touch: bool,
    pub stylus: bool,
}

pub enum BehaviorSource {
    NormalizedPressure,
    Tilt,
    TiltX,
    TiltY,
    Orientation,
    OrientationAboutZero,
    SpeedInMultiplesOfBrushSizePerSecond,
    VelocityXInMultiplesOfBrushSizePerSecond,
    VelocityYInMultiplesOfBrushSizePerSecond,
    Direction,
    DirectionAboutZero,
    NormalizedDirectionX,
    NormalizedDirectionY,
    DistanceTraveledInMultiplesOfBrushSize,
    TimeOfInput,
    TimeFromInputToStrokeEnd,
    PredictedDistanceTraveledInMultiplesOfBrushSize,
    PredictedTimeElapsed,
    DistanceRemainingInMultiplesOfBrushSize,
    TimeSinceInput,
    TimeSinceStrokeEnd,
    AccelerationInMultiplesOfBrushSizePerSecondSquared,
    AccelerationXInMultiplesOfBrushSizePerSecondSquared,
    AccelerationYInMultiplesOfBrushSizePerSecondSquared,
    AccelerationForwardInMultiplesOfBrushSizePerSecondSquared,
    AccelerationLateralInMultiplesOfBrushSizePerSecondSquared,
    SpeedInPhysicalUnitsPerSecond,
    DistanceTraveledInPhysicalUnits,
    AccelerationInPhysicalUnitsPerSecondSquared,
}

pub enum BehaviorTarget {
    WidthMultiplier,
    HeightMultiplier,
    SizeMultiplier,
    SlantOffset,
    PinchOffset,
    RotationOffset,
    CornerRoundingOffset,
    PositionOffsetXInMultiplesOfBrushSize,
    PositionOffsetYInMultiplesOfBrushSize,
    PositionOffsetForwardInMultiplesOfBrushSize,
    PositionOffsetLateralInMultiplesOfBrushSize,
    TextureAnimationProgressOffset,
    HueOffset,
    SaturationMultiplier,
    LuminosityOffset,
    OpacityMultiplier,
}

pub enum PolarBehaviorTarget {
    PositionOffsetAbsolute,
    PositionOffsetRelativeToInputDirection,
}

pub enum EasingCurve {
    Linear,
    CubicBezier { x1: f64, y1: f64, x2: f64, y2: f64 },
    Steps { count: u32 },
}
```

Sources should cover pressure, tilt, orientation, speed, velocity components,
direction, traveled distance, time of input, prediction distance/time, distance
remaining, time since input, time since stroke end, acceleration, and physical
centimeter variants when `stroke_unit_to_physical` is present.

Targets should cover width, height, size, slant, pinch, rotation, corner
rounding, absolute/forward/lateral position offsets, texture animation progress,
hue, saturation, luminosity, and opacity.

Use these enums:

```rust
pub enum OutOfRange {
    Clamp,
    Repeat,
    Mirror,
}

pub enum BinaryOp {
    Product,
    Sum,
    Min,
    Max,
    AndThen,
    OrElse,
    XorElse,
}

pub enum ProgressDomain {
    DistanceInStrokeUnits,
    DistanceInMultiplesOfBrushSize,
    DistanceInPhysicalUnits,
    TimeInSeconds,
}

pub enum Interpolation {
    Lerp,
    InverseLerp,
}
```

Stateful behavior evaluation should keep current and fixed copies of noise
generators, damping values, integral states, and target modifiers. When tip
states become fixed, copy current state into fixed state. When volatile geometry
is rebuilt, restore from the fixed state.

### Tip State and Shape Types

`TipState` is the fully evaluated brush footprint at a modeled input. It maps
closely to Ink's `BrushTipState`.

```rust
pub struct ColorShift {
    pub hue_offset_turns: f32,
    pub saturation_multiplier: f32,
    pub luminosity_offset: f32,
    pub opacity_multiplier: f32,
}

pub struct TipState {
    pub position: Point,
    pub width: f64,
    pub height: f64,
    pub corner_rounding: f64,
    pub rotation: f64,
    pub slant: f64,
    pub pinch: f64,
    pub color_shift: ColorShift,
    pub texture_animation_progress_offset: f64,
}

pub struct Circle {
    pub center: Point,
    pub radius: f64,
}

pub struct TipShape {
    pub center: Point,
    pub perimeter_circles: smallvec::SmallVec<[Circle; 4]>,
    pub bounds: Rect,
}

pub struct ExtrusionPoints {
    pub left: Vec<Point>,
    pub right: Vec<Point>,
}

pub enum TangentQuality {
    FirstContainsSecond,
    SecondContainsFirst,
    BadJoinedCoverage,
    Good,
}
```

`TipShape` should provide:

- `from_tip_state(state, min_nonzero_radius_and_separation)`
- `contains(other)`
- `tangent_circle_indices(first, second)`
- `append_start_cap(first, second, max_chord_height, out)`
- `append_end_cap(second_to_last, last, max_chord_height, out)`
- `append_turn(start, middle, end, max_chord_height, out)`
- `append_whole_shape(shape, max_chord_height, forward_direction, out)`

### Mesh Types

The CPU mesh should be easy to upload to `wgpu`, but it should still carry the
outline and derivative data needed for higher quality stroke rendering.

```rust
pub struct StrokeVertex {
    pub position: Point,
    pub color: ColorRgba,
    pub side_derivative: Vec2,
    pub side_label: f32,
    pub forward_derivative: Vec2,
    pub forward_label: f32,
    pub uv: [f32; 2],
    pub animation_offset: f32,
}

pub struct MutableStrokeMesh {
    pub vertices: Vec<StrokeVertex>,
    pub indices: Vec<u32>,
    pub outlines: Vec<Vec<u32>>,
    pub bounds: Option<Rect>,
}

pub struct MeshPartition {
    pub vertices: Vec<StrokeVertex>,
    pub indices: Vec<u32>,
    pub outlines: Vec<Vec<u32>>,
    pub bounds: Option<Rect>,
}

pub struct PartitionedStrokeMesh {
    pub render_groups: Vec<Vec<MeshPartition>>,
    pub bounds: Option<Rect>,
}

pub struct StrokeShapeUpdate {
    pub dirty_bounds: Option<Rect>,
    pub first_changed_vertex: Option<usize>,
    pub first_changed_index: Option<usize>,
}
```

`MutableStrokeMesh` is used during live construction. `PartitionedStrokeMesh` is
the immutable finished form. Partitions should be split before any single GPU
index buffer exceeds the target index width; `u32` is acceptable for the CPU
builder even if the final renderer chooses `u16` partitions later.

### Live Stroke Types

`InProgressStroke` owns the live input queues, one shared input modeler, and one
shape builder per brush coat.

```rust
pub struct InProgressStroke {
    // brush: Option<Brush>
    // queued_real_inputs: InputBatch
    // queued_predicted_inputs: InputBatch
    // processed_inputs: InputBatch
    // real_input_count: usize
    // current_time: f64
    // input_modeler: InputModeler
    // shape_builders: Vec<StrokeShapeBuilder>
    // updated_region: Option<Rect>
    // inputs_finished: bool
}

pub struct StrokeUpdate {
    pub dirty_bounds: Option<Rect>,
    pub coat_updates: Vec<StrokeShapeUpdate>,
}

pub struct FinishedStroke {
    pub brush: Brush,
    pub inputs: InputBatch,
    pub mesh: PartitionedStrokeMesh,
}

pub enum StrokeError {
    InvalidInput,
    InvalidBrush,
    InvalidTime,
    StrokeNotStarted,
    InputsFinished,
    TessellationFailed,
}

impl InProgressStroke {
    pub fn start(&mut self, brush: Brush, noise_seed: u32) -> Result<(), StrokeError>;

    pub fn enqueue_inputs(
        &mut self,
        real: InputBatch,
        predicted: InputBatch,
    ) -> Result<(), StrokeError>;

    pub fn finish_inputs(&mut self);

    pub fn update_shape(&mut self, current_time: f64) -> Result<StrokeUpdate, StrokeError>;

    pub fn needs_update(&self) -> bool;

    pub fn changes_with_time(&self) -> bool;

    pub fn finish(self) -> Result<FinishedStroke, StrokeError>;
}
```

`enqueue_inputs` only queues data. `update_shape` consumes queued inputs, erases
old predictions, updates the modeler, and updates each coat builder. This split
matches Ink's frame-oriented flow and lets callers decide when geometry should
be regenerated.

## Input Modeling

Input modeling is deterministic and independent of brush styling. It should not
depend on user-facing parameters like brush sharpness, pen width, or material.

The modeler accepts real inputs and a replaceable predicted tail, then emits
`ModeledInput` values. It must keep enough internal state to support live
streaming without recomputing the whole stroke in normal cases.

The modeling process should include:

- Validate finite values, nondecreasing timestamps, and no repeated x/y/time
  triplets within each real stroke segment.
- Merge or suppress samples that are too close in space-time, because these tend
  to amplify device noise without adding useful physical information.
- Upsample long time gaps by inserting modeled timestamps between raw samples.
  Use a desired upsampling period and a hard cap per raw interval to avoid
  pathological memory growth.
- Smooth position, pressure, and tilt with symmetric sliding-window averaging
  over linearly interpolated raw input.
- Average orientation with a circular mean, by averaging unit vectors instead of
  raw angles.
- Compute cumulative traveled distance along the modeled path.
- Compute velocity and acceleration with derivative windows over modeled
  samples.
- Mark modeled inputs stable once future real samples can no longer affect their
  smoothing window; keep later samples unstable.

The stability rule is central to interactive drawing. Stable modeled inputs may
be committed into stable mesh partitions. Unstable modeled inputs and predicted
inputs must remain rollbackable.

Implementation boundary:

- `InputModeler` owns `InputModelerState`, the modeled input list, and the
  concrete input model implementation.
- `SlidingWindowInputModeler` owns a rolling raw input queue. Each update removes
  old predicted raw inputs, erases unstable modeled inputs, appends new real and
  predicted raw inputs, models unstable positions and derivatives, marks stable
  modeled inputs, and trims raw input that can no longer affect future unstable
  windows.
- `PassthroughInputModeler` should exist for tests and comparison, but
  `SlidingWindow` should be the default production model.

## Brush Behavior

Brush behavior is separate from input modeling. It converts modeled input
features into modifiers for the brush tip.

The behavior engine should be deterministic and compact. A flattened
post-order program is a good representation because it can be evaluated like a
small stack machine without heap-heavy graph traversal during live drawing.

Supported v1 behavior sources should include:

- pressure
- tilt
- orientation
- speed
- acceleration
- traveled distance
- elapsed time
- distance in brush-size units
- time since stroke end, for final tail effects

Supported v1 nodes should include:

- source normalization over a configured range
- out-of-range handling with clamp, repeat, and mirror
- deterministic 1D smooth noise
- damping with exponential relaxation
- integral accumulation with the trapezoidal rule
- basic binary operations: multiply, add, min, max
- null-aware control operators when source values are indeterminate
- linear interpolation and inverse linear interpolation
- target mapping from normalized values into modifier ranges

Modifiers should be aggregated by target:

- Multiplicative targets: width, height, size, opacity, saturation.
- Additive targets: rotation, slant, pinch, roundness, hue, luminosity, center
  offset.

The first brush set should prioritize calligraphy pens and markers. Pencil
texture simulation can come later after the mesh engine is reliable.

Implementation boundary:

- `BehaviorEvaluator` runs one flattened behavior program for one `BrushTip`.
- It consumes `ModeledInput`, `InputModelerState`, brush size, current time, and
  stroke-finish state.
- It writes into a `TargetModifiers` accumulator. Multiplicative targets start
  at `1`; additive targets start at `0`.
- It stores current and fixed state for noise, damping, integrals, and terminal
  target memory so volatile rebuilding starts from the last fixed tip state.

## Tip State and Shape

The tip modeler combines each `ModeledInput`, the base `BrushTip`, and evaluated
behavior modifiers into a `TipState`.

Each tip state should include:

- center
- width and height
- rotation
- slant
- pinch
- roundness
- color and opacity
- fixed or volatile status

The shape builder should convert each `TipState` into an analytic `TipShape`.
Use Google's Ink-style representation:

```text
TipShape = convex hull of 1 to 4 perimeter circles
```

This is expressive enough for rounded, slanted, pinched, and rotated brush tips
while staying structured enough for robust tangent construction.

Arc and curved-boundary sampling should be controlled by a maximum chord-height
tolerance. The tolerance should be part of geometry configuration, not brush
behavior.

Implementation boundary:

- `BrushTipModeler` consumes `InputModelerState` and the full modeled input
  slice. It returns `new_fixed_tip_states` and the current `volatile_tip_states`.
- It may emit zero or more `TipState`s per `ModeledInput`, especially for
  particle brushes and interpolated particle timing.
- It marks a tip state fixed only when it is generated from stable modeled input
  and is far enough from the current end that the brush behavior can no longer
  change it.
- `TipShapeBuilder` converts one `TipState` into the convex hull of 1 to 4
  perimeter circles and supplies sampled extrusion points for caps, turns, and
  isolated shapes.

## Mesh Generation

The mesh generator converts a sequence of `TipShape` values into triangle mesh
geometry.

The main stroke extrusion should be custom, not delegated entirely to a generic
tessellator, because it must preserve incremental updates, tangent quality, and
rollback behavior.

The extruder should:

- Connect consecutive tip shapes with tangent-constrained extrusion.
- Generate start caps, end caps, turns, and isolated shape geometry.
- Maintain left and right outline chains where the stroke behaves like a
  swept ribbon.
- Classify tangent failures, including containment and bad coverage.
- Try an interpolated intermediate tip shape when direct extrusion is invalid.
- Insert a partition break and cap the current partition when the tangent cannot
  be regularized.
- Keep stable partitions separate from the volatile tail.
- Return dirty bounds that include both removed volatile geometry and newly
  generated geometry.

Self-intersections should first be handled by robust partition breaks and local
fallbacks. More advanced outline repair and retriangulation can be added after
the basic incremental engine is correct.

Implementation boundary:

- `BrushTipExtrusion` stores either a `TipState` plus its `TipShape`, or an
  explicit break point.
- `BrushTipExtruder` owns the extrusion history, outlines, saved volatile
  checkpoint, geometry builder, and bounds cache for one brush coat.
- On each extension it restores the saved checkpoint, extrudes all new fixed
  states, saves a new checkpoint, then extrudes volatile states.
- Tips with width and height below `brush.epsilon` become break points.
- `StrokeShapeBuilder` owns one `BrushTipModeler` and one `BrushTipExtruder` per
  coat. It restarts both if time-dependent brush behavior invalidates previously
  fixed states.

## Lyon Tessellation

Use `lyon` as the tessellation library for general polygon fill triangulation and
fallback cases. It is a dependency of the engine, but it is not the whole stroke
extrusion algorithm.

Good uses for `lyon`:

- Triangulating closed outlines for caps, isolated tip shapes, and fallback
  partitions.
- Triangulating repaired or sampled polygons after a break.
- Providing a tested fill tessellator for non-zero winding cases where custom
  ribbon triangulation is not the right tool.

Avoid using `lyon` as the live stroke model itself. The engine still needs its
own tip-shape sequence, tangent classification, stable/volatile boundaries, and
mesh partitioning.

## Rendering Pipeline

Rendering should use `wgpu` directly. Vello is intentionally out of scope for
now.

The renderer should follow the wgpu middleware pattern: encapsulate GPU
resources, expose explicit prepare and render phases, and avoid mixing app event
handling with engine internals.

The renderer should provide an API shaped like:

```rust
impl GpuRenderer {
    pub fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Self;

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &StrokeScene,
    );

    pub fn render<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        transform: RenderTransform,
    );
}
```

GPU resource strategy:

- Stable mesh partitions should stay resident in GPU buffers.
- Volatile tails should be rewritten whenever live input or predictions change.
- Mesh uploads should use explicit dirty ranges or partition-level replacement.
- Vertices should use premultiplied color.
- Rendering should support MSAA and a logical-to-device transform.
- Surface errors such as lost or outdated surfaces should be handled by the
  playground app, not by panicking in the engine.

The playground app should capture pointer input with Winit, feed it into
`InProgressStroke`, and render the resulting mesh through `GpuRenderer`.

The current implemented playground is the first interactive slice of that
renderer plan:

- it opens a Winit window
- turns cursor movement into mouse `RawInput`
- updates one live `InProgressStroke`
- uploads the current CPU tessellated mesh to `wgpu` each frame
- renders with a minimal color-only triangle pipeline

This is intentionally a prototype upload path. Stable GPU residency, dirty
range uploads, and partition-level cache management still belong to later
renderer work.

Renderer type design:

```rust
pub struct StrokeScene {
    pub strokes: Vec<PartitionedStrokeMesh>,
    pub live_strokes: Vec<MutableStrokeMesh>,
}

pub struct RenderTransform {
    pub stroke_to_device: Affine,
    pub device_pixel_ratio: f32,
}

pub struct GpuRenderer {
    // wgpu pipeline, bind groups, stable partition buffers, volatile buffers.
}

impl GpuRenderer {
    pub fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Self;

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &StrokeScene,
    );

    pub fn render<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        transform: RenderTransform,
    );
}
```

`GpuRenderer` should keep stable mesh partitions resident and rewrite volatile
tail buffers on each live stroke update. The renderer should not own window or
surface lifecycle; the playground app owns Winit and surface recovery.

## Testing Plan

Input modeling tests:

- `InputBatch` rejects non-finite values, inconsistent tool/optional formats,
  invalid pressure/tilt/orientation ranges, and nonpositive physical scale
- nondecreasing timestamp validation and repeated x/y/time rejection
- upsampling cap for long gaps
- suppression of samples too close in space-time
- straight-line preservation after smoothing
- pressure and tilt smoothing
- orientation wraparound with circular mean
- traveled distance, velocity, and acceleration computation
- stable/unstable boundary behavior
- stable and real input counts are monotonic
- predicted tail replacement removes old predicted modeled inputs

Brush behavior tests:

- flattened node stack validation
- null propagation and null-aware binary operators
- source normalization
- clamp, repeat, and mirror handling
- deterministic noise output
- damping over time and distance
- integral accumulation
- binary operations and null-aware operators
- target aggregation rules
- current/fixed behavior state rollback for volatile rebuilding

Geometry and mesh tests:

- tip-shape construction for round, slanted, pinched, and rotated tips
- tip-shape circle count, bounds, containment, and tangent circle selection
- chord-height tolerance for sampled arcs
- start cap, end cap, and isolated shape generation
- tangent success and tangent failure cases
- containment fallback with interpolated intermediate shapes
- partition breaks
- fixed/volatile checkpoint restore in `BrushTipExtruder`
- outline generation and dirty bounds
- `lyon` fallback triangulation for closed sampled outlines
- stable partition preservation when volatile tails are rebuilt

Incremental stroke tests:

- enqueue/update separation
- `needs_update`, `changes_with_time`, and `finish_inputs`
- predicted inputs are replaceable
- old predicted geometry is removed from the volatile tail
- stable mesh partitions do not change after new inputs arrive
- `finish()` commits remaining volatile geometry
- dirty bounds include both removed and added geometry
- finished strokes are immutable snapshots of brush, inputs, and mesh

Normal `cargo test --workspace` tests should remain fast and deterministic. GPU
and windowing validation should stay in the playground or optional ignored tests.

## Implementation Order

Completed minimal slice:

1. Define the new data model and public API in the library.
2. Implement a minimal `InputModeler` with passthrough and sliding-window modes.
3. Implement basic brush/tip state creation.
4. Implement simple swept-outline mesh generation with caps.
5. Tessellate closed stroke outlines with `lyon`.
6. Replace the playground's Vello renderer with a command-line mesh example.
7. Add an interactive Winit + `wgpu` playground that draws cursor strokes with
   the current mesh algorithm.

Next implementation steps:

1. Expand brush behavior evaluation beyond the current placeholder
   `BrushBehavior`.
2. Replace the current one-circle `TipShape` approximation with the full
   convex hull of 1 to 4 perimeter circles.
3. Add fixed/volatile checkpointing inside a real `BrushTipExtruder`.
4. Split large finished meshes into GPU-friendly partitions.
5. Replace whole-mesh-per-frame uploads in `bin/src/renderer.rs` with stable
   partition residency and volatile-tail updates.
