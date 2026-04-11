use crate::scene::Scene;
use anyhow::Result;
use quill::Point;
use tracing::trace;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton};

#[derive(Debug, Default)]
pub struct PointerInput {
    cursor_position: Option<Point>,
    drawing: bool,
}

impl PointerInput {
    pub fn cursor_moved(
        &mut self,
        position: PhysicalPosition<f64>,
        scene: &mut Scene,
    ) -> Result<bool> {
        let point = Point::new(position.x, position.y);
        self.cursor_position = Some(point);
        if self.drawing {
            let accepted = scene.append_point(point)?;
            trace!(
                accepted,
                x = point.x,
                y = point.y,
                "processed cursor movement"
            );
            return Ok(accepted);
        }
        Ok(false)
    }

    pub fn mouse_input(
        &mut self,
        button: MouseButton,
        state: ElementState,
        scene: &mut Scene,
    ) -> Result<bool> {
        if button != MouseButton::Left {
            return Ok(false);
        }

        match state {
            ElementState::Pressed => {
                if let Some(position) = self.cursor_position {
                    let accepted = scene.begin_stroke(position)?;
                    self.drawing = true;
                    Ok(accepted)
                } else {
                    Ok(false)
                }
            }
            ElementState::Released => {
                if self.drawing {
                    self.drawing = false;
                    scene.end_stroke()?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }
}
