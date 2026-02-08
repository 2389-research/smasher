// ABOUTME: Provider adapter infrastructure for multi-provider LLM support.
// ABOUTME: Defines the ProviderAdapter trait and re-exports all provider implementations.

pub mod anthropic;
pub mod gemini;
pub mod openai;

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::types::{Error, Request, Response, StreamEvent};

/// A boxed stream of StreamEvent results, used for streaming LLM responses.
pub type StreamResponse = Pin<Box<dyn Stream<Item = Result<StreamEvent, Error>> + Send>>;

/// Trait that all provider adapters must implement.
///
/// Each provider (Anthropic, OpenAI, Gemini) implements this trait to translate
/// between the unified request/response types and the provider's native API format.
#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    /// The provider name (e.g., "anthropic", "openai", "gemini").
    fn provider_name(&self) -> &str;

    /// Send a completion request and return a unified response.
    async fn complete(&self, request: &Request) -> Result<Response, Error>;

    /// Send a streaming completion request and return a stream of events.
    async fn stream(&self, request: &Request) -> Result<StreamResponse, Error>;
}
