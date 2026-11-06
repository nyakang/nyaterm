//! UI-independent native AI provider client.

mod adapter;
mod async_proxy;
mod client;
mod error;
mod frames;
mod ollama_compat;
mod request;
mod response;
pub use adapter::{complete_chat, stream_chat};
pub use client::AiProviderSession;
mod discovery;
pub use discovery::{discover_provider_models, test_model_connection};
#[cfg(test)]
mod tests;
