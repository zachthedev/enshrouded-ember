//! Decoding the descriptor table out of an image.
//!
//! Pointer fields decode through the image's own address conversion, so one
//! scanner reads a file on disk and a mapped process alike.
