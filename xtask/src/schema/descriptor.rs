//! The reflection type descriptors Keen's engine keeps in `.rdata`.
//!
//! A descriptor is found through its name, not through a registry. The bare
//! name sits at `+0x40` from the descriptor base as a `{pointer, length}` pair,
//! so a scan of `.rdata` for a pair whose target is a string of exactly that
//! length lands on every descriptor in the image.
//!
//! From the base: `+0x40` the bare name, `+0x50` the dotted `ecs.X` name,
//! `+0x60` the qualified `keen::...` name, `+0x80` size then alignment, `+0x88`
//! field count then flags, `+0x90` a 64-bit hash whose low half is FNV-1a 32 of
//! the qualified name, `+0x98` the field array, `+0xa0` the enumerator array. A
//! field record is 48 bytes and an enumerator record is 40.

use crate::image::Image;

/// Flag bit that makes a descriptor an enumeration rather than a structure.
pub const FLAG_ENUM: u32 = 0x8;

/// Offset from the descriptor base to the bare name slice.
const NAME_SLICE: usize = 0x40;

/// Longest name any slice in these tables carries.
const NAME_LIMIT: usize = 250;

/// Bytes a descriptor occupies, up to and including the enumerator array.
const DESCRIPTOR_SPAN: usize = 0xA8;

/// Bytes in one field record.
const FIELD_STRIDE: usize = 48;

/// Bytes in one enumerator record.
const ENUM_STRIDE: usize = 40;

/// Most fields or enumerators one descriptor is believed to carry.
const MAX_MEMBERS: u64 = 1024;

/// One decoded type descriptor.
#[derive(Debug, Clone)]
pub struct Descriptor {
    /// Virtual address of the descriptor base.
    pub base: u64,
    /// File offset of the descriptor base.
    pub offset: usize,
    /// The bare name, as in `PlayerSpawnPoint`.
    pub short: String,
    /// The dotted name, as in `ecs.PlayerSpawnPoint`, when the type has one.
    pub dotted: Option<String>,
    /// The qualified name, as in `keen::ecs::PlayerSpawnPoint`.
    pub full: String,
    /// The raw word holding size and alignment.
    pub size_word: u64,
    /// The raw word holding the field count and the flags.
    pub count_word: u64,
    /// Size of the type in bytes.
    pub size: u32,
    /// Alignment of the type in bytes.
    pub align: u16,
    /// Number of fields, or of enumerators when the type is an enumeration.
    pub members: u32,
    /// Reflection flags.
    pub flags: u32,
    /// The 64-bit hash.
    pub hash: u64,
    /// Virtual address of the field array.
    pub fields: u64,
    /// Virtual address of the enumerator array.
    pub enumerators: u64,
}

impl Descriptor {
    /// Whether this descriptor describes an enumeration.
    pub fn is_enum(&self) -> bool {
        self.flags & FLAG_ENUM != 0
    }
}

/// One field of a structure.
#[derive(Debug, Clone)]
pub struct Field {
    /// Byte offset of the field inside its type.
    pub offset: u64,
    /// The field name.
    pub name: String,
    /// The qualified name of the field's type, when its descriptor resolves.
    pub type_name: Option<String>,
    /// The bare name of the field's type, when its descriptor resolves.
    pub type_short: Option<String>,
    /// The size of the field's type, when its descriptor resolves.
    pub type_size: Option<u32>,
}

/// One enumerator of an enumeration.
#[derive(Debug, Clone)]
pub struct Enumerator {
    /// The value the enumerator stands for.
    pub value: u64,
    /// The enumerator name.
    pub name: String,
}

/// Read a `{pointer, length}` name slice at a file offset.
///
/// The length has to match the string exactly. That is what separates a real
/// name slice from a pointer that happens to sit next to a small integer.
pub fn name_slice(image: &Image, offset: usize) -> Option<String> {
    let rdata = image.section(".rdata")?;
    let pointer = image.qword_at(offset)?;
    let length = image.qword_at(offset + 8)?;
    if !(rdata.start..rdata.raw_end()).contains(&pointer) {
        return None;
    }
    if length == 0 || length >= NAME_LIMIT as u64 {
        return None;
    }
    let length = usize::try_from(length).ok()?;
    let start = image.offset_of(pointer)?;
    let bytes = image.bytes().get(start..start + length + 1)?;
    if bytes[length] != 0 || bytes[..length].contains(&0) {
        return None;
    }
    Some(bytes[..length].iter().map(|byte| *byte as char).collect())
}

/// Decode the descriptor based at a virtual address.
pub fn read(image: &Image, base: u64) -> Option<Descriptor> {
    if !base.is_multiple_of(8) {
        return None;
    }
    let offset = image.offset_of(base)?;
    if offset + DESCRIPTOR_SPAN > image.bytes().len() {
        return None;
    }
    let short = name_slice(image, offset + NAME_SLICE)?;
    let full = name_slice(image, offset + 0x60)?;
    let size_word = image.qword_at(offset + 0x80)?;
    let count_word = image.qword_at(offset + 0x88)?;
    Some(Descriptor {
        base,
        offset,
        short,
        dotted: image
            .qword_at(offset + 0x50)
            .and_then(|va| image.cstr(va, NAME_LIMIT)),
        full,
        size_word,
        count_word,
        #[expect(clippy::cast_possible_truncation, reason = "the low word is the size")]
        size: size_word as u32,
        #[expect(clippy::cast_possible_truncation, reason = "alignment is 16 bits wide")]
        align: (size_word >> 32) as u16,
        #[expect(clippy::cast_possible_truncation, reason = "the low word is the count")]
        members: count_word as u32,
        flags: (count_word >> 32) as u32,
        hash: image.qword_at(offset + 0x90)?,
        fields: image.qword_at(offset + 0x98)?,
        enumerators: image.qword_at(offset + 0xA0)?,
    })
}

/// Read a descriptor's fields.
///
/// The walk stops at the first record that does not decode. A field array is
/// reached from a heuristic scan, so a descriptor whose count is wrong yields
/// the fields it really has rather than nothing.
pub fn fields(image: &Image, descriptor: &Descriptor) -> Vec<Field> {
    let mut out = Vec::new();
    if descriptor.members == 0 || u64::from(descriptor.members) > MAX_MEMBERS {
        return out;
    }
    let Some(array) = image.offset_of(descriptor.fields) else {
        return out;
    };
    for index in 0..descriptor.members as usize {
        let record = array + index * FIELD_STRIDE;
        if record + FIELD_STRIDE > image.bytes().len() {
            break;
        }
        let Some(name) = name_slice(image, record) else {
            break;
        };
        let type_pointer = image.qword_at(record + 16).unwrap_or(0);
        let Some(offset) = image.qword_at(record + 24) else {
            break;
        };
        let described = type_pointer
            .checked_sub(NAME_SLICE as u64)
            .and_then(|base| read(image, base));
        out.push(Field {
            offset,
            name,
            type_name: described.as_ref().map(|t| t.full.clone()),
            type_short: described.as_ref().map(|t| t.short.clone()),
            type_size: described.as_ref().map(|t| t.size),
        });
    }
    out
}

/// Read an enumeration's enumerators, stopping at the first bad record.
pub fn enumerators(image: &Image, descriptor: &Descriptor) -> Vec<Enumerator> {
    let mut out = Vec::new();
    if descriptor.members == 0 || u64::from(descriptor.members) > MAX_MEMBERS {
        return out;
    }
    let Some(array) = image.offset_of(descriptor.enumerators) else {
        return out;
    };
    for index in 0..descriptor.members as usize {
        let record = array + index * ENUM_STRIDE;
        if record + ENUM_STRIDE > image.bytes().len() {
            break;
        }
        let Some(name) = name_slice(image, record) else {
            break;
        };
        let Some(value) = image.qword_at(record + 16) else {
            break;
        };
        out.push(Enumerator { value, name });
    }
    out
}

/// Every descriptor the image carries, in ascending file order.
///
/// The scan steps through `.rdata` eight bytes at a time looking for a name
/// slice, then treats the address `0x40` below it as a descriptor base.
pub fn walk(image: &Image) -> Vec<Descriptor> {
    let Some(rdata) = image.section(".rdata") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let first = rdata.raw_offset;
    let last = rdata.raw_offset + rdata.raw_size;
    let mut offset = first;
    while offset + 16 <= last {
        if name_slice(image, offset).is_some()
            && let Some(va) = image.va_of(offset)
            && let Some(base) = va.checked_sub(NAME_SLICE as u64)
            && let Some(descriptor) = read(image, base)
        {
            out.push(descriptor);
        }
        offset += 8;
    }
    out
}
