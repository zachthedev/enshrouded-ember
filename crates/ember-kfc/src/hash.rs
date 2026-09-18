//! The two identities a container carries: a resource id and a content hash.

use std::fmt;

/// The FNV-1a 32 offset basis.
const OFFSET_BASIS: u32 = 0x811c_9dc5;

/// The FNV-1a 32 prime.
const PRIME: u32 = 0x0100_0193;

/// FNV-1a over 32 bits.
///
/// ```
/// # use ember_kfc::fnv1a32;
/// assert_eq!(fnv1a32(b""), 0x811c_9dc5);
/// assert_eq!(fnv1a32(b"a"), 0xe40c_292c);
/// ```
#[must_use]
pub const fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash = OFFSET_BASIS;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u32;
        hash = hash.wrapping_mul(PRIME);
        index += 1;
    }
    hash
}

/// The type hash a [`ResourceId`] carries, from a fully qualified type name.
///
/// The name is spelled the way the engine spells it, with `::` separators and
/// no abbreviation, because the hash is taken over those exact bytes.
///
/// ```
/// # use ember_kfc::type_hash;
/// assert_eq!(type_hash("keen::LocaTagCollectionResource"), 0x3f13_ff35);
/// ```
#[must_use]
pub const fn type_hash(qualified_name: &str) -> u32 {
    fnv1a32(qualified_name.as_bytes())
}

/// The key of the resource map.
///
/// On disk this is 32 bytes: the GUID, the type hash, the part index, then two
/// reserved words a reader ignores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId {
    /// The GUID identifying the resource, in file order.
    pub guid: [u8; 16],
    /// FNV-1a 32 of the fully qualified type name.
    pub type_hash: u32,
    /// Which part of one logical resource this is.
    pub part_index: u32,
}

impl ResourceId {
    /// Bytes one record occupies in the key array.
    pub(crate) const STRIDE: usize = 32;
}

/// The name the game gives a resource: the GUID in file order, grouped 4-2-2-2-6,
/// then the type hash and the part index.
///
/// The grouping is textual. Nothing here byte-swaps the first three groups the
/// way a Microsoft GUID rendering does, because the container stores the bytes
/// in the order this prints them.
impl fmt::Display for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.guid.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        write!(f, "_{:08x}_{}", self.type_hash, self.part_index)
    }
}

/// The key of the content map, which also states the blob's length.
///
/// The length lives in the hash because nothing else in the directory records
/// it. Reading a blob means seeking to the entry's offset and taking exactly
/// [`size`](Self::size) bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash {
    /// The blob's length in bytes.
    pub size: u32,
    /// The three hash words that follow the length.
    pub words: [u32; 3],
}

impl ContentHash {
    /// Bytes one record occupies in the key array.
    pub(crate) const STRIDE: usize = 16;

    /// Build a hash from its four words, length first.
    #[must_use]
    pub const fn new(size: u32, words: [u32; 3]) -> Self {
        Self { size, words }
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:08x}", self.size)?;
        for word in self.words {
            write!(f, "{word:08x}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ContentHash, ResourceId, fnv1a32, type_hash};

    /// The published FNV-1a 32 test vectors, which fix the basis, the prime and
    /// the order of the xor and the multiply.
    #[test]
    fn the_hash_matches_the_published_vectors() {
        let cases: &[(&[u8], u32)] = &[
            (b"", 0x811c_9dc5),
            (b"a", 0xe40c_292c),
            (b"foobar", 0xbf9c_f968),
        ];
        for (input, expected) in cases {
            assert_eq!(
                fnv1a32(input),
                *expected,
                "fnv1a32({:?})",
                String::from_utf8_lossy(input)
            );
        }
    }

    /// The hash runs over the name's bytes and nothing else: no separator
    /// rewriting, no case folding, no terminator.
    #[test]
    fn a_type_hash_is_the_hash_of_the_name_as_written() {
        assert_eq!(
            type_hash("keen::LocaTagCollectionResource"),
            fnv1a32(b"keen::LocaTagCollectionResource")
        );
        assert_ne!(
            type_hash("keen::LocaTagCollectionResource"),
            type_hash("keen.LocaTagCollectionResource")
        );
        assert_ne!(
            type_hash("keen::LocaTagCollectionResource"),
            fnv1a32(b"keen::LocaTagCollectionResource\0")
        );
    }

    #[test]
    fn a_resource_id_prints_the_guid_in_file_order() {
        let id = ResourceId {
            guid: [
                0x50, 0x9f, 0xea, 0xdb, 0x4c, 0x60, 0x42, 0x5f, 0x9c, 0x7c, 0xde, 0xee, 0xfd, 0x9b,
                0x69, 0x20,
            ],
            type_hash: 0x21b2_a090,
            part_index: 3,
        };
        assert_eq!(
            id.to_string(),
            "509feadb-4c60-425f-9c7c-deeefd9b6920_21b2a090_3"
        );
    }

    #[test]
    fn a_content_hash_prints_its_four_words_with_the_length_first() {
        let hash = ContentHash::new(0x0000_1234, [0xdead_beef, 0x0000_0001, 0xffff_ffff]);
        assert_eq!(hash.to_string(), "00001234deadbeef00000001ffffffff");
    }
}
