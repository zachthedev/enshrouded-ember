//! The Windows backend.
//!
//! A full-path load of a system library can return the proxy itself, so a load
//! compares the returned base against Ember's own module.
