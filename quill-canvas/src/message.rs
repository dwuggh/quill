use bevy_ecs::message::Message;

use crate::{element::Tool, input::CanvasInput};

#[derive(Clone, Debug, Copy, Message)]
pub enum CanvasMessage {
    /// Select a tool.
    SelectTool(Tool),
    /// Feed pointer input to the canvas.
    Input(CanvasInput),
    /// Stop the renderer thread.
    Shutdown,
}
