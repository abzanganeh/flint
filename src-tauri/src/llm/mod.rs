//! LLM provider trait and concrete implementations.
//!
//! Reference: design doc §27 (Service Interface Contracts).

pub mod anthropic;
pub mod deepseek;
pub mod failover;
pub mod groq;
pub mod ollama;
pub mod openai;
pub mod openai_compat;
pub mod openrouter;
pub mod provider;
pub mod rate_limiter;
pub mod stack;
