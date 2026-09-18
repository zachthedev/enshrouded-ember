//! Type ids.
//!
//! A type id is `FNV-1a` 32 of the qualified name, so code computes it rather
//! than looking it up. The descriptor's other word is shared across types with
//! identical layouts, which makes it a layout hash rather than a name hash.
