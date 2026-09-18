//! Loading a library and reaching another process.
//!
//! Three things cross this seam: loading a library by name or by path,
//! resolving one of its exports, and getting a library loaded into a process
//! Ember is not running in.

pub mod linux;
pub mod windows;
