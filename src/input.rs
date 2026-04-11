use crate::StrokeError;
use kurbo::Point;
use std::f64::consts::{PI, TAU};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolType {
    Unknown,
    Mouse,
    Touch,
    Stylus,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawInput {
    pub tool: ToolType,
    pub position: Point,
    pub time: f64,
    pub stroke_unit_to_physical: Option<f64>,
    pub pressure: Option<f64>,
    pub tilt: Option<f64>,
    pub orientation: Option<f64>,
}

impl RawInput {
    pub fn new(position: Point, time: f64) -> Self {
        Self {
            tool: ToolType::Unknown,
            position,
            time,
            stroke_unit_to_physical: None,
            pressure: None,
            tilt: None,
            orientation: None,
        }
    }

    pub fn with_pressure(mut self, pressure: f64) -> Self {
        self.pressure = Some(pressure);
        self
    }

    pub fn with_tool(mut self, tool: ToolType) -> Self {
        self.tool = tool;
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct InputBatch {
    pub inputs: Vec<RawInput>,
    pub noise_seed: u32,
}

impl InputBatch {
    pub fn new(inputs: Vec<RawInput>) -> Result<Self, StrokeError> {
        let batch = Self {
            inputs,
            noise_seed: 0,
        };
        batch.validate()?;
        Ok(batch)
    }

    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }

    pub fn validate(&self) -> Result<(), StrokeError> {
        validate_input_sequence(&self.inputs)
    }
}

pub(crate) fn validate_input_sequence(inputs: &[RawInput]) -> Result<(), StrokeError> {
    let Some(first) = inputs.first() else {
        return Ok(());
    };
    validate_input(first)?;
    let tool = first.tool;
    let has_pressure = first.pressure.is_some();
    let has_tilt = first.tilt.is_some();
    let has_orientation = first.orientation.is_some();
    let has_physical = first.stroke_unit_to_physical.is_some();

    for pair in inputs.windows(2) {
        let prev = pair[0];
        let next = pair[1];
        validate_input(&next)?;
        if next.tool != tool
            || next.pressure.is_some() != has_pressure
            || next.tilt.is_some() != has_tilt
            || next.orientation.is_some() != has_orientation
            || next.stroke_unit_to_physical.is_some() != has_physical
        {
            return Err(StrokeError::InvalidInput(
                "tool and optional fields must be consistent inside a stroke",
            ));
        }
        if next.time < prev.time {
            return Err(StrokeError::InvalidInput(
                "input time must be nondecreasing",
            ));
        }
        if next.time == prev.time && next.position == prev.position {
            return Err(StrokeError::InvalidInput(
                "repeated position/time triplets are not allowed",
            ));
        }
    }
    Ok(())
}

fn validate_input(input: &RawInput) -> Result<(), StrokeError> {
    if !input.position.x.is_finite() || !input.position.y.is_finite() || !input.time.is_finite() {
        return Err(StrokeError::InvalidInput(
            "position and time must be finite",
        ));
    }
    if input.time < 0.0 {
        return Err(StrokeError::InvalidInput("input time must be non-negative"));
    }
    if let Some(value) = input.stroke_unit_to_physical
        && (!value.is_finite() || value <= 0.0)
    {
        return Err(StrokeError::InvalidInput(
            "physical scale must be finite and positive",
        ));
    }
    if let Some(value) = input.pressure
        && (!value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(StrokeError::InvalidInput("pressure must be in [0, 1]"));
    }
    if let Some(value) = input.tilt
        && (!value.is_finite() || !(0.0..=PI * 0.5).contains(&value))
    {
        return Err(StrokeError::InvalidInput("tilt must be in [0, pi / 2]"));
    }
    if let Some(value) = input.orientation
        && (!value.is_finite() || !(0.0..TAU).contains(&value))
    {
        return Err(StrokeError::InvalidInput("orientation must be in [0, 2pi)"));
    }
    Ok(())
}
