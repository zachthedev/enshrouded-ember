//! The two serialization primitives a resource payload is built from.
//!
//! Both are a pair of `u32` words. The first is an offset relative to the
//! position of the offset field itself, never to the start of the blob, so a
//! record can be copied without rewriting the offsets inside it. The second is a
//! count for an array and a byte length for a string.

use crate::error::{Error, Result};

/// Bytes a `blob_array` or `blob_string` header occupies.
pub(crate) const HEADER: usize = 8;

/// Read a little-endian `u32`, or say what did not fit.
pub(crate) fn read_u32(bytes: &[u8], at: usize, table: &'static str) -> Result<u32> {
    let end = at.checked_add(4).ok_or(Error::Truncated {
        table,
        at,
        need: 4,
        have: bytes.len(),
    })?;
    let slice = bytes.get(at..end).ok_or(Error::Truncated {
        table,
        at,
        need: 4,
        have: bytes.len(),
    })?;
    let mut word = [0u8; 4];
    word.copy_from_slice(slice);
    Ok(u32::from_le_bytes(word))
}

/// Read a little-endian `u64`, or say what did not fit.
pub(crate) fn read_u64(bytes: &[u8], at: usize, table: &'static str) -> Result<u64> {
    let end = at.checked_add(8).ok_or(Error::Truncated {
        table,
        at,
        need: 8,
        have: bytes.len(),
    })?;
    let slice = bytes.get(at..end).ok_or(Error::Truncated {
        table,
        at,
        need: 8,
        have: bytes.len(),
    })?;
    let mut word = [0u8; 8];
    word.copy_from_slice(slice);
    Ok(u64::from_le_bytes(word))
}

/// Check that both words of a header sit inside the blob.
fn header_fits(blob: &[u8], at: usize, field: &'static str) -> Result<()> {
    let end = at.checked_add(HEADER).ok_or(Error::Truncated {
        table: field,
        at,
        need: HEADER,
        have: blob.len(),
    })?;
    if end > blob.len() {
        return Err(Error::Truncated {
            table: field,
            at,
            need: HEADER,
            have: blob.len(),
        });
    }
    Ok(())
}

/// A `blob_array` header, resolved against the blob holding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobArray {
    /// Where the first element sits in the blob.
    pub at: usize,
    /// How many elements follow.
    pub count: usize,
}

impl BlobArray {
    /// Read a `blob_array` header at `at`.
    ///
    /// `field` names the field in a refusal, so a caller decoding a layout of
    /// its own says which one did not fit.
    ///
    /// The element stride belongs to the layout rather than to the header, so a
    /// caller checks the span the elements occupy with [`Self::end`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Truncated`] when the header does not fit, and
    /// [`Error::BlobRange`] when the first element sits outside the blob.
    pub fn read(blob: &[u8], at: usize, field: &'static str) -> Result<Self> {
        header_fits(blob, at, field)?;
        let relative = read_u32(blob, at, field)? as usize;
        let count = read_u32(blob, at + 4, field)? as usize;
        let start = at.checked_add(relative).ok_or(Error::BlobRange {
            field,
            at,
            end: usize::MAX,
            have: blob.len(),
        })?;
        if start > blob.len() {
            return Err(Error::BlobRange {
                field,
                at: start,
                end: start,
                have: blob.len(),
            });
        }
        Ok(Self { at: start, count })
    }

    /// Where the elements end, given the stride one element occupies.
    ///
    /// # Errors
    ///
    /// Returns [`Error::BlobRange`] when the elements run past the blob.
    pub fn end(&self, stride: usize, blob_len: usize, field: &'static str) -> Result<usize> {
        let end = self
            .count
            .checked_mul(stride)
            .and_then(|span| self.at.checked_add(span))
            .ok_or(Error::BlobRange {
                field,
                at: self.at,
                end: usize::MAX,
                have: blob_len,
            })?;
        if end > blob_len {
            return Err(Error::BlobRange {
                field,
                at: self.at,
                end,
                have: blob_len,
            });
        }
        Ok(end)
    }
}

/// A `blob_string` header, resolved against the blob holding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobString {
    /// Where the bytes sit in the blob.
    pub at: usize,
    /// How many bytes they occupy.
    pub len: usize,
}

impl BlobString {
    /// Read a `blob_string` header at `at`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Truncated`] when the header does not fit, and
    /// [`Error::BlobRange`] when the bytes run past the blob.
    pub fn read(blob: &[u8], at: usize, field: &'static str) -> Result<Self> {
        header_fits(blob, at, field)?;
        let relative = read_u32(blob, at, field)? as usize;
        let len = read_u32(blob, at + 4, field)? as usize;
        let start = at.checked_add(relative).ok_or(Error::BlobRange {
            field,
            at,
            end: usize::MAX,
            have: blob.len(),
        })?;
        let end = start.checked_add(len).ok_or(Error::BlobRange {
            field,
            at: start,
            end: usize::MAX,
            have: blob.len(),
        })?;
        if end > blob.len() {
            return Err(Error::BlobRange {
                field,
                at: start,
                end,
                have: blob.len(),
            });
        }
        Ok(Self { at: start, len })
    }

    /// The text this header names.
    ///
    /// The bytes are UTF-8 and carry no terminator, so the length is the only
    /// thing that ends the string.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotUtf8`] when the bytes are not UTF-8.
    pub fn text<'blob>(&self, blob: &'blob [u8], field: &'static str) -> Result<&'blob str> {
        let bytes = blob
            .get(self.at..self.at + self.len)
            .ok_or(Error::BlobRange {
                field,
                at: self.at,
                end: self.at + self.len,
                have: blob.len(),
            })?;
        std::str::from_utf8(bytes).map_err(|_| Error::NotUtf8 { field, at: self.at })
    }
}

#[cfg(test)]
mod tests {
    use super::{BlobArray, BlobString};
    use crate::error::Error;

    /// Lay a header at `at` pointing `distance` bytes past its own position.
    fn blob(at: usize, distance: u32, second: u32, total: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; total];
        bytes[at..at + 4].copy_from_slice(&distance.to_le_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&second.to_le_bytes());
        bytes
    }

    /// An offset counts from the offset field, not from the start of the blob.
    #[test]
    fn an_array_offset_is_relative_to_its_own_position() {
        let bytes = blob(0x10, 0x20, 4, 0x100);
        let array = BlobArray::read(&bytes, 0x10, "languages").expect("the header parses");
        assert_eq!(array.at, 0x30);
        assert_eq!(array.count, 4);
    }

    #[test]
    fn a_string_offset_is_relative_to_its_own_position() {
        let mut bytes = blob(0x08, 0x10, 5, 0x40);
        bytes[0x18..0x1d].copy_from_slice(b"hello");
        let string = BlobString::read(&bytes, 0x08, "text").expect("the header parses");
        assert_eq!(string.at, 0x18);
        assert_eq!(string.len, 5);
        assert_eq!(string.text(&bytes, "text").expect("utf-8"), "hello");
    }

    #[test]
    fn a_header_reaching_past_the_blob_is_refused() {
        let cases: &[(usize, u32, u32, usize)] = &[
            // The first element sits past the end.
            (0x00, 0x100, 1, 0x20),
            // The offset alone overflows.
            (0x00, u32::MAX, 1, 0x20),
        ];
        for (at, distance, count, total) in cases {
            let bytes = blob(*at, *distance, *count, *total);
            assert!(
                matches!(
                    BlobArray::read(&bytes, *at, "languages"),
                    Err(Error::BlobRange { .. })
                ),
                "offset {distance} in a {total}-byte blob"
            );
        }
    }

    #[test]
    fn elements_running_past_the_blob_are_refused() {
        let bytes = blob(0x00, 0x08, 100, 0x40);
        let array = BlobArray::read(&bytes, 0x00, "languages").expect("the header parses");
        assert!(matches!(
            array.end(20, bytes.len(), "languages"),
            Err(Error::BlobRange { .. })
        ));
    }

    #[test]
    fn a_string_running_past_the_blob_is_refused() {
        let bytes = blob(0x00, 0x08, 0x100, 0x40);
        assert!(matches!(
            BlobString::read(&bytes, 0x00, "text"),
            Err(Error::BlobRange { .. })
        ));
    }

    #[test]
    fn a_string_of_bytes_that_are_not_utf8_is_refused() {
        let mut bytes = blob(0x00, 0x08, 2, 0x20);
        bytes[0x08] = 0xff;
        bytes[0x09] = 0xfe;
        let string = BlobString::read(&bytes, 0x00, "text").expect("the header parses");
        assert!(matches!(
            string.text(&bytes, "text"),
            Err(Error::NotUtf8 { .. })
        ));
    }
}
