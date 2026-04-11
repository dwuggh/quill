use lyon::tessellation::TessellationError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StrokeError {
    #[error("invalid input: {0}")]
    InvalidInput(&'static str),
    #[error("invalid brush: {0}")]
    InvalidBrush(&'static str),
    #[error("invalid time")]
    InvalidTime,
    #[error("stroke has not been started")]
    StrokeNotStarted,
    #[error("stroke inputs are already finished")]
    InputsFinished,
    #[error("tessellation failed: {0}")]
    TessellationFailed(#[from] TessellationError),
}
