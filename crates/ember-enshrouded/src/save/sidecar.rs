//! A file a mod writes beside a save copy.
//!
//! Each sidecar is stamped with the save generation and the copy it was written
//! with, so a world rolled back loads the sidecar that matches it. Sidecars for
//! copies the index no longer names are pruned.
