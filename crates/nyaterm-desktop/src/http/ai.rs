//! Compatibility facade for the native AI provider client.

pub use nyaterm_ai_client::{
    AiProviderSession, complete_chat, discover_provider_models, stream_chat, test_model_connection,
};
