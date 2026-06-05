//! Flint prompt evaluation harness (Phase 7.2, design doc §20).
//!
//! Runs a curated 200-question bank against every prompt variant
//! (`gpt.txt`, `claude.txt`, `llama.txt`) and produces structured scores for
//! relevance, grounding, conciseness, depth structure, and latency. A
//! regression gate enforces:
//!
//! * win rate >= 50% vs the stored baseline
//! * directional conciseness pass rate >= 95%
//! * no per-domain relevance score below 0.7
//!
//! The harness is invoked via `cargo run -p evals` or the GitHub Action
//! triggered on `prompts/**` changes.

pub mod bank;
pub mod baseline;
pub mod error;
pub mod gate;
pub mod judge;
pub mod metrics;
pub mod report;
pub mod runner;

pub use error::EvalError;
