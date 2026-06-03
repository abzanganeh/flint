//! Session lifecycle: state machine, memory, persistence, and recovery.
//!
//! Reference: design doc §25 (Session State Machine) and `.cursor/rules`
//! §4.2.

pub mod memory;
pub mod persistence;
pub mod state;
