use bevy::{
    camera::Camera2d,
    input::{
        ButtonState,
        mouse::{MouseButton, MouseButtonInput},
        touch::{ForceTouch, TouchInput, TouchPhase},
    },
    prelude::*,
    window::{CursorEntered, CursorLeft, CursorMoved, PrimaryWindow, WindowPlugin},
};
use quill_canvas::prelude::{CanvasInput, QuillPlugin};
use quill_stroke::input::{Input as StrokeInput, Vec2 as StrokeVec2};

fn main() {
    App::new()
        .insert_resource(ClearColor(Color::srgb(0.97, 0.97, 0.94)))
        .init_resource::<PointerInputState>()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Quill".into(),
                resolution: (1280, 840).into(),
                ..Default::default()
            }),
            ..Default::default()
        }))
        .add_plugins(QuillPlugin)
        .add_systems(PreUpdate, forward_pointer_input)
        .run();
}

#[derive(Resource, Default)]
struct PointerInputState {
    cursor_world: Option<StrokeVec2>,
    cursor_inside: bool,
    mouse_drawing: bool,
    active_touch: Option<u64>,
}

impl PointerInputState {
    fn update_cursor_position(
        &mut self,
        window: Entity,
        position: Vec2,
        primary_window: Entity,
        camera: (&Camera, &GlobalTransform),
    ) -> Option<StrokeVec2> {
        if window != primary_window {
            return None;
        }

        let point = viewport_to_canvas(position, camera)?;
        self.cursor_world = Some(point);
        Some(point)
    }

    fn stroke_input(&self, point: StrokeVec2, timestamp: f64, pressure: f32) -> StrokeInput {
        StrokeInput::new(point, timestamp)
            .with_pressure(pressure)
            .with_tilt(StrokeVec2::new(0.0, 0.0))
    }

    fn touch_pressure(force: Option<ForceTouch>) -> f32 {
        match force {
            Some(ForceTouch::Normalized(value)) => value as f32,
            Some(ForceTouch::Calibrated {
                force,
                max_possible_force,
                ..
            }) if max_possible_force > 0.0 => (force / max_possible_force) as f32,
            _ => 1.0,
        }
        .clamp(0.0, 1.0)
    }
}

fn forward_pointer_input(
    time: Res<Time>,
    primary_window: Query<Entity, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    mut state: ResMut<PointerInputState>,
    mut cursor_entered: MessageReader<CursorEntered>,
    mut cursor_left: MessageReader<CursorLeft>,
    mut cursor_moved: MessageReader<CursorMoved>,
    mut mouse_buttons: MessageReader<MouseButtonInput>,
    mut touches: MessageReader<TouchInput>,
    mut canvas_inputs: MessageWriter<CanvasInput>,
) {
    let Ok(primary_window) = primary_window.single() else {
        return;
    };
    let Ok(camera) = camera.single() else {
        return;
    };
    let timestamp = time.elapsed_secs_f64();

    for event in cursor_entered.read() {
        if event.window == primary_window {
            state.cursor_inside = true;
            if let Some(point) = state.cursor_world {
                canvas_inputs.write(CanvasInput::PointerEnter(point));
            }
        }
    }

    for event in cursor_left.read() {
        if event.window == primary_window {
            state.cursor_inside = false;
            canvas_inputs.write(CanvasInput::PointerLeave);
        }
    }

    for event in cursor_moved.read() {
        let Some(point) =
            state.update_cursor_position(event.window, event.position, primary_window, camera)
        else {
            continue;
        };

        if !state.cursor_inside {
            state.cursor_inside = true;
            canvas_inputs.write(CanvasInput::PointerEnter(point));
        }

        canvas_inputs.write(CanvasInput::PointerMove(
            state.stroke_input(point, timestamp, 1.0),
        ));
    }

    for event in mouse_buttons.read() {
        if event.window != primary_window || event.button != MouseButton::Left {
            continue;
        }

        let Some(point) = state.cursor_world else {
            continue;
        };
        let input = state.stroke_input(point, timestamp, 1.0);

        match event.state {
            ButtonState::Pressed if !state.mouse_drawing => {
                state.mouse_drawing = true;
                canvas_inputs.write(CanvasInput::PointerDown(input));
            }
            ButtonState::Released if state.mouse_drawing => {
                state.mouse_drawing = false;
                canvas_inputs.write(CanvasInput::PointerUp(input));
            }
            _ => {}
        }
    }

    for event in touches.read() {
        if event.window != primary_window {
            continue;
        }

        let Some(point) = viewport_to_canvas(event.position, camera) else {
            continue;
        };
        let input = state.stroke_input(
            point,
            timestamp,
            PointerInputState::touch_pressure(event.force),
        );

        match event.phase {
            TouchPhase::Started if state.active_touch.is_none() => {
                state.active_touch = Some(event.id);
                canvas_inputs.write(CanvasInput::PointerEnter(point));
                canvas_inputs.write(CanvasInput::PointerDown(input));
            }
            TouchPhase::Moved if state.active_touch == Some(event.id) => {
                canvas_inputs.write(CanvasInput::PointerMove(input));
            }
            TouchPhase::Ended | TouchPhase::Canceled if state.active_touch == Some(event.id) => {
                state.active_touch = None;
                canvas_inputs.write(CanvasInput::PointerUp(input));
            }
            _ => {}
        }
    }
}

fn viewport_to_canvas(
    position: Vec2,
    (camera, transform): (&Camera, &GlobalTransform),
) -> Option<StrokeVec2> {
    let world = camera.viewport_to_world_2d(transform, position).ok()?;
    Some(StrokeVec2::new(world.x, world.y))
}
