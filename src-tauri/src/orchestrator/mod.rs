//! Orchestration layer: directional/depth/clarifying response threads,
//! pre-warm cache, and session lifecycle management.
//!
//! Reference: design doc §8 (System Architecture), `.cursor/rules` flint-core
//! §4 (parallel threads via tokio::spawn, never sequential).

pub mod prewarm;
