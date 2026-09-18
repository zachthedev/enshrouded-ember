//! The reader against a container set the test lays out itself.
//!
//! Nothing recovered from a Keen file is committed, so the fixture here is built
//! from scratch: real zstd frames padded into real slots, a real directory
//! indexing them, and real `.dat` containers beside it. The layout is the
//! format's; only the bytes are the test's own.
//!
//! This is where the break round lives. Each case corrupts one field and states
//! which guard must fire, because every failure this format offers is a silent
//! one: a chunk read untrimmed, a resource span one field out or a record stride
//! off by four all decode into something that looks like data.

use std::path::{Path, PathBuf};

use ember_kfc::{Archive, ContentHash, Error, ResourceId};

/// Bytes each chunk decompresses to in the fixture.
///
/// Small, and deliberately not a round number, so a resource laid across a
/// boundary lands at an offset nothing else would produce.
const CHUNK: usize = 1000;

/// Bytes each slot occupies, which is more than any frame here needs.
const SLOT: usize = 4096;

/// One resource the fixture indexes.
struct Entry {
    id: ResourceId,
    offset: usize,
    size: usize,
}

/// A container set on disk, and the bytes that went into it.
struct Fixture {
    dir: tempfile::TempDir,
    stream: Vec<u8>,
    entries: Vec<Entry>,
    blobs: Vec<(ContentHash, Vec<u8>)>,
}

impl Fixture {
    /// Lay a set out: three chunks of stream, four resources, two blobs in two
    /// containers.
    fn build() -> Self {
        Self::with(Doctor::none())
    }

    /// Lay a set out with one field bent.
    fn with(doctor: Doctor) -> Self {
        let dir = tempfile::tempdir().expect("a temp directory");
        // A 251-byte cycle, so no two chunk boundaries land on the same value
        // and a resource read from the wrong offset does not read back.
        let stream: Vec<u8> = (0..CHUNK * 3)
            .map(|index| u8::try_from(index % 251).expect("the cycle fits in a byte"))
            .collect();

        let entries = vec![
            Entry {
                id: id(1, 0xaaaa_0000),
                offset: 0,
                size: 16,
            },
            Entry {
                id: id(2, 0xaaaa_0000),
                offset: 100,
                size: 64,
            },
            // Straddles the first chunk boundary.
            Entry {
                id: id(3, 0xbbbb_0000),
                offset: CHUNK - 8,
                size: 32,
            },
            // Straddles the second, and reaches the last byte of the stream.
            Entry {
                id: id(4, 0xbbbb_0000),
                offset: CHUNK * 2 - 4,
                size: CHUNK + 4,
            },
        ];
        let blobs = vec![
            (ContentHash::new(48, [1, 2, 3]), vec![0x11u8; 48]),
            (ContentHash::new(32, [4, 5, 6]), vec![0x22u8; 32]),
        ];

        let frames: Vec<Vec<u8>> = stream
            .chunks(CHUNK)
            .enumerate()
            .map(|(index, chunk)| {
                let body = if doctor == Doctor::FrameProducesTooLittle && index == 0 {
                    &chunk[..chunk.len() - 1]
                } else {
                    chunk
                };
                zstd::bulk::compress(body, 3).expect("the fixture compresses")
            })
            .collect();

        write_payload(&dir.path().join("pack.kfc_resources"), &frames, doctor);
        write_containers(dir.path(), &blobs, doctor);
        let directory = directory(&stream, &frames, &entries, &blobs, doctor);
        std::fs::write(dir.path().join("pack.kfc"), directory).expect("the directory writes");

        Self {
            dir,
            stream,
            entries,
            blobs,
        }
    }

    fn kfc(&self) -> PathBuf {
        self.dir.path().join("pack.kfc")
    }

    fn open(&self) -> Result<Archive, Error> {
        Archive::open(&self.kfc())
    }
}

/// Which field a fixture bends, if any.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Doctor {
    /// Nothing is bent.
    None,
    /// Each chunk declares its whole padded slot as the frame, which is what a
    /// reader that skips the trim hands to zstd.
    PaddingInsideTheFrame,
    /// The first chunk's frame holds one byte less than the directory declares,
    /// so the tables still tile and only the frame disagrees.
    FrameProducesTooLittle,
    /// The last resource's span reaches past the declared stream.
    ResourcePastTheStream,
    /// A content entry's blob reaches past the container holding it.
    ContentPastTheContainer,
    /// The payload file carries a byte the chunk slots do not account for.
    PayloadTooLong,
    /// A `.dat` container is not the size the directory declares.
    ContainerTooShort,
}

impl Doctor {
    const fn none() -> Self {
        Self::None
    }
}

/// A resource id with a distinguishable GUID.
fn id(seed: u8, type_hash: u32) -> ResourceId {
    ResourceId {
        guid: [seed; 16],
        type_hash,
        part_index: 0,
    }
}

/// Write the payload: each frame padded out to its slot.
fn write_payload(path: &Path, frames: &[Vec<u8>], doctor: Doctor) {
    let mut bytes = Vec::with_capacity(frames.len() * SLOT);
    for frame in frames {
        assert!(frame.len() < SLOT, "a fixture frame fits in its slot");
        bytes.extend_from_slice(frame);
        bytes.resize(bytes.len() + (SLOT - frame.len()), 0);
    }
    if doctor == Doctor::PayloadTooLong {
        bytes.push(0);
    }
    std::fs::write(path, bytes).expect("the payload writes");
}

/// Write the `.dat` containers, one blob in each.
fn write_containers(dir: &Path, blobs: &[(ContentHash, Vec<u8>)], doctor: Doctor) {
    for (index, (_, data)) in blobs.iter().enumerate() {
        let mut bytes = vec![0u8; 0x1000];
        bytes.extend_from_slice(data);
        if doctor == Doctor::ContainerTooShort && index == 0 {
            bytes.pop();
        }
        std::fs::write(dir.join(format!("pack_{index:03}.dat")), bytes)
            .expect("a container writes");
    }
}

/// Build the `.kfc` directory over the fixture's tables.
fn directory(
    stream: &[u8],
    frames: &[Vec<u8>],
    entries: &[Entry],
    blobs: &[(ContentHash, Vec<u8>)],
    doctor: Doctor,
) -> Vec<u8> {
    const LOCATIONS_AT: usize = 0x10;
    const LOCATION_COUNT: usize = 16;
    const VERSION: &[u8] = b"1|^/fixture|2026-01-01";

    let mut bytes = vec![0u8; LOCATIONS_AT + LOCATION_COUNT * 8];
    bytes[0..4].copy_from_slice(b"KFC3");
    bytes[8..12].copy_from_slice(&12u32.to_le_bytes());

    let table = |bytes: &mut Vec<u8>, slot: usize, count: usize, data: &[u8]| {
        let record = LOCATIONS_AT + slot * 8;
        let relative = u32::try_from(bytes.len() - record).expect("the table is close by");
        bytes[record..record + 4].copy_from_slice(&relative.to_le_bytes());
        bytes[record + 4..record + 8]
            .copy_from_slice(&u32::try_from(count).expect("few entries").to_le_bytes());
        bytes.extend_from_slice(data);
    };

    // 0: version
    table(&mut bytes, 0, VERSION.len(), VERSION);

    // 1: containers
    let mut container_table = Vec::new();
    for (index, (hash, _)) in blobs.iter().enumerate() {
        let mut size = 0x1000u64 + u64::from(hash.size);
        if doctor == Doctor::ContentPastTheContainer && index == 0 {
            size -= 1;
        }
        container_table.extend_from_slice(&size.to_le_bytes());
        container_table.extend_from_slice(&1u64.to_le_bytes());
    }
    table(&mut bytes, 1, blobs.len(), &container_table);

    // 4: the one decompressed stream
    let compressed: usize = if doctor == Doctor::PaddingInsideTheFrame {
        frames.len() * SLOT
    } else {
        frames.iter().map(Vec::len).sum()
    };
    let declared_stream = if doctor == Doctor::ResourcePastTheStream {
        stream.len() - 8
    } else {
        stream.len()
    };
    let mut stream_table = Vec::new();
    stream_table.extend_from_slice(&word(declared_stream).to_le_bytes());
    stream_table.extend_from_slice(&word(compressed).to_le_bytes());
    stream_table.extend_from_slice(&word(entries.len()).to_le_bytes());
    table(&mut bytes, 4, 1, &stream_table);

    // 7 and 8: the content map
    let mut keys = Vec::new();
    let mut values = Vec::new();
    for (index, (hash, _)) in blobs.iter().enumerate() {
        keys.extend_from_slice(&hash.size.to_le_bytes());
        for word in hash.words {
            keys.extend_from_slice(&word.to_le_bytes());
        }
        values.extend_from_slice(&0x1000u32.to_le_bytes());
        values.extend_from_slice(&0u16.to_le_bytes());
        values.extend_from_slice(&u16::try_from(index).expect("few containers").to_le_bytes());
        values.extend_from_slice(&[0u8; 8]);
    }
    table(&mut bytes, 7, blobs.len(), &keys);
    table(&mut bytes, 8, blobs.len(), &values);

    // 10 and 11: the resource map
    let mut keys = Vec::new();
    let mut values = Vec::new();
    for entry in entries {
        keys.extend_from_slice(&entry.id.guid);
        keys.extend_from_slice(&entry.id.type_hash.to_le_bytes());
        keys.extend_from_slice(&entry.id.part_index.to_le_bytes());
        keys.extend_from_slice(&[0u8; 8]);
        values.extend_from_slice(&word(entry.offset).to_le_bytes());
        values.extend_from_slice(&word(entry.size).to_le_bytes());
    }
    table(&mut bytes, 10, entries.len(), &keys);
    table(&mut bytes, 11, entries.len(), &values);

    // 15: the chunk table
    let mut chunks = Vec::new();
    let mut slot_at = 0usize;
    let mut stream_at = 0usize;
    for frame in frames {
        let declared = stream.len().min(stream_at + CHUNK) - stream_at;
        let frame_size = if doctor == Doctor::PaddingInsideTheFrame {
            SLOT
        } else {
            frame.len()
        };
        chunks.extend_from_slice(&word(slot_at).to_le_bytes());
        chunks.extend_from_slice(&word(SLOT).to_le_bytes());
        chunks.extend_from_slice(&word(frame_size).to_le_bytes());
        chunks.extend_from_slice(&word(stream_at).to_le_bytes());
        chunks.extend_from_slice(&word(declared).to_le_bytes());
        slot_at += SLOT;
        stream_at += declared;
    }
    table(&mut bytes, 15, frames.len(), &chunks);

    bytes
}

/// Narrow a fixture size to the `u32` the format stores it in.
fn word(value: usize) -> u32 {
    u32::try_from(value).expect("a fixture value fits in 32 bits")
}

#[test]
fn a_well_formed_set_reads_every_resource_back_byte_for_byte() {
    let fixture = Fixture::build();
    let mut archive = fixture.open().expect("the fixture opens");
    for entry in &fixture.entries {
        let bytes = archive
            .read_resource(entry.id)
            .unwrap_or_else(|err| panic!("reading {}: {err}", entry.id));
        assert_eq!(
            bytes,
            fixture.stream[entry.offset..entry.offset + entry.size],
            "{} did not read back",
            entry.id
        );
    }
}

/// Two of the fixture's resources cross a chunk boundary, which is the case a
/// reader gets wrong by decompressing one chunk and stopping.
#[test]
fn a_resource_spanning_chunks_reads_back_whole() {
    let fixture = Fixture::build();
    let mut archive = fixture.open().expect("the fixture opens");
    let straddling: Vec<&Entry> = fixture
        .entries
        .iter()
        .filter(|entry| entry.offset / CHUNK != (entry.offset + entry.size - 1) / CHUNK)
        .collect();
    assert_eq!(
        straddling.len(),
        2,
        "the fixture lays two across a boundary"
    );
    for entry in straddling {
        let bytes = archive.read_resource(entry.id).expect("the resource reads");
        assert_eq!(bytes.len(), entry.size);
        assert_eq!(
            bytes,
            fixture.stream[entry.offset..entry.offset + entry.size]
        );
    }
}

#[test]
fn every_content_blob_reads_back_byte_for_byte() {
    let fixture = Fixture::build();
    let archive = fixture.open().expect("the fixture opens");
    for (hash, data) in &fixture.blobs {
        let bytes = archive.read_content(*hash).expect("the blob reads");
        assert_eq!(&bytes, data, "content {hash} did not read back");
    }
    archive
        .verify_containers()
        .expect("the declared sizes match the files");
}

#[test]
fn a_resource_the_directory_does_not_carry_is_refused() {
    let fixture = Fixture::build();
    let mut archive = fixture.open().expect("the fixture opens");
    let absent = ResourceId {
        guid: [0xff; 16],
        type_hash: 0,
        part_index: 0,
    };
    assert!(matches!(
        archive.read_resource(absent),
        Err(Error::UnknownResource { .. })
    ));
}

#[test]
fn a_content_hash_the_directory_does_not_carry_is_refused() {
    let fixture = Fixture::build();
    let archive = fixture.open().expect("the fixture opens");
    assert!(matches!(
        archive.read_content(ContentHash::new(1, [0, 0, 0])),
        Err(Error::UnknownContent { .. })
    ));
}

/// The padding is the whole reason a plain `zstd -d` of a payload stops after
/// one chunk. A reader that hands the slot over instead of the frame gets a
/// refusal here rather than a short read.
#[test]
fn padding_left_inside_the_frame_is_refused() {
    let fixture = Fixture::with(Doctor::PaddingInsideTheFrame);
    let mut archive = fixture.open().expect("the doctored set still opens");
    let first = fixture.entries[0].id;
    assert!(
        matches!(archive.read_resource(first), Err(Error::Decompress { .. })),
        "an untrimmed slot must refuse"
    );
}

#[test]
fn a_chunk_that_decompresses_to_the_wrong_length_is_refused() {
    let fixture = Fixture::with(Doctor::FrameProducesTooLittle);
    let mut archive = fixture.open().expect("the doctored set still opens");
    let first = fixture.entries[0].id;
    assert!(
        matches!(
            archive.read_resource(first),
            Err(Error::Decompress { .. } | Error::ChunkSize { .. })
        ),
        "a chunk that does not produce the declared length must refuse"
    );
}

#[test]
fn a_resource_running_past_the_stream_is_refused() {
    let fixture = Fixture::with(Doctor::ResourcePastTheStream);
    // The chunk table still covers the real stream, so the directory refuses
    // before an archive exists.
    assert!(
        matches!(fixture.open(), Err(Error::StreamSize { .. })),
        "a stream shorter than its chunks must refuse at parse"
    );
}

#[test]
fn a_content_blob_running_past_its_container_is_refused() {
    let fixture = Fixture::with(Doctor::ContentPastTheContainer);
    let archive = fixture.open().expect("the doctored set still opens");
    let hash = fixture.blobs[0].0;
    assert!(
        matches!(archive.read_content(hash), Err(Error::ContentRange { .. })),
        "a blob reaching past its container must refuse"
    );
}

#[test]
fn a_payload_the_slots_do_not_account_for_is_refused() {
    let fixture = Fixture::with(Doctor::PayloadTooLong);
    assert!(
        matches!(fixture.open(), Err(Error::PayloadSize { .. })),
        "a payload with a byte the slots do not cover must refuse"
    );
}

#[test]
fn a_container_that_is_not_the_declared_size_is_refused() {
    let fixture = Fixture::with(Doctor::ContainerTooShort);
    let archive = fixture.open().expect("the doctored set still opens");
    assert!(
        matches!(
            archive.verify_containers(),
            Err(Error::ContainerSize { .. })
        ),
        "a container that is not the declared size must refuse"
    );
}

#[test]
fn a_directory_whose_magic_moved_is_refused() {
    let fixture = Fixture::build();
    let mut bytes = std::fs::read(fixture.kfc()).expect("the directory is readable");
    bytes[0..4].copy_from_slice(b"KFC4");
    std::fs::write(fixture.kfc(), bytes).expect("the directory writes");
    assert!(matches!(
        fixture.open(),
        Err(Error::Magic { found }) if found == 0x3443_464b
    ));
}

#[test]
fn a_missing_payload_names_the_file_it_looked_for() {
    let fixture = Fixture::build();
    let payload = fixture.dir.path().join("pack.kfc_resources");
    std::fs::remove_file(&payload).expect("the payload is removable");
    let Err(Error::Io { path, .. }) = fixture.open() else {
        panic!("an absent payload must refuse");
    };
    assert_eq!(path, payload);
}

#[test]
fn resources_are_found_by_type() {
    let fixture = Fixture::build();
    let archive = fixture.open().expect("the fixture opens");
    assert_eq!(archive.directory().by_type(0xaaaa_0000).count(), 2);
    assert_eq!(archive.directory().by_type(0xbbbb_0000).count(), 2);
    assert_eq!(archive.directory().by_type(0).count(), 0);
    assert_eq!(archive.directory().type_counts().len(), 2);
}

#[test]
fn container_paths_carry_a_three_digit_index() {
    let fixture = Fixture::build();
    let archive = fixture.open().expect("the fixture opens");
    assert!(
        archive.container_path(7).ends_with("pack_007.dat"),
        "a container path is the directory's stem, an underscore and three digits"
    );
}
