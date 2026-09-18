//! Reading an image from a file on disk.
//!
//! A section's virtual size can exceed its raw size, so a read past the raw
//! bytes fails rather than returning zeros.
