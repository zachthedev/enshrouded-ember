//! The localization table, which is the one resource layout this crate decodes.
//!
//! Every other resource needs the reflection registry to know its field layout,
//! and the registry lives in the executable rather than in either data file.
//! These two structures are small enough to check themselves from bytes alone:
//! the language array closes on the resource's declared size, and the tag
//! records end exactly where the text they address begins.
//!
//! The table is two levels. A `keen::LocaTagCollectionResource` lives in the
//! payload and holds nothing but hashes: one for the authoring language and one
//! per shipped language. Each hash addresses a content blob in a `.dat`
//! container, and that blob holds the records.
//!
//! The dedicated server ships no such resource. A mod naming a tag id is naming
//! something only the client can resolve.
//!
//! # What a caller may keep
//!
//! The decoded table is Keen's own written text rather than functional interface
//! data, so it is never committed. A mod that displays a line resolves the ids it
//! needs at build time, out of a client the developer fetched, and keeps the ids
//! alone. This crate is how that build step reads the table, and everything it
//! produces belongs under `.cache` like every other extraction.

use crate::blob::{BlobArray, BlobString, read_u32};
use crate::error::{Error, Result};
use crate::hash::{ContentHash, type_hash};

/// Bytes one tag record occupies.
///
/// An id, a string header, an argument array header, and a generic count. A
/// stride one field out still decodes into readable text, which is why
/// [`decode_tags`] checks the records against the bytes they address rather
/// than trusting the stride.
pub const TAG_STRIDE: usize = 24;

/// Bytes one language record occupies: a language id, then a content hash.
pub const LANGUAGE_STRIDE: usize = 20;

/// The id of the authoring language, which carries every tag.
///
/// Translated tables carry a subset, so a tag present in one and absent from the
/// other is ordinary rather than a decode failure.
pub const KEENGLISH: u32 = 0;

/// The language id English ships under.
pub const ENGLISH: u32 = 1;

/// The resource that names every language's tag table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagCollection {
    /// The authoring language's table.
    pub keenglish: ContentHash,
    /// Each shipped language's id and table.
    pub languages: Vec<(u32, ContentHash)>,
}

impl TagCollection {
    /// The fully qualified type name the directory hashes into a type id.
    pub const TYPE_NAME: &'static str = "keen::LocaTagCollectionResource";

    /// The type hash a resource of this type carries.
    pub const TYPE_HASH: u32 = type_hash(Self::TYPE_NAME);

    /// Decode a `keen::LocaTagCollectionResource` payload.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Truncated`] when a field does not fit and
    /// [`Error::BlobRange`] when the language array runs past the resource.
    pub fn parse(resource: &[u8]) -> Result<Self> {
        const FIELD: &str = "keenglishDataHash";
        let keenglish = read_content_hash(resource, 0, FIELD)?;
        let array = BlobArray::read(resource, 0x10, "languages")?;
        array.end(LANGUAGE_STRIDE, resource.len(), "languages")?;
        let languages = (0..array.count)
            .map(|index| {
                let at = array.at + index * LANGUAGE_STRIDE;
                Ok((
                    read_u32(resource, at, "languages")?,
                    read_content_hash(resource, at + 4, "languages")?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            keenglish,
            languages,
        })
    }

    /// The table for one language id.
    #[must_use]
    pub fn language(&self, id: u32) -> Option<ContentHash> {
        if id == KEENGLISH {
            return Some(self.keenglish);
        }
        self.languages
            .iter()
            .find(|(language, _)| *language == id)
            .map(|(_, hash)| *hash)
    }
}

/// One localization tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocaTag {
    /// The id the client resolves text by.
    pub id: u32,
    /// The text, which is UTF-8 and carries no terminator.
    pub text: String,
    /// Where the text sits in the blob this tag came from.
    ///
    /// The records and the text are laid end to end, so the lowest of these
    /// across a table is where the record array stops.
    pub text_at: usize,
    /// How many positional arguments the text takes.
    pub arguments: u32,
    /// How many generic arguments the text takes.
    pub generic_arguments: u32,
}

/// Decode a `keen::LocaTagCollectionResourceData` blob.
///
/// The blob opens with a `blob_array` header, then the records, then the text
/// each record points at. Two structural facts close the decode: the records end
/// exactly where the first string begins, and the ids ascend. A stride one field
/// out breaks both, which is what stops a wrong stride producing readable
/// nonsense.
///
/// # Errors
///
/// Returns [`Error::Truncated`] or [`Error::BlobRange`] when a record or a
/// string does not fit, [`Error::NotUtf8`] when text is not UTF-8,
/// [`Error::TagRecords`] when the records do not end where the text begins, and
/// [`Error::TagOrder`] when the ids do not ascend.
pub fn decode_tags(blob: &[u8]) -> Result<Vec<LocaTag>> {
    const FIELD: &str = "tags";
    let records = BlobArray::read(blob, 0, FIELD)?;
    let records_end = records.end(TAG_STRIDE, blob.len(), FIELD)?;

    let mut out: Vec<LocaTag> = Vec::with_capacity(records.count);
    let mut first_text: Option<usize> = None;
    let mut previous: Option<u32> = None;
    for index in 0..records.count {
        let at = records.at + index * TAG_STRIDE;
        let id = read_u32(blob, at, FIELD)?;
        if let Some(last) = previous
            && id <= last
        {
            return Err(Error::TagOrder {
                index,
                id,
                previous: last,
            });
        }
        previous = Some(id);

        let text = BlobString::read(blob, at + 4, "text")?;
        if text.len > 0 {
            first_text = Some(first_text.map_or(text.at, |seen: usize| seen.min(text.at)));
        }
        // The argument array's element layout is not established, so the count
        // is read and the offset is left unresolved.
        let arguments = read_u32(blob, at + 0x10, "arguments")?;
        let generic_arguments = read_u32(blob, at + 0x14, "genericArguments")?;
        out.push(LocaTag {
            id,
            text: text.text(blob, "text")?.to_owned(),
            text_at: text.at,
            arguments,
            generic_arguments,
        });
    }

    if let Some(first) = first_text
        && first != records_end
    {
        return Err(Error::TagRecords {
            count: records.count,
            stride: TAG_STRIDE,
            end: records_end,
            first,
        });
    }
    Ok(out)
}

/// Read a 16-byte content hash, length word first.
fn read_content_hash(bytes: &[u8], at: usize, field: &'static str) -> Result<ContentHash> {
    Ok(ContentHash::new(
        read_u32(bytes, at, field)?,
        [
            read_u32(bytes, at + 4, field)?,
            read_u32(bytes, at + 8, field)?,
            read_u32(bytes, at + 12, field)?,
        ],
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        ENGLISH, KEENGLISH, LANGUAGE_STRIDE, LocaTag, TAG_STRIDE, TagCollection, decode_tags,
    };
    use crate::blob::HEADER;
    use crate::error::Error;
    use crate::hash::{ContentHash, type_hash};

    /// A tag as it goes into a synthetic blob.
    struct Row {
        id: u32,
        text: &'static str,
        arguments: u32,
        generic_arguments: u32,
    }

    /// Lay a tag blob out the way the container does: an eight-byte header, the
    /// records, then the text each record points at.
    ///
    /// `stride` is a parameter so a case can lay the records one field short and
    /// watch the decoder refuse the result.
    fn tag_blob(rows: &[Row], stride: usize) -> Vec<u8> {
        let records_at = HEADER;
        let text_at = records_at + rows.len() * stride;
        let mut bytes = vec![0u8; text_at];
        let relative = u32::try_from(records_at).expect("the header is small");
        bytes[0..4].copy_from_slice(&relative.to_le_bytes());
        bytes[4..8].copy_from_slice(&u32::try_from(rows.len()).expect("few rows").to_le_bytes());

        for (index, row) in rows.iter().enumerate() {
            let at = records_at + index * stride;
            let text_offset = bytes.len();
            bytes.extend_from_slice(row.text.as_bytes());
            let relative = u32::try_from(text_offset - (at + 4)).expect("the text is close by");
            bytes[at..at + 4].copy_from_slice(&row.id.to_le_bytes());
            bytes[at + 4..at + 8].copy_from_slice(&relative.to_le_bytes());
            bytes[at + 8..at + 12].copy_from_slice(
                &u32::try_from(row.text.len())
                    .expect("the text is short")
                    .to_le_bytes(),
            );
            if stride >= TAG_STRIDE {
                bytes[at + 0x10..at + 0x14].copy_from_slice(&row.arguments.to_le_bytes());
                bytes[at + 0x14..at + 0x18].copy_from_slice(&row.generic_arguments.to_le_bytes());
            }
        }
        bytes
    }

    fn rows() -> Vec<Row> {
        vec![
            Row {
                id: 0x0001_0000,
                text: "Chest",
                arguments: 0,
                generic_arguments: 0,
            },
            Row {
                id: 0x0002_0000,
                text: "Open {0}",
                arguments: 1,
                generic_arguments: 0,
            },
            Row {
                id: 0x0003_0000,
                text: "",
                arguments: 0,
                generic_arguments: 2,
            },
        ]
    }

    #[test]
    fn a_well_formed_blob_decodes() {
        let blob = tag_blob(&rows(), TAG_STRIDE);
        let tags = decode_tags(&blob).expect("the blob decodes");
        let records_end = HEADER + rows().len() * TAG_STRIDE;
        assert_eq!(
            tags,
            vec![
                LocaTag {
                    id: 0x0001_0000,
                    text: "Chest".to_string(),
                    text_at: records_end,
                    arguments: 0,
                    generic_arguments: 0,
                },
                LocaTag {
                    id: 0x0002_0000,
                    text: "Open {0}".to_string(),
                    text_at: records_end + 5,
                    arguments: 1,
                    generic_arguments: 0,
                },
                LocaTag {
                    id: 0x0003_0000,
                    text: String::new(),
                    text_at: records_end + 13,
                    arguments: 0,
                    generic_arguments: 2,
                },
            ]
        );
    }

    /// The records and the text are laid end to end, so a stride that is one
    /// field short leaves the array overlapping the text it addresses. This is
    /// the failure that would otherwise produce readable nonsense.
    #[test]
    fn records_laid_at_the_wrong_stride_are_refused() {
        let blob = tag_blob(&rows(), 20);
        assert!(
            matches!(
                decode_tags(&blob),
                Err(Error::TagOrder { .. } | Error::TagRecords { .. } | Error::BlobRange { .. })
            ),
            "a 20-byte stride must refuse rather than decode"
        );
    }

    #[test]
    fn ids_that_do_not_ascend_are_refused() {
        let mut blob = tag_blob(&rows(), TAG_STRIDE);
        let second = HEADER + TAG_STRIDE;
        blob[second..second + 4].copy_from_slice(&1u32.to_le_bytes());
        assert!(matches!(
            decode_tags(&blob),
            Err(Error::TagOrder {
                index: 1,
                id: 1,
                previous: 0x0001_0000
            })
        ));
    }

    #[test]
    fn a_repeated_id_is_refused() {
        let mut blob = tag_blob(&rows(), TAG_STRIDE);
        let second = HEADER + TAG_STRIDE;
        blob[second..second + 4].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        assert!(matches!(decode_tags(&blob), Err(Error::TagOrder { .. })));
    }

    /// A gap between the records and the text means the record count and the
    /// stride do not agree with the layout, whichever of the two moved.
    #[test]
    fn text_that_does_not_begin_at_the_end_of_the_records_is_refused() {
        let mut blob = tag_blob(&rows(), TAG_STRIDE);
        // Push the first string four bytes further out and widen the blob to
        // match, so the span still fits and only the seam moves.
        let first = HEADER;
        let relative = u32::from_le_bytes([
            blob[first + 4],
            blob[first + 5],
            blob[first + 6],
            blob[first + 7],
        ]);
        blob[first + 4..first + 8].copy_from_slice(&(relative + 4).to_le_bytes());
        blob.extend_from_slice(&[0u8; 4]);
        assert!(matches!(
            decode_tags(&blob),
            Err(Error::TagRecords {
                stride: TAG_STRIDE,
                ..
            })
        ));
    }

    #[test]
    fn a_record_array_running_past_the_blob_is_refused() {
        let mut blob = tag_blob(&rows(), TAG_STRIDE);
        blob[4..8].copy_from_slice(&1000u32.to_le_bytes());
        assert!(matches!(decode_tags(&blob), Err(Error::BlobRange { .. })));
    }

    /// The language array closes on the resource's declared size: the header
    /// sits at 0x10, the entries start at 0x18, and the resource ends where the
    /// last entry does.
    #[test]
    fn the_language_array_closes_on_the_resource_size() {
        let count = 15usize;
        let total = 0x18 + count * LANGUAGE_STRIDE;
        let mut resource = vec![0u8; total];
        resource[0..4].copy_from_slice(&0x1234u32.to_le_bytes());
        resource[4..8].copy_from_slice(&1u32.to_le_bytes());
        resource[8..12].copy_from_slice(&2u32.to_le_bytes());
        resource[12..16].copy_from_slice(&3u32.to_le_bytes());
        resource[0x10..0x14].copy_from_slice(&8u32.to_le_bytes());
        resource[0x14..0x18].copy_from_slice(
            &u32::try_from(count)
                .expect("fifteen languages")
                .to_le_bytes(),
        );
        for index in 0..count {
            let at = 0x18 + index * LANGUAGE_STRIDE;
            let id = u32::try_from(index).expect("few languages") + 1;
            resource[at..at + 4].copy_from_slice(&id.to_le_bytes());
            resource[at + 4..at + 8].copy_from_slice(&(0x100 + id).to_le_bytes());
        }
        assert_eq!(total, 324, "fifteen 20-byte entries from 0x18 close at 324");

        let collection = TagCollection::parse(&resource).expect("the resource parses");
        assert_eq!(
            collection.keenglish,
            ContentHash::new(0x1234, [1, 2, 3]),
            "the authoring hash is the first field"
        );
        assert_eq!(collection.languages.len(), count);
        assert_eq!(
            collection.language(ENGLISH),
            Some(ContentHash::new(0x101, [0, 0, 0]))
        );
        assert_eq!(collection.language(KEENGLISH), Some(collection.keenglish));
        assert_eq!(collection.language(999), None);
    }

    #[test]
    fn a_language_array_running_past_the_resource_is_refused() {
        let mut resource = vec![0u8; 324];
        resource[0x10..0x14].copy_from_slice(&8u32.to_le_bytes());
        resource[0x14..0x18].copy_from_slice(&64u32.to_le_bytes());
        assert!(matches!(
            TagCollection::parse(&resource),
            Err(Error::BlobRange { .. })
        ));
    }

    #[test]
    fn the_collection_type_hash_is_the_hash_of_its_name() {
        assert_eq!(
            TagCollection::TYPE_HASH,
            type_hash(TagCollection::TYPE_NAME)
        );
    }
}
