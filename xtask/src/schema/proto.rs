//! The network message registry the server keeps in `.data`.
//!
//! This table is separate from the reflection schema. It is a hand-written
//! serialization registry for the messages a client and a server exchange, so
//! its records are 40 bytes and its field records are 64, and none of it
//! carries a `keen::` name.
//!
//! A record holds a name pointer, the wire size of the message, a pointer to
//! the field array, and the field count. A field record holds a name pointer, a
//! type code, a byte offset and a capacity.
//!
//! Several registries sit in `.data`: one for login and authentication, one for
//! base sharing, and the game session registry. [`ANCHOR`] names the game one,
//! because that is the layer a mod intercepts.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use crate::image::Image;

/// The message whose registry block is the game session protocol.
pub const ANCHOR: &str = "ServerInfoMessage";

/// Bytes in one registry record.
const RECORD_STRIDE: usize = 0x28;

/// Bytes in one field record.
const FIELD_STRIDE: usize = 0x40;

/// Longest name a registry record carries.
const NAME_LIMIT: usize = 64;

/// Shortest name a registry record carries.
const MIN_NAME: usize = 3;

/// Most fields one message is believed to carry.
const MAX_FIELDS: u64 = 64;

/// The wire types a field record's code selects.
///
/// A code of `9` and a code of `0xa` are both eight bytes wide and only `0xa`
/// is confirmed unsigned, so `9` prints with a question mark rather than
/// claiming a signedness nothing established.
static TYPES: LazyLock<BTreeMap<u32, &'static str>> = LazyLock::new(|| {
    BTreeMap::from([
        (0x0, "bool"),
        (0x4, "string/bytes"),
        (0x8, "uint32"),
        (0x9, "uint64?"),
        (0xA, "uint64"),
        (0xC, "message"),
    ])
});

/// One field of a message.
#[derive(Debug, Clone)]
pub struct Field {
    /// The field name.
    pub name: String,
    /// The wire type, as [`TYPES`] names it.
    pub wire_type: &'static str,
    /// Byte offset of the field inside the message.
    pub offset: u64,
    /// Elements the field holds, which is one for a scalar.
    pub capacity: u64,
}

/// One message in the registry.
#[derive(Debug, Clone)]
pub struct Message {
    /// The message name.
    pub name: String,
    /// The wire size of the message in bytes.
    pub size: u64,
    /// The field count the record states, whether or not the fields decode.
    pub declared_fields: u64,
    /// The fields that decoded.
    pub fields: Vec<Field>,
}

/// Read a registry name at a virtual address.
fn identifier(image: &Image, va: u64) -> Option<String> {
    let text = image.cstr(va, NAME_LIMIT)?;
    if text.len() < MIN_NAME {
        return None;
    }
    let mut bytes = text.bytes();
    let first = bytes.next()?;
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return None;
    }
    if !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        return None;
    }
    Some(text)
}

/// The name a registry record at this file offset carries.
fn record_name(image: &Image, offset: usize) -> Option<String> {
    identifier(image, image.qword_at(offset)?)
}

/// Read the game session registry block.
///
/// Returns an empty list when the anchor message is absent, which is how a
/// build that moved the registry reports itself rather than guessing.
pub fn walk(image: &Image) -> Vec<Message> {
    let Some(data) = image.section(".data") else {
        return Vec::new();
    };
    let first = data.raw_offset;
    let last = data.raw_offset + data.raw_size;
    let mut anchor = None;
    for offset in (first..).step_by(8) {
        if offset + RECORD_STRIDE > last {
            break;
        }
        if record_name(image, offset).as_deref() == Some(ANCHOR) {
            anchor = Some(offset);
            break;
        }
    }
    let Some(anchor) = anchor else {
        return Vec::new();
    };

    let mut start = anchor;
    while start >= first + RECORD_STRIDE && record_name(image, start - RECORD_STRIDE).is_some() {
        start -= RECORD_STRIDE;
    }
    let mut end = anchor;
    while end + 2 * RECORD_STRIDE <= last && record_name(image, end + RECORD_STRIDE).is_some() {
        end += RECORD_STRIDE;
    }

    let mut out = Vec::new();
    for record in (start..=end).step_by(RECORD_STRIDE) {
        let Some(name) = record_name(image, record) else {
            continue;
        };
        let size = image.qword_at(record + 0x08).unwrap_or(0);
        let array = image.qword_at(record + 0x10).unwrap_or(0);
        let declared_fields = image.qword_at(record + 0x18).unwrap_or(0);
        let mut fields = Vec::new();
        if declared_fields <= MAX_FIELDS
            && let Some(base) = image.offset_of(array)
        {
            for index in 0..usize::try_from(declared_fields).unwrap_or(0) {
                let entry = base + index * FIELD_STRIDE;
                if entry + FIELD_STRIDE > image.bytes().len() {
                    break;
                }
                let (Some(name_va), Some(code_word)) =
                    (image.qword_at(entry), image.qword_at(entry + 0x08))
                else {
                    break;
                };
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "the type code is the low half of the word"
                )]
                let code = code_word as u32;
                let (Some(field_name), Some(wire_type)) =
                    (identifier(image, name_va), TYPES.get(&code))
                else {
                    break;
                };
                fields.push(Field {
                    name: field_name,
                    wire_type,
                    offset: image.qword_at(entry + 0x18).unwrap_or(0),
                    capacity: image.qword_at(entry + 0x28).unwrap_or(0),
                });
            }
        }
        out.push(Message {
            name,
            size,
            declared_fields,
            fields,
        });
    }
    out
}
