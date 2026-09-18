//! Why a container read refused.
//!
//! Every variant carries the numbers a reader needs to tell a corrupt file from
//! a build whose format moved. A refusal is always preferred to a plausible
//! answer, because the failures this format offers are silent ones: a record
//! stride that is one field out still decodes, and produces readable nonsense.

use std::path::PathBuf;

use crate::hash::{ContentHash, ResourceId};

/// The result of a container read.
pub type Result<T> = std::result::Result<T, Error>;

/// Why a container could not be read.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A file in the container set could not be read.
    #[error("reading {path}")]
    Io {
        /// The file the read was against.
        path: PathBuf,
        /// What the operating system said.
        #[source]
        source: std::io::Error,
    },

    /// The first word of the directory is not `KFC3`.
    #[error("the directory opens with {found:#010x}, which is not KFC3")]
    Magic {
        /// The word that was there.
        found: u32,
    },

    /// A table or record runs past the end of the file holding it.
    #[error("{table} needs {need} bytes at {at}, and the file holds {have}")]
    Truncated {
        /// The table or record that did not fit.
        table: &'static str,
        /// Where it starts.
        at: usize,
        /// How many bytes it needs from there.
        need: usize,
        /// How many bytes the file holds.
        have: usize,
    },

    /// The directory does not declare exactly one decompressed resource stream.
    #[error("the directory declares {count} resource streams, and a reader models one")]
    StreamCount {
        /// How many the directory declared.
        count: usize,
    },

    /// Two parallel arrays that address each other by index disagree on length.
    #[error("{keys} holds {key_count} entries and {values} holds {value_count}")]
    Unpaired {
        /// The key array's name.
        keys: &'static str,
        /// How many keys it holds.
        key_count: usize,
        /// The value array's name.
        values: &'static str,
        /// How many values it holds.
        value_count: usize,
    },

    /// The chunks do not tile the decompressed stream.
    #[error(
        "chunk {index} starts at {found} in the decompressed stream, and the chunks before it end at {expected}"
    )]
    ChunkStreamGap {
        /// Which chunk broke the run.
        index: usize,
        /// Where the chunk says it starts.
        found: u64,
        /// Where the run reached.
        expected: u64,
    },

    /// The chunk slots do not tile the payload file.
    #[error(
        "chunk {index} starts at payload offset {found}, and the slots before it end at {expected}"
    )]
    ChunkSlotGap {
        /// Which chunk broke the run.
        index: usize,
        /// Where the chunk says its slot starts.
        found: u64,
        /// Where the run reached.
        expected: u64,
    },

    /// A chunk declares a frame larger than the slot holding it.
    #[error("chunk {index} declares a {frame}-byte frame inside a {slot}-byte slot")]
    FrameOverrun {
        /// Which chunk.
        index: usize,
        /// The frame size the directory declares.
        frame: u64,
        /// The slot size the directory declares.
        slot: u64,
    },

    /// The frame sizes do not sum to the stream's declared compressed size.
    #[error("the chunk frames hold {found} bytes, and the directory declares {expected}")]
    FrameTotal {
        /// What the frame sizes sum to.
        found: u64,
        /// What the directory declares.
        expected: u64,
    },

    /// A span the directory declares does not fit in this target's address space.
    #[error("{what} is {size} bytes, which does not fit in memory on this target")]
    TooLarge {
        /// What was being read.
        what: &'static str,
        /// The size the directory declares.
        size: u64,
    },

    /// The chunks do not account for the whole decompressed stream.
    #[error(
        "the chunks decompress to {found} bytes, and the directory declares a {expected}-byte stream"
    )]
    StreamSize {
        /// What the chunk lengths sum to.
        found: u64,
        /// What the directory declares.
        expected: u64,
    },

    /// The payload file is not the size the chunk slots sum to.
    #[error("{path} holds {found} bytes, and the chunk slots sum to {expected}")]
    PayloadSize {
        /// The payload file.
        path: PathBuf,
        /// What the file holds.
        found: u64,
        /// What the slots sum to.
        expected: u64,
    },

    /// A chunk's frame did not decompress.
    ///
    /// Leaving a slot's padding inside the frame lands here, because zstd reads
    /// the trailing bytes as the start of another frame and finds no magic.
    #[error("chunk {index} did not decompress")]
    Decompress {
        /// Which chunk.
        index: usize,
        /// What zstd said.
        #[source]
        source: std::io::Error,
    },

    /// A chunk decompressed to a length the directory does not declare.
    #[error("chunk {index} decompressed to {found} bytes, and the directory declares {expected}")]
    ChunkSize {
        /// Which chunk.
        index: usize,
        /// What the frame produced.
        found: usize,
        /// What the directory declares.
        expected: u64,
    },

    /// A resource's span runs past the decompressed stream.
    #[error(
        "resource {id} spans {offset}..{end}, and the decompressed stream holds {stream} bytes"
    )]
    ResourceRange {
        /// The resource that does not fit.
        id: ResourceId,
        /// Where its span starts.
        offset: u64,
        /// Where its span ends.
        end: u64,
        /// How long the stream is.
        stream: u64,
    },

    /// The directory holds no resource with this id.
    #[error("resource {id} is not in the directory")]
    UnknownResource {
        /// The id that was asked for.
        id: ResourceId,
    },

    /// The directory holds no content blob with this hash.
    #[error("content {hash} is not in the directory")]
    UnknownContent {
        /// The hash that was asked for.
        hash: ContentHash,
    },

    /// A content entry names a container the directory does not declare.
    #[error("content {hash} names container {index}, and the directory declares {count}")]
    ContainerIndex {
        /// The blob's hash.
        hash: ContentHash,
        /// The container index the entry carries.
        index: usize,
        /// How many containers the directory declares.
        count: usize,
    },

    /// A content blob's span runs past the container holding it.
    #[error("content {hash} spans {offset}..{end} in container {index}, which holds {size} bytes")]
    ContentRange {
        /// The blob's hash.
        hash: ContentHash,
        /// Which container.
        index: usize,
        /// Where its span starts.
        offset: u64,
        /// Where its span ends.
        end: u64,
        /// The container's declared size.
        size: u64,
    },

    /// A `.dat` container is not the size the directory declares.
    #[error("{path} holds {found} bytes, and the directory declares {expected}")]
    ContainerSize {
        /// The container file.
        path: PathBuf,
        /// What the file holds.
        found: u64,
        /// What the directory declares.
        expected: u64,
    },

    /// A blob-relative offset points outside the blob.
    #[error("{field} points to {at}..{end}, and the blob holds {have} bytes")]
    BlobRange {
        /// Which field carried the offset.
        field: &'static str,
        /// Where the span starts.
        at: usize,
        /// Where the span ends.
        end: usize,
        /// How long the blob is.
        have: usize,
    },

    /// A string field does not hold UTF-8.
    #[error("{field} at {at} does not hold UTF-8")]
    NotUtf8 {
        /// Which field.
        field: &'static str,
        /// Where the string starts.
        at: usize,
    },

    /// The tag records do not end where the first string begins.
    ///
    /// This is the shape a wrong record stride takes. The records and the
    /// strings they point at are laid end to end, so a stride one field out
    /// leaves the array overlapping the text it addresses.
    #[error(
        "{count} tag records of {stride} bytes end at {end}, and the first string begins at {first}"
    )]
    TagRecords {
        /// How many records the header declares.
        count: usize,
        /// The stride this reader decodes at.
        stride: usize,
        /// Where the record array ends.
        end: usize,
        /// Where the first string begins.
        first: usize,
    },

    /// Tag ids do not ascend, which is the other shape a wrong stride takes.
    #[error("tag record {index} carries id {id}, which does not follow {previous}")]
    TagOrder {
        /// Which record broke the order.
        index: usize,
        /// The id it carries.
        id: u32,
        /// The id before it.
        previous: u32,
    },
}
