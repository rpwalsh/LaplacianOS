//! In-kernel typed temporal graph subsystem.
//!
//! This is the shared substrate for operating-system telemetry, prediction,
//! provenance, and the built-in SCCE cognitive runtime. Kernel paths perform
//! bounded mutation and inspection; durable storage and heavyweight graph
//! processing belong to ring-3 services.
//!
//! Modules:
//! - `types`: Node, Edge, NodeKind, EdgeKind, Weight, flags, ADJ_NONE
//! - `arena`: Static graph store with adjacency lists
//! - `seed`: Boot-time graph population from BootInfo and hardware state
//! - `temporal`: Type-pair attenuation and transition statistics
//! - `spectral`: Spectral snapshots and change detection
//! - `walk`: Deterministic PowerWalk transition distributions
//! - `pattern`: Public structural pattern types
//! - `causal`: Public causal graph shape
//! - `tensor`: Public audit tensor shape

pub mod arena;
pub mod bootstrap;
pub mod causal;
pub mod handles;
pub mod pattern;
pub mod seed;
pub mod spectral;
pub mod temporal;
pub mod tensor;
pub mod twin;
pub mod typed_operator;
pub mod types;
pub mod walk;
