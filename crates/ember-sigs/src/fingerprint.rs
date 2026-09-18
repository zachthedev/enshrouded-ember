//! The fingerprint a build is matched on.
//!
//! The `CodeView` signature bytes in file order plus the age, read from the
//! image. Timestamp and image size give a weaker match, which is reported and
//! never resolves.
