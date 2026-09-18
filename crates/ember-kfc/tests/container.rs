//! The reader against a real container set.
//!
//! Every case asserts a relationship the format holds to, never a number read
//! off one build. A declared container size equals the file beside it, the chunk
//! slots tile the payload, the language array closes on the resource's own size,
//! and the tag records end where the text they address begins. A build that
//! changes those numbers still passes; a build that breaks the format does not.
//!
//! Nothing recovered from a container is committed, so these run against builds
//! the caller fetched and skip cleanly when none is named.
//!
//! ```text
//! EMBER_SERVER_EXE=<path>\enshrouded_server.exe
//! EMBER_CLIENT_EXE=<path>\enshrouded.exe
//! cargo test -p ember-kfc -- --ignored
//! ```
//!
//! Both variables name an executable, and the container set sits beside it under
//! the same stem, which is how the format ships on the server and on the client
//! alike. The dedicated server carries no localization table, so the tag cases
//! need the client.
//!
//! The break round against bytes a test lays out itself is in `archive.rs`. What
//! is here is what only a real build can show.

use std::path::PathBuf;

use ember_kfc::loca::{LANGUAGE_STRIDE, LocaTag, TAG_STRIDE, TagCollection, decode_tags};
use ember_kfc::{Archive, ContentHash, Error, type_hash};

/// The variable naming the dedicated server executable.
const SERVER_VAR: &str = "EMBER_SERVER_EXE";

/// The variable naming the game client executable.
const CLIENT_VAR: &str = "EMBER_CLIENT_EXE";

/// The type every build ships entity templates as.
const TEMPLATE_TYPE: &str = "keen::ecs::TemplateResource";

/// Bytes the tag blob's own header occupies, before the first record.
const TAG_HEADER: usize = 8;

#[test]
#[ignore = "needs a fetched build, named by EMBER_SERVER_EXE or EMBER_CLIENT_EXE"]
fn the_directory_parses_on_every_build_named() {
    let Some(builds) = builds() else { return };
    for (name, archive) in builds {
        let directory = archive.directory();
        let version = directory.version();
        assert!(
            version.contains('|') && version.contains("^/"),
            "{name}: the version line is `{version}`, and a build states a revision, a caret path and a timestamp"
        );
        assert!(
            !directory.resources().is_empty(),
            "{name}: a build ships resources"
        );
        assert_eq!(
            u64::try_from(directory.resources().len()).expect("a resource count fits in 64 bits"),
            directory.stream().count,
            "{name}: the stream record and the key array must agree on the resource count"
        );
        assert!(
            !directory.chunks().is_empty(),
            "{name}: a payload is at least one chunk"
        );
        assert!(
            directory.content_count() > 0,
            "{name}: a build ships content blobs"
        );
    }
}

/// The chunk slots tile the payload file exactly, and the frames sum to the
/// compressed size the stream record declares.
///
/// `Archive::open` refuses a payload that is not the slot sum, and
/// `Directory::parse` refuses chunks that do not tile, so this states the
/// relationship the open already enforced and names the numbers when it moves.
#[test]
#[ignore = "needs a fetched build, named by EMBER_SERVER_EXE or EMBER_CLIENT_EXE"]
fn the_chunk_slots_tile_the_payload_file() {
    let Some(builds) = builds() else { return };
    for (name, archive) in builds {
        let directory = archive.directory();
        let slots: u64 = directory.chunks().iter().map(|chunk| chunk.size).sum();
        let frames: u64 = directory
            .chunks()
            .iter()
            .map(|chunk| chunk.frame_size)
            .sum();
        let on_disk = std::fs::metadata(payload_of(&name))
            .expect("the payload is readable")
            .len();
        assert_eq!(slots, on_disk, "{name}: the slots must sum to the payload");
        assert_eq!(
            frames,
            directory.stream().compressed_size,
            "{name}: the frames must sum to the declared compressed size"
        );
        assert!(
            frames < slots,
            "{name}: every slot is padded past its frame, which is what a reader has to trim"
        );
    }
}

#[test]
#[ignore = "needs a fetched build, named by EMBER_SERVER_EXE or EMBER_CLIENT_EXE"]
fn every_declared_container_matches_the_file_beside_it() {
    let Some(builds) = builds() else { return };
    for (name, archive) in builds {
        let count = archive.directory().containers().len();
        assert!(
            count.is_power_of_two(),
            "{name}: the format ships a power-of-two container count, and this build declares {count}"
        );
        archive
            .verify_containers()
            .unwrap_or_else(|err| panic!("{name}: {err}"));
    }
}

/// Reading the first and the last resource walks the first and the last chunk,
/// which is the payload's framing at both ends.
#[test]
#[ignore = "needs a fetched build, named by EMBER_SERVER_EXE or EMBER_CLIENT_EXE"]
fn resources_at_both_ends_of_the_stream_read_back_at_their_declared_size() {
    let Some(builds) = builds() else { return };
    for (name, mut archive) in builds {
        let resources = archive.directory().resources();
        let mut wanted = Vec::new();
        if let Some(first) = resources.first() {
            wanted.push((first.id, first.size));
        }
        if let Some(last) = resources.last() {
            wanted.push((last.id, last.size));
        }
        for (id, size) in wanted {
            let bytes = archive
                .read_resource(id)
                .unwrap_or_else(|err| panic!("{name}: reading {id}: {err}"));
            assert_eq!(
                u64::try_from(bytes.len()).expect("a resource length fits in 64 bits"),
                size,
                "{name}: {id} read short"
            );
        }
    }
}

#[test]
#[ignore = "needs a fetched build, named by EMBER_SERVER_EXE or EMBER_CLIENT_EXE"]
fn an_id_the_directory_does_not_carry_is_refused() {
    let Some(builds) = builds() else { return };
    for (name, mut archive) in builds {
        let mut absent = archive
            .directory()
            .resources()
            .first()
            .expect("a build ships resources")
            .id;
        absent.part_index = absent.part_index.wrapping_add(0x5a5a_5a5a);
        assert!(
            matches!(
                archive.read_resource(absent),
                Err(Error::UnknownResource { .. })
            ),
            "{name}: an id the directory does not carry must refuse"
        );

        let hash = ContentHash::new(0x20, [0xdead_beef, 0xdead_beef, 0xdead_beef]);
        assert!(
            archive.directory().content(hash).is_none(),
            "{name}: the probe hash must not name a real blob"
        );
        assert!(
            matches!(
                archive.read_content(hash),
                Err(Error::UnknownContent { .. })
            ),
            "{name}: a hash the directory does not carry must refuse"
        );
    }
}

/// The entity templates are the one table both builds ship in full, so the two
/// counts agreeing is the cross-build check the format offers.
#[test]
#[ignore = "needs both a server and a client build"]
fn both_builds_carry_the_same_entity_template_count() {
    let (Some(server), Some(client)) = (archive(SERVER_VAR), archive(CLIENT_VAR)) else {
        eprintln!("skipped: set both {SERVER_VAR} and {CLIENT_VAR} to run this");
        return;
    };
    let wanted = type_hash(TEMPLATE_TYPE);
    let on_server = server.directory().by_type(wanted).count();
    let on_client = client.directory().by_type(wanted).count();
    assert!(on_server > 0, "the server ships entity templates");
    assert_eq!(
        on_server, on_client,
        "the server and the client ship one entity template table between them"
    );
    assert!(
        server.directory().resources().len() < client.directory().resources().len(),
        "the server's resource set is a subset of the client's"
    );
}

#[test]
#[ignore = "needs both a server and a client build"]
fn the_server_ships_no_localization_table_and_the_client_ships_one() {
    let (Some(server), Some(client)) = (archive(SERVER_VAR), archive(CLIENT_VAR)) else {
        eprintln!("skipped: set both {SERVER_VAR} and {CLIENT_VAR} to run this");
        return;
    };
    assert_eq!(
        server.directory().by_type(TagCollection::TYPE_HASH).count(),
        0,
        "the dedicated server ships no localization table, so a tag id is client-only"
    );
    assert_eq!(
        client.directory().by_type(TagCollection::TYPE_HASH).count(),
        1,
        "the client ships exactly one localization table"
    );
}

/// The header sits at 0x10 and the entries follow it at 0x18, so the entries end
/// exactly where the resource does.
#[test]
#[ignore = "needs a fetched client build, named by EMBER_CLIENT_EXE"]
fn the_language_array_closes_on_the_resource_size() {
    let Some(mut client) = archive(CLIENT_VAR) else {
        eprintln!("skipped: set {CLIENT_VAR} to run this");
        return;
    };
    let bytes = collection_bytes(&mut client);
    let collection = TagCollection::parse(&bytes).expect("the localization resource parses");
    assert_eq!(
        0x18 + collection.languages.len() * LANGUAGE_STRIDE,
        bytes.len(),
        "{} languages of {LANGUAGE_STRIDE} bytes from 0x18 must close on the resource's {} bytes",
        collection.languages.len(),
        bytes.len()
    );
    assert!(
        collection.languages.iter().any(|(id, _)| *id == 1),
        "the client ships English, which is language id 1"
    );
    for (id, hash) in &collection.languages {
        assert!(
            client.directory().content(*hash).is_some(),
            "language {id} names a blob the directory carries"
        );
    }
}

/// The records and the text are laid end to end. A stride one field out leaves
/// the array overlapping the text it addresses, which is the failure that would
/// otherwise decode into readable nonsense.
#[test]
#[ignore = "needs a fetched client build, named by EMBER_CLIENT_EXE"]
fn the_tag_records_end_where_the_first_string_begins() {
    let Some(mut client) = archive(CLIENT_VAR) else {
        eprintln!("skipped: set {CLIENT_VAR} to run this");
        return;
    };
    let (blob, tags) = authoring_table(&mut client);
    assert!(!tags.is_empty(), "the authoring table ships tags");
    assert_eq!(
        TAG_HEADER + tags.len() * TAG_STRIDE,
        first_text(&tags),
        "{} records of {TAG_STRIDE} bytes after the {TAG_HEADER}-byte header must end where the text begins",
        tags.len()
    );
    assert!(
        tags.windows(2).all(|pair| pair[0].id < pair[1].id),
        "tag ids ascend"
    );
    assert!(
        tags.iter().any(|tag| !tag.text.is_empty()),
        "the authoring table carries text"
    );
    assert!(
        tags.iter().any(|tag| tag.arguments > 0),
        "some tags take arguments"
    );
    assert!(
        blob.len() > first_text(&tags),
        "the text follows the records rather than ending the blob"
    );
}

/// Every translated table decodes at the same stride and carries a subset of the
/// authoring language's ids.
#[test]
#[ignore = "needs a fetched client build, named by EMBER_CLIENT_EXE"]
fn every_language_decodes_at_the_same_stride() {
    let Some(mut client) = archive(CLIENT_VAR) else {
        eprintln!("skipped: set {CLIENT_VAR} to run this");
        return;
    };
    let bytes = collection_bytes(&mut client);
    let collection = TagCollection::parse(&bytes).expect("the localization resource parses");
    let (_, authoring) = authoring_table(&mut client);
    let authoring_ids: std::collections::HashSet<u32> =
        authoring.iter().map(|tag| tag.id).collect();

    for (id, hash) in &collection.languages {
        let blob = client
            .read_content(*hash)
            .unwrap_or_else(|err| panic!("language {id}: {err}"));
        let tags = decode_tags(&blob).unwrap_or_else(|err| panic!("language {id}: {err}"));
        assert_eq!(
            TAG_HEADER + tags.len() * TAG_STRIDE,
            first_text(&tags),
            "language {id} decodes at the same stride"
        );
        let extra = tags
            .iter()
            .filter(|tag| !authoring_ids.contains(&tag.id))
            .count();
        assert!(
            tags.len() <= authoring.len(),
            "language {id} carries {} tags against the authoring table's {}",
            tags.len(),
            authoring.len()
        );
        assert!(
            extra <= 1,
            "language {id} carries {extra} ids the authoring table does not"
        );
    }
}

/// The break round against real bytes: the record count is the one field a
/// wrong stride is indistinguishable from, and moving it must refuse.
#[test]
#[ignore = "needs a fetched client build, named by EMBER_CLIENT_EXE"]
fn a_real_tag_blob_with_the_record_count_moved_is_refused() {
    let Some(mut client) = archive(CLIENT_VAR) else {
        eprintln!("skipped: set {CLIENT_VAR} to run this");
        return;
    };
    let (blob, tags) = authoring_table(&mut client);
    let count = u32::try_from(tags.len()).expect("a tag count fits in 32 bits");

    let mut long = blob.clone();
    long[4..8].copy_from_slice(&(count + 1).to_le_bytes());
    assert!(
        matches!(
            decode_tags(&long),
            Err(Error::TagOrder { .. } | Error::TagRecords { .. } | Error::BlobRange { .. })
        ),
        "a record count one too high must refuse"
    );

    let mut short = blob;
    short[4..8].copy_from_slice(&(count - 1).to_le_bytes());
    assert!(
        matches!(decode_tags(&short), Err(Error::TagRecords { .. })),
        "a record count one too low must refuse, because the records stop short of the text"
    );
}

/// Each build named, with its archive open.
fn builds() -> Option<Vec<(String, Archive)>> {
    let out: Vec<(String, Archive)> = [SERVER_VAR, CLIENT_VAR]
        .into_iter()
        .filter_map(|name| archive(name).map(|archive| (name.to_string(), archive)))
        .collect();
    if out.is_empty() {
        eprintln!("skipped: set {SERVER_VAR} or {CLIENT_VAR} to run this");
        return None;
    }
    Some(out)
}

/// The archive beside the executable a variable names.
fn archive(name: &str) -> Option<Archive> {
    let exe = var(name)?;
    assert!(
        exe.is_file(),
        "{name} names {}, which is not a file",
        exe.display()
    );
    Some(
        Archive::beside(&exe).unwrap_or_else(|err| panic!("{name} names {}: {err}", exe.display())),
    )
}

/// The payload file beside the executable a variable names.
fn payload_of(name: &str) -> PathBuf {
    var(name)
        .expect("the variable was read to open the archive")
        .with_extension("kfc_resources")
}

/// The localization resource's payload.
fn collection_bytes(client: &mut Archive) -> Vec<u8> {
    let resource = *client
        .directory()
        .by_type(TagCollection::TYPE_HASH)
        .next()
        .expect("the client ships a localization table");
    client
        .read_resource(resource.id)
        .expect("the localization resource reads")
}

/// The authoring language's blob and its decoded tags.
fn authoring_table(client: &mut Archive) -> (Vec<u8>, Vec<LocaTag>) {
    let bytes = collection_bytes(client);
    let collection = TagCollection::parse(&bytes).expect("the localization resource parses");
    let blob = client
        .read_content(collection.keenglish)
        .expect("the authoring table reads");
    let tags = decode_tags(&blob).expect("the authoring table decodes");
    (blob, tags)
}

/// Where the first byte of text sits, which is where the record array stops.
fn first_text(tags: &[LocaTag]) -> usize {
    tags.iter()
        .filter(|tag| !tag.text.is_empty())
        .map(|tag| tag.text_at)
        .min()
        .expect("a table carries text")
}

/// Read an environment variable as a path, treating empty as unset.
fn var(name: &str) -> Option<PathBuf> {
    let value = std::env::var_os(name)?;
    (!value.is_empty()).then(|| PathBuf::from(value))
}
