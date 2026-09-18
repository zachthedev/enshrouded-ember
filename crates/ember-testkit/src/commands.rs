//! Routing a command to the module that answers it.
//!
//! The routing is exhaustive over the command set, so a command with no handler
//! is a compile error rather than a silent no-op.

pub mod player;
pub mod sim;
pub mod state;
pub mod world;
