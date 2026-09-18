//! Read Keen's KFC3 resource containers.
//!
//! A build ships its data as three kinds of file. One `.kfc` directory holds
//! every index and is uncompressed. One `.kfc_resources` payload holds typed
//! resource data as a run of independent zstd frames, one per chunk, each padded
//! out to a slot the directory sizes. A power-of-two count of `.dat` containers
//! hold opaque content blobs, addressed by a 128-bit [`ContentHash`] whose first
//! word is the blob's length.
//!
//! The padding is why a plain `zstd -d` of the payload stops after one chunk.
//! Trimmed to its frame size, a chunk is an ordinary zstd frame with nothing
//! custom about it.
//!
//! Reading a resource means finding its [`ResourceId`] in the directory, taking
//! the offset and size the parallel entry gives in the decompressed stream, and
//! copying those bytes out of whichever chunks the span touches. A resource can
//! straddle a chunk boundary, so more than one chunk may be decompressed for
//! one read.
//!
//! ```no_run
//! use ember_kfc::{Archive, type_hash};
//!
//! let mut archive = Archive::open("enshrouded_server.kfc".as_ref())?;
//! let wanted = type_hash("keen::ecs::TemplateResource");
//! let templates: Vec<_> = archive
//!     .directory()
//!     .by_type(wanted)
//!     .map(|resource| resource.id)
//!     .collect();
//! for id in templates {
//!     let bytes = archive.read_resource(id)?;
//!     let _ = bytes.len();
//! }
//! # Ok::<(), ember_kfc::Error>(())
//! ```
//!
//! # Scope
//!
//! Reading only. Writing a container needs the content hash, which runs four
//! parallel AES rounds over the data, and the blob serialization rules for
//! laying a resource out. A reader needs neither, because every hash it looks up
//! is already stored, so neither is here.
//!
//! Decoding an arbitrary resource's fields needs the reflection registry, which
//! lives in the executable rather than in either data file. This crate hands a
//! caller a resource's type hash and its bytes. The one layout it decodes is the
//! localization table in [`loca`], which is small enough to be self-checking
//! from bytes alone.

mod archive;
mod blob;
mod directory;
mod error;
mod hash;
pub mod loca;

pub use archive::Archive;
pub use blob::{BlobArray, BlobString};
pub use directory::{ChunkInfo, ContainerInfo, ContentEntry, Directory, Resource, StreamInfo};
pub use error::{Error, Result};
pub use hash::{ContentHash, ResourceId, fnv1a32, type_hash};
