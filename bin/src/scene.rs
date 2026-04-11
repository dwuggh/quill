use anyhow::Result;
use quill::{
    Brush, ColorRgba, FinishedStroke, InProgressStroke, InputBatch, MutableStrokeMesh, Point,
    RawInput, ToolType,
};
use std::time::Instant;
use tracing::{debug, trace, warn};

const MIN_SAMPLE_DISTANCE: f64 = 0.75;
const MIN_SAMPLE_PERIOD: f64 = 1.0 / 240.0;

#[derive(Debug)]
struct ActiveStroke {
    stroke: InProgressStroke,
    started_at: Instant,
    last_sample: Option<(Point, f64)>,
    accepted_samples: usize,
    needs_update: bool,
}

impl ActiveStroke {
    fn new(brush: Brush, noise_seed: u32) -> Result<Self> {
        let mut stroke = InProgressStroke::default();
        stroke.start(brush, noise_seed)?;
        Ok(Self {
            stroke,
            started_at: Instant::now(),
            last_sample: None,
            accepted_samples: 0,
            needs_update: false,
        })
    }

    fn enqueue_point(&mut self, position: Point) -> Result<bool> {
        let mut time = self.started_at.elapsed().as_secs_f64();
        if let Some((last_position, last_time)) = self.last_sample {
            if time <= last_time {
                time = last_time + 1e-6;
            }

            let distance = position.distance(last_position);
            let dt = time - last_time;
            if distance < MIN_SAMPLE_DISTANCE && dt < MIN_SAMPLE_PERIOD {
                trace!(distance, dt, "skipping dense cursor sample before enqueue");
                return Ok(false);
            }
        }

        let input = RawInput::new(position, time)
            .with_tool(ToolType::Mouse)
            .with_pressure(1.0);
        self.stroke
            .enqueue_inputs(InputBatch::new(vec![input])?, InputBatch::default())?;
        self.last_sample = Some((position, time));
        self.accepted_samples += 1;
        self.needs_update = true;
        trace!(
            accepted_samples = self.accepted_samples,
            time,
            x = position.x,
            y = position.y,
            "enqueued live cursor sample"
        );
        Ok(true)
    }

    fn update_shape(&mut self) -> Result<bool> {
        if !self.needs_update {
            return Ok(false);
        }

        let current_time = self.started_at.elapsed().as_secs_f64();
        let update = self.stroke.update_shape(current_time)?;
        let mesh = self.stroke.meshes().next();
        debug!(
            accepted_samples = self.accepted_samples,
            dirty_bounds = ?update.dirty_bounds,
            vertices = mesh.map_or(0, |mesh| mesh.vertices.len()),
            indices = mesh.map_or(0, |mesh| mesh.indices.len()),
            "updated live stroke mesh"
        );
        self.needs_update = false;
        Ok(true)
    }
}

#[derive(Debug)]
pub struct Scene {
    brush: Brush,
    next_noise_seed: u32,
    completed_strokes: Vec<FinishedStroke>,
    active_stroke: Option<ActiveStroke>,
}

impl Scene {
    pub fn new() -> Result<Self> {
        Ok(Self {
            brush: Brush::new(18.0, ColorRgba::BLACK)?,
            next_noise_seed: 1,
            completed_strokes: Vec::new(),
            active_stroke: None,
        })
    }

    pub fn begin_stroke(&mut self, position: Point) -> Result<bool> {
        let noise_seed = self.next_noise_seed;
        let mut active = ActiveStroke::new(self.brush.clone(), noise_seed)?;
        self.next_noise_seed = self.next_noise_seed.wrapping_add(1);
        let accepted = active.enqueue_point(position)?;
        debug!(
            x = position.x,
            y = position.y,
            accepted,
            noise_seed,
            "started stroke"
        );
        self.active_stroke = Some(active);
        Ok(accepted)
    }

    pub fn append_point(&mut self, position: Point) -> Result<bool> {
        if let Some(active) = &mut self.active_stroke {
            return active.enqueue_point(position);
        }
        Ok(false)
    }

    pub fn update_live_stroke(&mut self) -> Result<bool> {
        let Some(active) = &mut self.active_stroke else {
            return Ok(false);
        };
        active.update_shape()
    }

    pub fn end_stroke(&mut self) -> Result<()> {
        let Some(mut active) = self.active_stroke.take() else {
            return Ok(());
        };
        let _ = active.update_shape()?;
        let mesh_stats = active
            .stroke
            .meshes()
            .map(|mesh| (mesh.vertices.len(), mesh.indices.len()))
            .collect::<Vec<_>>();
        self.completed_strokes.push(active.stroke.finish()?);
        debug!(
            completed_strokes = self.completed_strokes.len(),
            samples = active.accepted_samples,
            mesh_stats = ?mesh_stats,
            "finished stroke"
        );
        Ok(())
    }

    pub fn clear(&mut self) {
        debug!(
            completed_strokes = self.completed_strokes.len(),
            had_active = self.active_stroke.is_some(),
            "cleared playground scene"
        );
        self.completed_strokes.clear();
        self.active_stroke = None;
    }

    pub fn completed_strokes(&self) -> &[FinishedStroke] {
        &self.completed_strokes
    }

    pub fn active_meshes(&self) -> Vec<&MutableStrokeMesh> {
        self.active_stroke
            .as_ref()
            .map(|active| active.stroke.meshes().collect())
            .unwrap_or_default()
    }

    pub fn log_large_scene_if_needed(&self) {
        let mut total_vertices = 0_usize;
        let mut total_indices = 0_usize;

        for stroke in &self.completed_strokes {
            for group in &stroke.mesh.render_groups {
                for partition in group {
                    total_vertices += partition.vertices.len();
                    total_indices += partition.indices.len();
                }
            }
        }
        for mesh in self.active_meshes() {
            total_vertices += mesh.vertices.len();
            total_indices += mesh.indices.len();
        }

        if total_vertices > 100_000 || total_indices > 300_000 {
            warn!(
                total_vertices,
                total_indices,
                completed_strokes = self.completed_strokes.len(),
                active = self.active_stroke.is_some(),
                "scene geometry is large for the current prototype renderer"
            );
        }
    }
}
