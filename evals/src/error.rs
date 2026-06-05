//! Error types for the eval harness.
//!
//! The harness fails loudly on any infrastructure error (missing question
//! file, malformed JSON, judge unreachable). Per-question runtime failures
//! are recorded in the report but do not abort the run.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvalError {
    #[error("question bank file not found: {path}")]
    BankFileMissing { path: String },

    #[error("question bank could not be parsed ({path}): {source}")]
    BankParse {
        path: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("baseline file could not be read ({path}): {source}")]
    BaselineRead {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("judge call failed: {0}")]
    Judge(String),

    #[error("runner failed: {0}")]
    Runner(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
