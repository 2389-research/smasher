// ABOUTME: High-level API functions for common LLM operations.
// ABOUTME: Provides generate (with tool loop), stream, generate_object, and stream_object convenience functions.

pub mod generate;
pub mod generate_object;
pub mod stream;
pub mod stream_object;

pub use generate::generate;
pub use generate_object::generate_object;
pub use stream::{StreamAccumulator, StreamResult, stream};
pub use stream_object::{PartialObjectEvent, StreamObjectResult, stream_object};
