//! Minimal Quill stroke engine.
//!
//! The current implementation validates raw input, produces a modeled path,
//! turns a brush into variable-width tip states, builds a swept outline, and
//! tessellates that outline with `lyon`.

pub mod brush;
pub mod error;
pub mod input;
pub mod mesh;
pub mod model;
pub mod shape;
pub mod stroke;

pub use brush::{
    BlendMode, Brush, BrushBehavior, BrushCoat, BrushFamily, BrushPaint, BrushTip, ColorRgba,
};
pub use error::StrokeError;
pub use input::{InputBatch, RawInput, ToolType};
pub use mesh::{
    MeshPartition, MutableStrokeMesh, PartitionedStrokeMesh, StrokeShapeUpdate, StrokeVertex,
    build_stroke_mesh,
};
pub use model::{InputMetrics, InputModelConfig, InputModeler, InputModelerState, ModeledInput};
pub use shape::{Circle, ColorShift, TangentQuality, TipShape, TipState};
pub use stroke::{FinishedStroke, InProgressStroke, StrokeUpdate, sample_demo_inputs};

pub use kurbo;
pub use kurbo::{Point, Rect, Vec2};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_repeated_triplets() {
        let inputs = vec![
            RawInput::new(Point::new(1.0, 2.0), 0.0),
            RawInput::new(Point::new(1.0, 2.0), 0.0),
        ];
        assert!(InputBatch::new(inputs).is_err());
    }

    #[test]
    fn smooths_and_tessellates_demo_stroke() {
        let brush = Brush::new(18.0, ColorRgba::BLACK).unwrap();
        let mut stroke = InProgressStroke::default();
        stroke.start(brush, 0).unwrap();
        stroke
            .enqueue_inputs(
                InputBatch::new(sample_demo_inputs()).unwrap(),
                InputBatch::default(),
            )
            .unwrap();
        let update = stroke.update_shape(1.0).unwrap();
        let mesh = stroke.meshes().next().unwrap();

        assert!(update.dirty_bounds.is_some());
        assert!(mesh.vertices.len() >= 3);
        assert!(mesh.indices.len() >= 3);
        assert_eq!(mesh.indices.len() % 3, 0);
        assert!(stroke.modeled_state().stable_input_count > 0);
    }

    #[test]
    fn predicted_inputs_are_replaced() {
        let brush = Brush::new(12.0, ColorRgba::BLACK).unwrap();
        let mut stroke = InProgressStroke::default();
        stroke.start(brush, 0).unwrap();
        stroke
            .enqueue_inputs(
                InputBatch::new(vec![
                    RawInput::new(Point::new(0.0, 0.0), 0.0),
                    RawInput::new(Point::new(20.0, 0.0), 0.1),
                ])
                .unwrap(),
                InputBatch::new(vec![RawInput::new(Point::new(40.0, 40.0), 0.2)]).unwrap(),
            )
            .unwrap();
        stroke.update_shape(0.2).unwrap();
        let first_predicted_count = stroke.modeled_state().real_input_count;
        stroke
            .enqueue_inputs(
                InputBatch::default(),
                InputBatch::new(vec![RawInput::new(Point::new(40.0, -40.0), 0.2)]).unwrap(),
            )
            .unwrap();
        stroke.update_shape(0.2).unwrap();
        assert_eq!(
            stroke.modeled_state().real_input_count,
            first_predicted_count
        );
    }
}
