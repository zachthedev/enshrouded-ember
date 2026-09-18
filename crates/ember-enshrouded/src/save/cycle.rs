//! The seam around a save.
//!
//! A mod stages a payload before the save and the host commits it only once the
//! server reports the save succeeded, so a failed save leaves no file describing
//! a world that was never written.
