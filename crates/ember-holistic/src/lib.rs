//! Bindings to the Holistic engine.
//!
//! The server binary carries its own reflection vocabulary as strings: entity
//! component types registered as `ecs.<Name>`, engine types as `keen::<Name>`,
//! and systems, which the engine calls programs, under `snake_case` names.
//! Those names are stable across builds in a way addresses are not, so they are
//! what Ember anchors on.
//!
//! Scope is the engine and nothing above it. Anything true only because the game
//! is Enshrouded belongs to `ember-enshrouded`.

pub mod build;
pub mod entity;
pub mod error;
pub mod frame;
pub mod program;
pub mod reflect;
pub mod xref;

pub use error::{HolisticError, Result};
