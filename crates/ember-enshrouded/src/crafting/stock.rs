//! The per-base list of containers a craft draws from.
//!
//! The server rebuilds the list every tick from an entity query, keyed by which
//! base's bounding box holds the container. The base comes from the operator's
//! position, not the station's.
