//! Operating system seam for Ember.
//!
//! Three concerns cross the operating system boundary and nothing else does:
//! reading the running executable's image, installing an inline hook, and
//! getting a library loaded into the server process. Each is a trait with a
//! backend per platform, so the rest of Ember and every mod stay platform
//! neutral.
//!
//! Keen ships a Windows dedicated server only, and the Linux backends are
//! declared rather than written against a binary that does not exist.

pub mod hook;
pub mod image;
pub mod library;
pub mod platform;
pub mod scan;
pub mod testpe;
pub mod win32;

pub use platform::Platform;
