//! Reading an executable image.
//!
//! The `Image` trait, the `Rva` and `Va` newtypes, section lookup, and the
//! refusals a read returns. One trait covers a file on disk and the image mapped
//! into a running process, so a scanner written once serves both.

pub mod file;
pub mod pdata;
pub mod process;
