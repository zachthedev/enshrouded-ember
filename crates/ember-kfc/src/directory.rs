//! The `.kfc` directory: the uncompressed index every other file is read through.
//!
//! The header is a magic word, a directory size, two words of padding, then
//! sixteen location records in a fixed order. A location is a relative offset
//! and a count, and the offset counts from the position of the offset field
//! itself. A relative offset of zero marks a location the build does not use.
//!
//! Six of the sixteen locations hold bucket arrays and resource bundles. Those
//! accelerate lookup and group resources by type, and walking the parallel key
//! and value arrays produces the same mapping without them, so this reader skips
//! them.

use std::collections::HashMap;

use crate::blob::{read_u32, read_u64};
use crate::error::{Error, Result};
use crate::hash::{ContentHash, ResourceId};

/// The ASCII bytes `KFC3`, read as a little-endian word.
const MAGIC: u32 = 0x3343_464b;

/// Where the location records start.
const LOCATIONS_AT: usize = 0x10;

/// Bytes one location record occupies.
const LOCATION_STRIDE: usize = 8;

/// How many location records the header carries.
const LOCATION_COUNT: usize = 16;

/// The location records this reader takes, by position in the header.
mod slot {
    pub(super) const VERSION: usize = 0;
    pub(super) const CONTAINERS: usize = 1;
    pub(super) const RESOURCE_STREAMS: usize = 4;
    pub(super) const CONTENT_KEYS: usize = 7;
    pub(super) const CONTENT_VALUES: usize = 8;
    pub(super) const RESOURCE_KEYS: usize = 10;
    pub(super) const RESOURCE_VALUES: usize = 11;
    pub(super) const RESOURCE_CHUNKS: usize = 15;
}

/// The name each location record carries, for a refusal that names the table.
const LOCATION_NAMES: [&str; LOCATION_COUNT] = [
    "version",
    "containers",
    "unused0",
    "unused1",
    "resource streams",
    "resource indices",
    "content buckets",
    "content keys",
    "content values",
    "resource buckets",
    "resource keys",
    "resource values",
    "resource bundle buckets",
    "resource bundle keys",
    "resource bundle values",
    "resource chunks",
];

/// Where one table sits and how many entries it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Location {
    /// The absolute file offset the table starts at.
    at: usize,
    /// How many entries it holds.
    count: usize,
}

/// One `.dat` container, as the directory declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerInfo {
    /// The size the `.dat` file holding this container must be.
    pub size: u64,
    /// How many content blobs it holds.
    pub count: u64,
}

impl ContainerInfo {
    /// Bytes one record occupies.
    const STRIDE: usize = 16;
}

/// The decompressed resource stream the payload file expands to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    /// The stream's total length once every chunk is decompressed.
    pub uncompressed_size: u64,
    /// The zstd frames' total length, slot padding excluded.
    pub compressed_size: u64,
    /// How many resources the stream holds.
    pub count: u64,
}

impl StreamInfo {
    /// Bytes one record occupies.
    const STRIDE: usize = 12;
}

/// One zstd frame in the payload file, and where it lands in the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkInfo {
    /// Where the slot starts in the payload file.
    pub offset: u64,
    /// Bytes the slot occupies: the frame, then padding out to the slot.
    pub size: u64,
    /// Bytes of zstd frame inside the slot.
    pub frame_size: u64,
    /// Where the chunk starts in the decompressed stream.
    pub stream_offset: u64,
    /// The chunk's decompressed length.
    pub stream_size: u64,
}

impl ChunkInfo {
    /// Bytes one record occupies.
    const STRIDE: usize = 20;

    /// Where the frame ends in the payload file, padding excluded.
    #[must_use]
    pub const fn frame_end(&self) -> u64 {
        self.offset + self.frame_size
    }

    /// Where the chunk ends in the decompressed stream.
    #[must_use]
    pub const fn stream_end(&self) -> u64 {
        self.stream_offset + self.stream_size
    }
}

/// One resource: an identity, and a span in the decompressed stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resource {
    /// The id the directory keys this resource on.
    pub id: ResourceId,
    /// Where the resource starts in the decompressed stream.
    pub offset: u64,
    /// How many bytes it occupies.
    pub size: u64,
}

impl Resource {
    /// Bytes one value record occupies.
    const VALUE_STRIDE: usize = 8;

    /// Where the resource ends in the decompressed stream.
    #[must_use]
    pub const fn end(&self) -> u64 {
        self.offset + self.size
    }
}

/// Where one content blob sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentEntry {
    /// Where the blob starts inside its `.dat` container.
    pub offset: u64,
    /// Flags the directory carries for this blob.
    pub flags: u16,
    /// Which `.dat` container holds it.
    pub container_index: u16,
}

impl ContentEntry {
    /// Bytes one record occupies.
    const STRIDE: usize = 16;
}

/// The parsed `.kfc` directory.
///
/// Every table a reader needs is decoded at construction and the file's bytes
/// are dropped, so a `Directory` is self-contained. The bucket arrays and the
/// resource bundles are skipped, because walking the parallel key and value
/// arrays yields the same mapping.
#[derive(Debug, Clone)]
pub struct Directory {
    version: String,
    containers: Vec<ContainerInfo>,
    stream: StreamInfo,
    chunks: Vec<ChunkInfo>,
    resources: Vec<Resource>,
    by_id: HashMap<ResourceId, usize>,
    contents: HashMap<ContentHash, ContentEntry>,
}

impl Directory {
    /// Parse a `.kfc` file's bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Magic`] when the first word is not `KFC3`,
    /// [`Error::Truncated`] when a table runs past the file,
    /// [`Error::StreamCount`] when the directory does not declare exactly one
    /// decompressed stream, [`Error::Unpaired`] when two parallel arrays
    /// disagree on length, and one of the chunk errors when the chunks do not
    /// tile the stream and the payload file.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let magic = read_u32(bytes, 0, "the directory header")?;
        if magic != MAGIC {
            return Err(Error::Magic { found: magic });
        }
        let locations = read_locations(bytes)?;

        let version = read_version(bytes, locations[slot::VERSION])?;
        let containers = read_containers(bytes, locations[slot::CONTAINERS])?;
        let stream = read_stream(bytes, locations[slot::RESOURCE_STREAMS])?;
        let chunks = read_chunks(bytes, locations[slot::RESOURCE_CHUNKS])?;
        check_chunks(&chunks, stream)?;
        let resources = read_resources(
            bytes,
            locations[slot::RESOURCE_KEYS],
            locations[slot::RESOURCE_VALUES],
        )?;
        let contents = read_contents(
            bytes,
            locations[slot::CONTENT_KEYS],
            locations[slot::CONTENT_VALUES],
        )?;

        let by_id = resources
            .iter()
            .enumerate()
            .map(|(index, resource)| (resource.id, index))
            .collect();
        Ok(Self {
            version,
            containers,
            stream,
            chunks,
            resources,
            by_id,
            contents,
        })
    }

    /// The build's version line: the revision, the branch and the timestamp,
    /// joined by `|`.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Every `.dat` container the build declares, in container-index order.
    #[must_use]
    pub fn containers(&self) -> &[ContainerInfo] {
        &self.containers
    }

    /// The decompressed resource stream the payload expands to.
    #[must_use]
    pub const fn stream(&self) -> StreamInfo {
        self.stream
    }

    /// Every zstd chunk, in payload order.
    #[must_use]
    pub fn chunks(&self) -> &[ChunkInfo] {
        &self.chunks
    }

    /// Every resource, in directory order.
    #[must_use]
    pub fn resources(&self) -> &[Resource] {
        &self.resources
    }

    /// The resource this id names.
    #[must_use]
    pub fn resource(&self, id: ResourceId) -> Option<&Resource> {
        self.by_id
            .get(&id)
            .and_then(|index| self.resources.get(*index))
    }

    /// Every resource whose type hash matches, in directory order.
    ///
    /// The hash comes from [`type_hash`](crate::type_hash) over a fully
    /// qualified type name.
    pub fn by_type(&self, type_hash: u32) -> impl Iterator<Item = &Resource> {
        self.resources
            .iter()
            .filter(move |resource| resource.id.type_hash == type_hash)
    }

    /// Every type hash present, with how many resources carry it.
    #[must_use]
    pub fn type_counts(&self) -> HashMap<u32, usize> {
        let mut counts: HashMap<u32, usize> = HashMap::new();
        for resource in &self.resources {
            *counts.entry(resource.id.type_hash).or_default() += 1;
        }
        counts
    }

    /// Where the blob this hash names sits.
    #[must_use]
    pub fn content(&self, hash: ContentHash) -> Option<&ContentEntry> {
        self.contents.get(&hash)
    }

    /// How many content blobs the directory indexes.
    #[must_use]
    pub fn content_count(&self) -> usize {
        self.contents.len()
    }
}

/// Read the sixteen location records, resolving each to an absolute offset.
fn read_locations(bytes: &[u8]) -> Result<[Location; LOCATION_COUNT]> {
    let mut out = [Location { at: 0, count: 0 }; LOCATION_COUNT];
    for (index, location) in out.iter_mut().enumerate() {
        let name = LOCATION_NAMES[index];
        let at = LOCATIONS_AT + index * LOCATION_STRIDE;
        let relative = read_u32(bytes, at, name)? as usize;
        let count = read_u32(bytes, at + 4, name)? as usize;
        // A relative offset of zero marks a location this build does not use.
        *location = if relative == 0 {
            Location { at: 0, count: 0 }
        } else {
            Location {
                at: at + relative,
                count,
            }
        };
    }
    Ok(out)
}

/// Check that a table's whole span sits inside the file.
fn span(bytes: &[u8], location: Location, stride: usize, table: &'static str) -> Result<()> {
    let need = location.count.checked_mul(stride).ok_or(Error::Truncated {
        table,
        at: location.at,
        need: usize::MAX,
        have: bytes.len(),
    })?;
    let end = location.at.checked_add(need).ok_or(Error::Truncated {
        table,
        at: location.at,
        need,
        have: bytes.len(),
    })?;
    if end > bytes.len() {
        return Err(Error::Truncated {
            table,
            at: location.at,
            need,
            have: bytes.len(),
        });
    }
    Ok(())
}

/// The version line, which is ASCII and carries no terminator.
fn read_version(bytes: &[u8], location: Location) -> Result<String> {
    const TABLE: &str = "the version string";
    span(bytes, location, 1, TABLE)?;
    let raw = bytes
        .get(location.at..location.at + location.count)
        .ok_or(Error::Truncated {
            table: TABLE,
            at: location.at,
            need: location.count,
            have: bytes.len(),
        })?;
    Ok(String::from_utf8_lossy(raw).into_owned())
}

/// The `.dat` container table.
fn read_containers(bytes: &[u8], location: Location) -> Result<Vec<ContainerInfo>> {
    const TABLE: &str = "the container table";
    span(bytes, location, ContainerInfo::STRIDE, TABLE)?;
    (0..location.count)
        .map(|index| {
            let at = location.at + index * ContainerInfo::STRIDE;
            Ok(ContainerInfo {
                size: read_u64(bytes, at, TABLE)?,
                count: read_u64(bytes, at + 8, TABLE)?,
            })
        })
        .collect()
}

/// The one decompressed stream the payload expands to.
fn read_stream(bytes: &[u8], location: Location) -> Result<StreamInfo> {
    const TABLE: &str = "the resource stream table";
    span(bytes, location, StreamInfo::STRIDE, TABLE)?;
    if location.count != 1 {
        return Err(Error::StreamCount {
            count: location.count,
        });
    }
    Ok(StreamInfo {
        uncompressed_size: u64::from(read_u32(bytes, location.at, TABLE)?),
        compressed_size: u64::from(read_u32(bytes, location.at + 4, TABLE)?),
        count: u64::from(read_u32(bytes, location.at + 8, TABLE)?),
    })
}

/// The zstd chunk table.
fn read_chunks(bytes: &[u8], location: Location) -> Result<Vec<ChunkInfo>> {
    const TABLE: &str = "the chunk table";
    span(bytes, location, ChunkInfo::STRIDE, TABLE)?;
    (0..location.count)
        .map(|index| {
            let at = location.at + index * ChunkInfo::STRIDE;
            Ok(ChunkInfo {
                offset: u64::from(read_u32(bytes, at, TABLE)?),
                size: u64::from(read_u32(bytes, at + 4, TABLE)?),
                frame_size: u64::from(read_u32(bytes, at + 8, TABLE)?),
                stream_offset: u64::from(read_u32(bytes, at + 12, TABLE)?),
                stream_size: u64::from(read_u32(bytes, at + 16, TABLE)?),
            })
        })
        .collect()
}

/// The chunks tile the payload file and the decompressed stream, with no gap and
/// no overlap.
///
/// Reading a resource copies bytes out of whichever chunks its span touches, so
/// a gap would silently shorten a resource and an overlap would silently
/// duplicate bytes inside one.
fn check_chunks(chunks: &[ChunkInfo], stream: StreamInfo) -> Result<()> {
    let mut slot_end = 0u64;
    let mut stream_end = 0u64;
    let mut frame_total = 0u64;
    for (index, chunk) in chunks.iter().enumerate() {
        if chunk.frame_size > chunk.size {
            return Err(Error::FrameOverrun {
                index,
                frame: chunk.frame_size,
                slot: chunk.size,
            });
        }
        if chunk.offset != slot_end {
            return Err(Error::ChunkSlotGap {
                index,
                found: chunk.offset,
                expected: slot_end,
            });
        }
        if chunk.stream_offset != stream_end {
            return Err(Error::ChunkStreamGap {
                index,
                found: chunk.stream_offset,
                expected: stream_end,
            });
        }
        slot_end = chunk.offset + chunk.size;
        stream_end = chunk.stream_offset + chunk.stream_size;
        frame_total += chunk.frame_size;
    }
    if stream_end != stream.uncompressed_size {
        return Err(Error::StreamSize {
            found: stream_end,
            expected: stream.uncompressed_size,
        });
    }
    if frame_total != stream.compressed_size {
        return Err(Error::FrameTotal {
            found: frame_total,
            expected: stream.compressed_size,
        });
    }
    Ok(())
}

/// The resource key and value arrays, walked in parallel.
fn read_resources(bytes: &[u8], keys: Location, values: Location) -> Result<Vec<Resource>> {
    const TABLE: &str = "the resource table";
    if keys.count != values.count {
        return Err(Error::Unpaired {
            keys: "resource keys",
            key_count: keys.count,
            values: "resource values",
            value_count: values.count,
        });
    }
    span(bytes, keys, ResourceId::STRIDE, TABLE)?;
    span(bytes, values, Resource::VALUE_STRIDE, TABLE)?;
    (0..keys.count)
        .map(|index| {
            let key_at = keys.at + index * ResourceId::STRIDE;
            let value_at = values.at + index * Resource::VALUE_STRIDE;
            let mut guid = [0u8; 16];
            guid.copy_from_slice(bytes.get(key_at..key_at + 16).ok_or(Error::Truncated {
                table: TABLE,
                at: key_at,
                need: 16,
                have: bytes.len(),
            })?);
            Ok(Resource {
                id: ResourceId {
                    guid,
                    type_hash: read_u32(bytes, key_at + 16, TABLE)?,
                    part_index: read_u32(bytes, key_at + 20, TABLE)?,
                },
                offset: u64::from(read_u32(bytes, value_at, TABLE)?),
                size: u64::from(read_u32(bytes, value_at + 4, TABLE)?),
            })
        })
        .collect()
}

/// The content key and value arrays, walked in parallel into a map.
fn read_contents(
    bytes: &[u8],
    keys: Location,
    values: Location,
) -> Result<HashMap<ContentHash, ContentEntry>> {
    const TABLE: &str = "the content table";
    if keys.count != values.count {
        return Err(Error::Unpaired {
            keys: "content keys",
            key_count: keys.count,
            values: "content values",
            value_count: values.count,
        });
    }
    span(bytes, keys, ContentHash::STRIDE, TABLE)?;
    span(bytes, values, ContentEntry::STRIDE, TABLE)?;
    let mut out = HashMap::with_capacity(keys.count);
    for index in 0..keys.count {
        let key_at = keys.at + index * ContentHash::STRIDE;
        let value_at = values.at + index * ContentEntry::STRIDE;
        let hash = ContentHash::new(
            read_u32(bytes, key_at, TABLE)?,
            [
                read_u32(bytes, key_at + 4, TABLE)?,
                read_u32(bytes, key_at + 8, TABLE)?,
                read_u32(bytes, key_at + 12, TABLE)?,
            ],
        );
        let entry = ContentEntry {
            offset: u64::from(read_u32(bytes, value_at, TABLE)?),
            flags: read_u16(bytes, value_at + 4, TABLE)?,
            container_index: read_u16(bytes, value_at + 6, TABLE)?,
        };
        out.insert(hash, entry);
    }
    Ok(out)
}

/// Read a little-endian `u16`, or say what did not fit.
fn read_u16(bytes: &[u8], at: usize, table: &'static str) -> Result<u16> {
    let slice = bytes.get(at..at + 2).ok_or(Error::Truncated {
        table,
        at,
        need: 2,
        have: bytes.len(),
    })?;
    let mut word = [0u8; 2];
    word.copy_from_slice(slice);
    Ok(u16::from_le_bytes(word))
}

#[cfg(test)]
mod tests {
    use super::{ChunkInfo, Directory, LOCATION_COUNT, LOCATION_STRIDE, LOCATIONS_AT, slot};
    use crate::error::Error;
    use crate::hash::ResourceId;

    /// A directory laid out the way the file is: a header, sixteen locations,
    /// then whatever tables the case needs.
    struct Builder {
        bytes: Vec<u8>,
    }

    impl Builder {
        /// A header with the magic in place and every location unused.
        fn new() -> Self {
            let mut bytes = vec![0u8; LOCATIONS_AT + LOCATION_COUNT * LOCATION_STRIDE];
            bytes[0..4].copy_from_slice(b"KFC3");
            bytes[8..12].copy_from_slice(&12u32.to_le_bytes());
            Self { bytes }
        }

        /// Append a table and point a location record at it.
        fn table(mut self, index: usize, count: u32, data: &[u8]) -> Self {
            let record = LOCATIONS_AT + index * LOCATION_STRIDE;
            let start = self.bytes.len();
            let relative = u32::try_from(start - record).expect("the table is close by");
            self.bytes[record..record + 4].copy_from_slice(&relative.to_le_bytes());
            self.bytes[record + 4..record + 8].copy_from_slice(&count.to_le_bytes());
            self.bytes.extend_from_slice(data);
            self
        }

        fn build(self) -> Vec<u8> {
            self.bytes
        }
    }

    /// Words as a little-endian byte run.
    fn words(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// One `StreamInfo` record.
    fn stream(uncompressed: u32, compressed: u32, count: u32) -> Vec<u8> {
        words(&[uncompressed, compressed, count])
    }

    /// One chunk record.
    fn chunk(offset: u32, size: u32, frame: u32, stream_offset: u32, stream_size: u32) -> Vec<u8> {
        words(&[offset, size, frame, stream_offset, stream_size])
    }

    /// The smallest directory that parses: one stream, one chunk, no resources.
    fn minimal() -> Vec<u8> {
        Builder::new()
            .table(slot::VERSION, 3, b"1|2")
            .table(slot::RESOURCE_STREAMS, 1, &stream(0x20, 0x10, 0))
            .table(slot::RESOURCE_CHUNKS, 1, &chunk(0, 0x40, 0x10, 0, 0x20))
            .build()
    }

    #[test]
    fn a_minimal_directory_parses() {
        let directory = Directory::parse(&minimal()).expect("the directory parses");
        assert_eq!(directory.version(), "1|2");
        assert_eq!(directory.stream().uncompressed_size, 0x20);
        assert_eq!(directory.chunks().len(), 1);
        assert!(directory.resources().is_empty());
    }

    #[test]
    fn a_location_offset_counts_from_its_own_record() {
        let bytes = minimal();
        let record = LOCATIONS_AT + slot::VERSION * LOCATION_STRIDE;
        let relative = u32::from_le_bytes([
            bytes[record],
            bytes[record + 1],
            bytes[record + 2],
            bytes[record + 3],
        ]) as usize;
        let at = record + relative;
        assert_eq!(&bytes[at..at + 3], b"1|2");
    }

    #[test]
    fn a_directory_without_the_magic_is_refused() {
        let mut bytes = minimal();
        bytes[0..4].copy_from_slice(b"KFC2");
        assert!(matches!(
            Directory::parse(&bytes),
            Err(Error::Magic { found }) if found == 0x3243_464b
        ));
    }

    #[test]
    fn a_header_shorter_than_the_location_records_is_refused() {
        let bytes = minimal();
        for length in [0usize, 4, 0x10, 0x40] {
            assert!(
                matches!(
                    Directory::parse(&bytes[..length]),
                    Err(Error::Magic { .. } | Error::Truncated { .. })
                ),
                "a {length}-byte header"
            );
        }
    }

    #[test]
    fn a_table_running_past_the_file_is_refused() {
        let bytes = Builder::new()
            .table(slot::RESOURCE_STREAMS, 1, &stream(0x20, 0x10, 0))
            .table(slot::RESOURCE_CHUNKS, 1, &chunk(0, 0x40, 0x10, 0, 0x20))
            // Nine records declared and one written.
            .table(slot::CONTAINERS, 9, &words(&[1, 0, 1, 0]))
            .build();
        assert!(matches!(
            Directory::parse(&bytes),
            Err(Error::Truncated { .. })
        ));
    }

    #[test]
    fn a_directory_declaring_more_than_one_stream_is_refused() {
        let bytes = Builder::new()
            .table(
                slot::RESOURCE_STREAMS,
                2,
                &[stream(0x20, 0x10, 0), stream(0x20, 0x10, 0)].concat(),
            )
            .table(slot::RESOURCE_CHUNKS, 1, &chunk(0, 0x40, 0x10, 0, 0x20))
            .build();
        assert!(matches!(
            Directory::parse(&bytes),
            Err(Error::StreamCount { count: 2 })
        ));
    }

    #[test]
    fn parallel_arrays_that_disagree_on_length_are_refused() {
        let bytes = Builder::new()
            .table(slot::RESOURCE_STREAMS, 1, &stream(0x20, 0x10, 0))
            .table(slot::RESOURCE_CHUNKS, 1, &chunk(0, 0x40, 0x10, 0, 0x20))
            .table(slot::RESOURCE_KEYS, 2, &[0u8; 64])
            .table(slot::RESOURCE_VALUES, 1, &[0u8; 8])
            .build();
        assert!(matches!(
            Directory::parse(&bytes),
            Err(Error::Unpaired {
                key_count: 2,
                value_count: 1,
                ..
            })
        ));
    }

    /// A gap or an overlap in the chunk run would silently shorten or duplicate
    /// bytes inside a resource that spans the boundary.
    #[test]
    fn chunks_that_do_not_tile_are_refused() {
        let good = [
            chunk(0, 0x40, 0x10, 0, 0x20),
            chunk(0x40, 0x40, 0x10, 0x20, 0x20),
        ]
        .concat();
        let parsed = Directory::parse(
            &Builder::new()
                .table(slot::RESOURCE_STREAMS, 1, &stream(0x40, 0x20, 0))
                .table(slot::RESOURCE_CHUNKS, 2, &good)
                .build(),
        );
        assert!(parsed.is_ok(), "two chunks that tile parse");

        let cases: &[(&str, Vec<u8>, u32)] = &[
            (
                "a gap in the payload file",
                [
                    chunk(0, 0x40, 0x10, 0, 0x20),
                    chunk(0x80, 0x40, 0x10, 0x20, 0x20),
                ]
                .concat(),
                0x40,
            ),
            (
                "a gap in the decompressed stream",
                [
                    chunk(0, 0x40, 0x10, 0, 0x20),
                    chunk(0x40, 0x40, 0x10, 0x40, 0x20),
                ]
                .concat(),
                0x40,
            ),
            (
                "an overlap in the decompressed stream",
                [
                    chunk(0, 0x40, 0x10, 0, 0x20),
                    chunk(0x40, 0x40, 0x10, 0x10, 0x20),
                ]
                .concat(),
                0x40,
            ),
        ];
        for (name, table, size) in cases {
            let bytes = Builder::new()
                .table(slot::RESOURCE_STREAMS, 1, &stream(*size, 0x20, 0))
                .table(slot::RESOURCE_CHUNKS, 2, table)
                .build();
            assert!(
                matches!(
                    Directory::parse(&bytes),
                    Err(Error::ChunkSlotGap { .. } | Error::ChunkStreamGap { .. })
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn a_frame_larger_than_its_slot_is_refused() {
        let bytes = Builder::new()
            .table(slot::RESOURCE_STREAMS, 1, &stream(0x20, 0x80, 0))
            .table(slot::RESOURCE_CHUNKS, 1, &chunk(0, 0x40, 0x80, 0, 0x20))
            .build();
        assert!(matches!(
            Directory::parse(&bytes),
            Err(Error::FrameOverrun {
                index: 0,
                frame: 0x80,
                slot: 0x40
            })
        ));
    }

    #[test]
    fn chunks_that_do_not_cover_the_declared_stream_are_refused() {
        let bytes = Builder::new()
            .table(slot::RESOURCE_STREAMS, 1, &stream(0x100, 0x10, 0))
            .table(slot::RESOURCE_CHUNKS, 1, &chunk(0, 0x40, 0x10, 0, 0x20))
            .build();
        assert!(matches!(
            Directory::parse(&bytes),
            Err(Error::StreamSize {
                found: 0x20,
                expected: 0x100
            })
        ));
    }

    #[test]
    fn a_resource_is_found_by_its_id_and_by_its_type() {
        let mut key = vec![0u8; ResourceId::STRIDE];
        key[0..16].copy_from_slice(&[0xaa; 16]);
        key[16..20].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        key[20..24].copy_from_slice(&7u32.to_le_bytes());
        let bytes = Builder::new()
            .table(slot::RESOURCE_STREAMS, 1, &stream(0x20, 0x10, 1))
            .table(slot::RESOURCE_CHUNKS, 1, &chunk(0, 0x40, 0x10, 0, 0x20))
            .table(slot::RESOURCE_KEYS, 1, &key)
            .table(slot::RESOURCE_VALUES, 1, &words(&[0x10, 0x08]))
            .build();

        let directory = Directory::parse(&bytes).expect("the directory parses");
        let id = ResourceId {
            guid: [0xaa; 16],
            type_hash: 0x1234_5678,
            part_index: 7,
        };
        let found = directory.resource(id).expect("the resource is indexed");
        assert_eq!(found.offset, 0x10);
        assert_eq!(found.size, 0x08);
        assert_eq!(found.end(), 0x18);
        assert_eq!(directory.by_type(0x1234_5678).count(), 1);
        assert_eq!(directory.by_type(0).count(), 0);
        assert_eq!(directory.type_counts().get(&0x1234_5678), Some(&1));
    }

    #[test]
    fn a_content_hash_finds_its_entry() {
        let key = words(&[0x40, 0x1111_1111, 0x2222_2222, 0x3333_3333]);
        let mut value = words(&[0x1000]);
        value.extend_from_slice(&0u16.to_le_bytes());
        value.extend_from_slice(&5u16.to_le_bytes());
        value.extend_from_slice(&[0u8; 8]);
        let bytes = Builder::new()
            .table(slot::RESOURCE_STREAMS, 1, &stream(0x20, 0x10, 0))
            .table(slot::RESOURCE_CHUNKS, 1, &chunk(0, 0x40, 0x10, 0, 0x20))
            .table(slot::CONTENT_KEYS, 1, &key)
            .table(slot::CONTENT_VALUES, 1, &value)
            .build();

        let directory = Directory::parse(&bytes).expect("the directory parses");
        let hash = crate::hash::ContentHash::new(0x40, [0x1111_1111, 0x2222_2222, 0x3333_3333]);
        let entry = directory.content(hash).expect("the blob is indexed");
        assert_eq!(entry.offset, 0x1000);
        assert_eq!(entry.container_index, 5);
        assert_eq!(entry.flags, 0);
        assert_eq!(directory.content_count(), 1);
        assert!(
            directory
                .content(crate::hash::ContentHash::new(0x41, [1, 2, 3]))
                .is_none()
        );
    }

    #[test]
    fn a_chunk_reports_both_of_its_ends() {
        let chunk = ChunkInfo {
            offset: 0x100,
            size: 0x80,
            frame_size: 0x70,
            stream_offset: 0x200,
            stream_size: 0x400,
        };
        assert_eq!(chunk.frame_end(), 0x170);
        assert_eq!(chunk.stream_end(), 0x600);
    }
}
