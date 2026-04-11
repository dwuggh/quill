use crate::input::validate_input_sequence;
use crate::mesh::{union_optional_rects, union_rect_accum};
use crate::{
    Brush, InputBatch, InputModelConfig, InputModeler, InputModelerState, MeshPartition,
    MutableStrokeMesh, PartitionedStrokeMesh, RawInput, StrokeError, StrokeShapeUpdate, ToolType,
    build_stroke_mesh,
};
use kurbo::{Point, Rect};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct StrokeUpdate {
    pub dirty_bounds: Option<Rect>,
    pub coat_updates: Vec<StrokeShapeUpdate>,
}

#[derive(Debug, Clone)]
pub struct FinishedStroke {
    pub brush: Brush,
    pub inputs: InputBatch,
    pub mesh: PartitionedStrokeMesh,
}

#[derive(Debug, Clone)]
pub struct StrokeShapeBuilder {
    brush: Brush,
    mesh: MutableStrokeMesh,
}

impl StrokeShapeBuilder {
    pub fn new(brush: Brush) -> Self {
        Self {
            brush,
            mesh: MutableStrokeMesh::default(),
        }
    }

    pub fn build(
        &mut self,
        modeled_inputs: &[crate::ModeledInput],
    ) -> Result<StrokeShapeUpdate, StrokeError> {
        let previous_bounds = self.mesh.bounds;
        self.mesh = build_stroke_mesh(modeled_inputs, &self.brush)?;
        Ok(StrokeShapeUpdate {
            dirty_bounds: union_optional_rects(previous_bounds, self.mesh.bounds),
            first_changed_vertex: Some(0).filter(|_| !self.mesh.vertices.is_empty()),
            first_changed_index: Some(0).filter(|_| !self.mesh.indices.is_empty()),
        })
    }

    pub fn mesh(&self) -> &MutableStrokeMesh {
        &self.mesh
    }
}

#[derive(Debug, Clone)]
pub struct InProgressStroke {
    brush: Option<Brush>,
    real_inputs: Vec<RawInput>,
    predicted_inputs: Vec<RawInput>,
    input_modeler: InputModeler,
    shape_builders: Vec<StrokeShapeBuilder>,
    updated_region: Option<Rect>,
    inputs_finished: bool,
}

impl Default for InProgressStroke {
    fn default() -> Self {
        Self {
            brush: None,
            real_inputs: Vec::new(),
            predicted_inputs: Vec::new(),
            input_modeler: InputModeler::new(InputModelConfig::default()),
            shape_builders: Vec::new(),
            updated_region: None,
            inputs_finished: true,
        }
    }
}

impl InProgressStroke {
    pub fn start(&mut self, brush: Brush, noise_seed: u32) -> Result<(), StrokeError> {
        brush.validate()?;
        self.real_inputs.clear();
        self.predicted_inputs.clear();
        self.input_modeler = InputModeler::new(brush.family.input_model);
        self.shape_builders = brush
            .family
            .coats
            .iter()
            .map(|_| StrokeShapeBuilder::new(brush.clone()))
            .collect();
        self.updated_region = None;
        self.inputs_finished = false;
        self.brush = Some(brush);
        let _ = noise_seed;
        Ok(())
    }

    pub fn enqueue_inputs(
        &mut self,
        real: InputBatch,
        predicted: InputBatch,
    ) -> Result<(), StrokeError> {
        if self.brush.is_none() {
            return Err(StrokeError::StrokeNotStarted);
        }
        if self.inputs_finished {
            return Err(StrokeError::InputsFinished);
        }
        real.validate()?;
        predicted.validate()?;
        self.real_inputs.extend(real.inputs);
        self.predicted_inputs = predicted.inputs;
        let mut combined = self.real_inputs.clone();
        combined.extend_from_slice(&self.predicted_inputs);
        validate_input_sequence(&combined)?;
        Ok(())
    }

    pub fn finish_inputs(&mut self) {
        self.inputs_finished = true;
        self.predicted_inputs.clear();
    }

    pub fn update_shape(&mut self, current_time: f64) -> Result<StrokeUpdate, StrokeError> {
        let brush = self.brush.as_ref().ok_or(StrokeError::StrokeNotStarted)?;
        if !current_time.is_finite() || current_time < 0.0 {
            return Err(StrokeError::InvalidTime);
        }

        let modeled_inputs = self.input_modeler.model(
            &self.real_inputs,
            &self.predicted_inputs,
            current_time,
            self.inputs_finished,
        )?;

        let mut update = StrokeUpdate::default();
        for builder in &mut self.shape_builders {
            let coat_update = builder.build(modeled_inputs)?;
            update.dirty_bounds =
                union_optional_rects(update.dirty_bounds, coat_update.dirty_bounds);
            update.coat_updates.push(coat_update);
        }
        if self.shape_builders.is_empty() && brush.family.coats.is_empty() {
            return Err(StrokeError::InvalidBrush("brush has no coats"));
        }
        self.updated_region = union_optional_rects(self.updated_region, update.dirty_bounds);
        Ok(update)
    }

    pub fn needs_update(&self) -> bool {
        self.brush.is_some() && (!self.real_inputs.is_empty() || !self.predicted_inputs.is_empty())
    }

    pub fn changes_with_time(&self) -> bool {
        false
    }

    pub fn meshes(&self) -> impl Iterator<Item = &MutableStrokeMesh> {
        self.shape_builders.iter().map(StrokeShapeBuilder::mesh)
    }

    pub fn updated_region(&self) -> Option<Rect> {
        self.updated_region
    }

    pub fn modeled_state(&self) -> &InputModelerState {
        self.input_modeler.state()
    }

    pub fn finish(mut self) -> Result<FinishedStroke, StrokeError> {
        self.finish_inputs();
        let last_time = self.real_inputs.last().map_or(0.0, |input| input.time);
        let _ = self.update_shape(last_time)?;
        let brush = self.brush.clone().ok_or(StrokeError::StrokeNotStarted)?;
        let render_groups = self
            .shape_builders
            .into_iter()
            .map(|builder| {
                vec![MeshPartition {
                    vertices: builder.mesh.vertices,
                    indices: builder.mesh.indices,
                    outlines: builder.mesh.outlines,
                    bounds: builder.mesh.bounds,
                }]
            })
            .collect::<Vec<_>>();
        let bounds = render_groups
            .iter()
            .flat_map(|group| group.iter().filter_map(|mesh| mesh.bounds))
            .fold(None, union_rect_accum);
        Ok(FinishedStroke {
            brush,
            inputs: InputBatch {
                inputs: self.real_inputs,
                noise_seed: 0,
            },
            mesh: PartitionedStrokeMesh {
                render_groups,
                bounds,
            },
        })
    }
}

pub fn sample_demo_inputs() -> Vec<RawInput> {
    (0..48)
        .map(|i| {
            let t = i as f64 / 120.0;
            let x = 40.0 + i as f64 * 8.0;
            let y = 160.0 + (i as f64 * 0.32).sin() * 48.0;
            let pressure = 0.35 + 0.65 * ((i as f64 * 0.17).sin() * 0.5 + 0.5);
            RawInput::new(Point::new(x, y), t)
                .with_tool(ToolType::Stylus)
                .with_pressure(pressure)
        })
        .collect()
}
