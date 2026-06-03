//! RAG (Retrieval-Augmented Generation) pipeline: embedding, vector store,
//! and MMR retrieval.
//!
//! Reference: design doc §11 (RAG), `.cursor/rules` §13 (RAG rules).

pub mod chunker;
pub mod embedder;
pub mod retriever;
pub mod store;
