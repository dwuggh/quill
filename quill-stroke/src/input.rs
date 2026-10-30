//! Frontend-independent samples and incremental stroke input modeling.

mod model;
mod point;
mod will;

pub use model::{InputModelError, InputModeler, InputSource, ModelUpdate, ModelUpdateMeta};
pub use point::{InputSample, PathPoint, PenType};
pub use will::{WillInputModeler, WillInputModelerConfig};
