//! Finding mod libraries and reading what they declare.
//!
//! Each mod is one directory holding one library. The version export is read
//! first, and a mismatch is refused by name before anything else in the library
//! runs.
