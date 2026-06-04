//! LLM provider trait and concrete implementations.
//!
//! Reference: design doc §27 (Service Interface Contracts).

pub mod failover;
pub mod groq;
pub mod ollama;
pub mod provider;
pub mod rate_limiter;
