//! The vocabulary of an inline hook.
//!
//! `HookTarget`, `Order`, `Flow` and the type-erased link a mod library hands
//! across the boundary live here, in the lowest crate every caller above the
//! operating system seam can see.

pub mod backend;
pub mod registry;
