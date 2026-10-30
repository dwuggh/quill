//! Shared model contracts and validation for real and predicted samples.

use super::point::{InputSample, PathPoint};

/// Whether a sample was observed or provisionally predicted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputSource {
    /// A measurement received from the input device.
    Real,
    /// A provisional measurement replaced by the next enqueue operation.
    Predicted,
}

/// Invalid configuration or sample data rejected before model state is changed.
#[derive(Debug, thiserror::Error)]
pub enum InputModelError {
    #[error("invalid input modeler config field `{field}` with value {value}")]
    InvalidConfig { field: &'static str, value: f64 },

    #[error("non-finite {input_source:?} input at index {index}, field `{field}`, value {value}")]
    NonFiniteInput {
        input_source: InputSource,
        index: usize,
        field: &'static str,
        value: f64,
    },

    #[error(
        "non-monotonic {input_source:?} input timestamp at index {index}: previous {previous}, current {current}"
    )]
    NonMonotonicTimestamp {
        input_source: InputSource,
        index: usize,
        previous: f64,
        current: f64,
    },
}

/// Indices describing which modeled points must be replaced and which are final.
#[derive(Clone, Copy, Debug)]
pub struct ModelUpdateMeta {
    /// Old modeled path indices that should be considered invalid.
    ///
    /// Downstream geometry caches should delete/rebuild from this index.
    pub invalidated_from: usize,

    /// Prefix `[0, stable_len)` will not change anymore.
    pub stable_len: usize,
}

/// Borrowed replacement suffix of an incrementally modeled path.
#[derive(Clone, Copy, Debug)]
pub struct ModelUpdate<'a> {
    /// Absolute path indices used to apply this update.
    pub metadata: ModelUpdateMeta,

    /// Points replacing the previous path from `metadata.invalidated_from` onward.
    pub updated: &'a [PathPoint],
}

/// Smooth and interpolate raw samples into an incrementally updated stroke path.
pub trait InputModeler {
    /// Clear cached samples, predictions, and modeled points for a new stroke.
    fn reset(&mut self);

    /// Append real samples and replace all previous predicted samples.
    ///
    /// Rejected input leaves model state unchanged. Successful calls return the
    /// replacement suffix and the length of the prefix that is now final.
    fn enqueue<'a>(
        &'a mut self,
        points: &[InputSample],
        predicted: &[InputSample],
    ) -> Result<ModelUpdate<'a>, InputModelError>;
    /// Return the number of modeled points that future input will not change.
    fn stable_len(&self) -> usize;
}

/// A borrowed batch with source information for validation diagnostics.
pub(super) struct InputBatch<'a> {
    inputs: &'a [InputSample],
    source: InputSource,
}

impl<'a> InputBatch<'a> {
    /// Associate samples with their real or predicted origin.
    pub(super) fn new(inputs: &'a [InputSample], source: InputSource) -> Self {
        Self { inputs, source }
    }

    /// Check finite fields and nondecreasing timestamps, including the batch boundary.
    pub(super) fn validate_after(
        &self,
        previous_timestamp: Option<f64>,
    ) -> Result<(), InputModelError> {
        let mut previous_timestamp = previous_timestamp;

        for (index, input) in self.inputs.iter().enumerate() {
            input.validate(self.source, index)?;

            if let Some(previous) = previous_timestamp
                && input.timestamp < previous
            {
                return Err(InputModelError::NonMonotonicTimestamp {
                    input_source: self.source,
                    index,
                    previous,
                    current: input.timestamp,
                });
            }
            previous_timestamp = Some(input.timestamp);
        }

        Ok(())
    }
}
