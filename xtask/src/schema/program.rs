//! The program records Keen's engine keeps in `.rdata`.
//!
//! A program is a unit of engine work with an entry function and the component
//! sets it touches. Each record holds its name at `+0x00` as a
//! `{pointer, length}` pair, its entry function at `+0x10`, and seven
//! `{pointer, count}` slices of name slices at the offsets [`SLOTS`] lists.
//!
//! The entry pointer is what separates a server program from a client one. A
//! record whose entry is NULL in this image belongs to the other build.
//!
//! **The record set is heuristic and carries false positives.** Nothing in the
//! image marks where the program table starts or ends, so the walk recognizes
//! records by shape, and a string that happens to sit beside a readable slice
//! is kept. `else`, `material`, `loop`, `enable` and `hierarchy` are in the
//! output and are not programs. Read the dump as candidates to check rather
//! than as an authoritative list, and confirm any single record against the
//! bytes before building on it.

use crate::image::Image;
use crate::schema::descriptor::name_slice;

/// Offsets of the seven component slices, in the order the dump prints them.
pub const SLOTS: [usize; 7] = [0x18, 0x38, 0x58, 0x98, 0xB0, 0xE0, 0xF0];

/// Column names for the seven slices, carrying their offsets so a reader of the
/// dump can go back to the bytes.
pub const SLOT_COLUMNS: [&str; 7] = [
    "trigger(+18)",
    "iter_read(+38)",
    "iter_write(+58)",
    "globals_ro(+98)",
    "globals_rw(+b0)",
    "rand_read(+e0)",
    "rand_write(+f0)",
];

/// Bytes one record occupies.
const RECORD_SPAN: usize = 0x100;

/// Bytes in one element of a component slice.
const ELEMENT_STRIDE: usize = 16;

/// Most components one slice is believed to name.
///
/// The bound is what separates a real slice from a pointer that happens to sit
/// beside a large integer, and it is load bearing: raising it admits records
/// that are not programs.
const MAX_ELEMENTS: u64 = 64;

/// Shortest program name.
const MIN_NAME: usize = 4;

/// One decoded program record.
#[derive(Debug, Clone)]
pub struct Program {
    /// File offset of the record.
    pub offset: usize,
    /// The program name.
    pub name: String,
    /// The entry function, or zero when this build does not carry it.
    pub entry: u64,
    /// The seven component slices, in [`SLOTS`] order.
    pub slots: [Vec<String>; 7],
}

/// Whether a name is spelled the way every program name is.
fn is_program_name(name: &str) -> bool {
    name.len() >= MIN_NAME
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Read one component slice, stopping at the first element that fails.
fn slice(image: &Image, offset: usize) -> Vec<String> {
    let mut out = Vec::new();
    let Some(rdata) = image.section(".rdata") else {
        return out;
    };
    let (Some(pointer), Some(count)) = (image.qword_at(offset), image.qword_at(offset + 8)) else {
        return out;
    };
    if pointer == 0 || count == 0 || count > MAX_ELEMENTS {
        return out;
    }
    if !(rdata.start..rdata.raw_end()).contains(&pointer) {
        return out;
    }
    let Some(array) = image.offset_of(pointer) else {
        return out;
    };
    let count = usize::try_from(count).unwrap_or(0);
    for index in 0..count {
        let Some(name) = name_slice(image, array + index * ELEMENT_STRIDE) else {
            break;
        };
        out.push(name);
    }
    out
}

/// Every program record the image carries, in ascending file order.
pub fn walk(image: &Image) -> Vec<Program> {
    let (Some(rdata), Some(text)) = (image.section(".rdata"), image.section(".text")) else {
        return Vec::new();
    };
    let executable = text.start..text.raw_end();
    let mut out = Vec::new();
    let last = rdata.raw_offset + rdata.raw_size;
    for offset in (rdata.raw_offset..).step_by(8) {
        if offset + RECORD_SPAN >= last {
            break;
        }
        let Some(name) = name_slice(image, offset) else {
            continue;
        };
        if !is_program_name(&name) {
            continue;
        }
        let entry = image.qword_at(offset + 0x10).unwrap_or(0);
        if entry != 0 && !executable.contains(&entry) {
            continue;
        }
        let slots = SLOTS.map(|slot| slice(image, offset + slot));
        if slots.iter().all(Vec::is_empty) {
            continue;
        }
        out.push(Program {
            offset,
            name,
            entry,
            slots,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::is_program_name;

    #[test]
    fn program_names_are_lowercase_snake_case_of_at_least_four_bytes() {
        let cases: &[(&str, bool)] = &[
            ("player_crafting", true),
            ("else", true),
            ("hierarchy", true),
            ("weaponskill", true),
            ("float3", true),
            ("ui2", false),
            ("ignore", true),
            ("PlayerSpawnPoint", false),
            ("Displacement Scale", false),
            ("10.0f", false),
            ("keen::bool", false),
            ("", false),
        ];
        for (name, want) in cases {
            assert_eq!(is_program_name(name), *want, "name {name:?}");
        }
    }
}
