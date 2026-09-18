//! A mod's log handle, which writes through the host's one sink.
//!
//! A mod library carries its own copy of every global, so a subscriber
//! installed inside one would collect lines nobody reads.
