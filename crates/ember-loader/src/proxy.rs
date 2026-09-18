//! Standing in for a system library the server already imports.
//!
//! The library Ember stands in for is chosen by the name it is given on disk,
//! and every export of that library is forwarded, because anything else in the
//! process may import the ones the server does not.

pub mod exports;
pub mod forward;
