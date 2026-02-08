// ABOUTME: Core type definitions for the unified LLM client.
// ABOUTME: Provides Role, ContentPart, Message, Request, Response, Tool, and Stream types.

pub mod catalog;
pub mod content;
pub mod error;
pub mod message;
pub mod request;
pub mod response;
pub mod response_format;
pub mod role;
pub mod stream;
pub mod tool;

pub use catalog::{
    ModelInfo, Provider, get_latest_model, infer_provider, lookup_model, lookup_model_or_default,
    models_for_provider,
};
pub use content::{
    AudioData, ContentPart, DocumentData, DocumentSourceType, ImageData, ImageSourceType,
    RedactedThinkingData, ThinkingData, ToolCallData, ToolResultData,
};
pub use error::{Error, Result, StatusClass};
pub use message::Message;
pub use request::{Request, ThinkingConfig};
pub use response::{FinishReason, RateLimitInfo, Response, Usage, Warning};
pub use response_format::ResponseFormat;
pub use role::Role;
pub use stream::{StreamEvent, StreamEventType};
pub use tool::{ToolCall, ToolChoice, ToolDefinition, ToolResult};
