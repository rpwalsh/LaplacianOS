//! SCCE cognitive runtime engines.
//!
//! The cognitive subsystem shares LaplacianOS's typed temporal graph substrate
//! while keeping heavyweight retrieval, ranking, spectral refresh, and model
//! orchestration out of latency-critical mutation paths.

pub mod bm25;
pub mod correlation;
pub mod engine;
pub mod indexing;
pub mod kneser_ney;
pub mod lanczos;
pub mod lsh;
pub mod memory;
pub mod pagerank;
pub mod pipeline;
pub mod redact;
pub mod sketch;
pub mod spectral_refresh;
pub mod walsh_math;
