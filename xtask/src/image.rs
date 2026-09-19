//! Offline access to a Windows executable.
//!
//! The extractors read a build from disk rather than from a running process, so
//! every address in this module is either a virtual address the image would
//! have at its preferred base or a file offset into the bytes on disk.
//!
//! One rule governs the translation. A section's virtual size can exceed its
//! raw size, and a server build's `.data` section exceeds it by megabytes. A
//! virtual address landing in that gap has no bytes behind it, so
//! [`Image::offset_of`] returns `None` rather than a zero-filled read.

use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};

/// One section of the image, in the two coordinate systems at once.
#[derive(Debug, Clone)]
pub struct Section {
    /// The section name as the header spells it.
    pub name: String,
    /// The first virtual address the section covers.
    pub start: u64,
    /// The size the section occupies once loaded.
    pub virtual_size: u32,
    /// The first file offset the section covers.
    pub raw_offset: usize,
    /// The bytes the section actually occupies on disk.
    pub raw_size: usize,
}

impl Section {
    /// The first virtual address past the section.
    ///
    /// The sum is checked at construction, so a saturating add here is a
    /// formality that keeps the arithmetic from ever wrapping.
    fn end(&self) -> u64 {
        self.start
            .saturating_add(u64::from(self.virtual_size).max(self.raw_size as u64))
    }

    /// The first virtual address past the part with bytes behind it.
    pub fn raw_end(&self) -> u64 {
        self.start.saturating_add(self.raw_size as u64)
    }
}

/// The `CodeView` record that names a build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    /// The RSDS signature, in the byte order the record stores it.
    pub guid: [u8; 16],
    /// The number of times this PDB was rebuilt for the same signature.
    pub age: u32,
}

impl Fingerprint {
    /// The sixteen signature bytes in the order the record stores them, hex and
    /// lowercase.
    ///
    /// This is what a signature row matches on, and it is not what a symbol
    /// server takes. See [`Fingerprint::pdb_guid`].
    pub fn guid_file_order(&self) -> String {
        use std::fmt::Write as _;

        let mut out = String::with_capacity(32);
        for byte in self.guid {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    /// The same bytes as Microsoft renders a PDB GUID.
    ///
    /// The first three fields of a GUID are little-endian, so the record's byte
    /// order and this one differ in the first eight bytes. Only this rendering
    /// finds a PDB: strip the hyphens and upper-case it to build a `symsrv`
    /// path.
    pub fn pdb_guid(&self) -> String {
        let first = u32::from_le_bytes([self.guid[0], self.guid[1], self.guid[2], self.guid[3]]);
        let second = u16::from_le_bytes([self.guid[4], self.guid[5]]);
        let third = u16::from_le_bytes([self.guid[6], self.guid[7]]);
        let tail = self.guid_file_order();
        format!(
            "{first:08x}-{second:04x}-{third:04x}-{}-{}",
            &tail[16..20],
            &tail[20..]
        )
    }
}

/// An executable held in memory, with its section table decoded.
pub struct Image {
    bytes: Vec<u8>,
    sections: Vec<Section>,
    fingerprint: Option<Fingerprint>,
}

impl Image {
    /// Read and parse an executable.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or is not a PE image.
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("reading the image at {}", path.display()))?;
        let pe = goblin::pe::PE::parse(&bytes)
            .with_context(|| format!("parsing {} as a PE image", path.display()))?;
        let optional = pe
            .header
            .optional_header
            .ok_or_else(|| anyhow!("{} has no optional header", path.display()))?;
        let image_base = optional.windows_fields.image_base;
        let mut sections = Vec::with_capacity(pe.sections.len());
        for section in &pe.sections {
            let name = section.name().unwrap_or("").to_string();
            // A crafted header can put ImageBase near the top of the address
            // space, and an address that wraps would alias the bottom of it.
            let start = image_base
                .checked_add(u64::from(section.virtual_address))
                .ok_or_else(|| {
                    anyhow!(
                        "{}: section {name} has a virtual address that overflows past ImageBase {image_base:#x}",
                        path.display()
                    )
                })?;
            let raw_size = (section.size_of_raw_data as usize).min(bytes.len());
            if start
                .checked_add(u64::from(section.virtual_size).max(raw_size as u64))
                .is_none()
            {
                bail!(
                    "{}: section {name} has a virtual size that overflows past its start {start:#x}",
                    path.display()
                );
            }
            sections.push(Section {
                name,
                start,
                virtual_size: section.virtual_size,
                raw_offset: section.pointer_to_raw_data as usize,
                raw_size,
            });
        }
        let fingerprint = pe
            .debug_data
            .and_then(|debug| debug.codeview_pdb70_debug_info)
            .map(|info| Fingerprint {
                guid: info.signature,
                age: info.age,
            });
        Ok(Self {
            bytes,
            sections,
            fingerprint,
        })
    }

    /// The whole file.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The `CodeView` record, when the image carries one.
    pub fn fingerprint(&self) -> Option<&Fingerprint> {
        self.fingerprint.as_ref()
    }

    /// The section with this name.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|section| section.name == name)
    }

    /// The section covering a virtual address.
    pub fn section_of(&self, va: u64) -> Option<&Section> {
        self.sections
            .iter()
            .find(|section| (section.start..section.end()).contains(&va))
    }

    /// The file offset behind a virtual address.
    ///
    /// `None` when the address is outside every section, or inside the part of
    /// a section that has no bytes on disk.
    pub fn offset_of(&self, va: u64) -> Option<usize> {
        let section = self.section_of(va)?;
        if va >= section.raw_end() {
            return None;
        }
        let within = usize::try_from(va - section.start).ok()?;
        let offset = section.raw_offset.checked_add(within)?;
        (offset < self.bytes.len()).then_some(offset)
    }

    /// The virtual address a file offset loads at.
    pub fn va_of(&self, offset: usize) -> Option<u64> {
        let section = self.sections.iter().find(|section| {
            (section.raw_offset..section.raw_offset + section.raw_size).contains(&offset)
        })?;
        Some(section.start + (offset - section.raw_offset) as u64)
    }

    /// The little-endian 64-bit word at a file offset.
    pub fn qword_at(&self, offset: usize) -> Option<u64> {
        let slice: [u8; 8] = self.bytes.get(offset..offset + 8)?.try_into().ok()?;
        Some(u64::from_le_bytes(slice))
    }

    /// The NUL-terminated string at a virtual address, up to `limit` bytes.
    ///
    /// Keen's tables hold ASCII, so the bytes are taken one to one rather than
    /// decoded, which keeps a stray high byte from turning into a replacement
    /// character and changing a dump.
    pub fn cstr(&self, va: u64, limit: usize) -> Option<String> {
        let start = self.offset_of(va)?;
        let end = (start + limit + 1).min(self.bytes.len());
        let slice = self.bytes.get(start..end)?;
        let length = slice.iter().position(|byte| *byte == 0)?;
        Some(slice[..length].iter().map(|byte| *byte as char).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::{Fingerprint, Image, Section};
    use crate::testutil::TestDir;

    fn image_with(sections: Vec<Section>, bytes: Vec<u8>) -> Image {
        Image {
            bytes,
            sections,
            fingerprint: None,
        }
    }

    /// One section of a synthetic image: name, virtual address, virtual size,
    /// raw size.
    type Synthetic = (&'static str, u32, u32, u32);

    /// A minimal PE32+ that `goblin` parses, with the header fields a crafted
    /// input would set.
    ///
    /// Nothing in it comes from a Keen binary. It is the smallest shape that
    /// carries a DOS header, a COFF header, a PE32+ optional header with its
    /// sixteen empty directories, and one section header per entry.
    fn synthetic_pe(image_base: u64, sections: &[Synthetic]) -> Vec<u8> {
        const FILE_ALIGN: u32 = 0x200;
        const OPT_SIZE: u16 = 240;
        let align = |x: u32| x.div_ceil(FILE_ALIGN) * FILE_ALIGN;
        let e_lfanew: u32 = 0x40;
        let count = u16::try_from(sections.len()).expect("a synthetic image has few sections");
        let headers_end = e_lfanew + 4 + 20 + u32::from(OPT_SIZE) + 40 * u32::from(count);
        let headers_raw = align(headers_end);

        let mut out = vec![0u8; e_lfanew as usize];
        out[0..2].copy_from_slice(b"MZ");
        out[0x3C..0x40].copy_from_slice(&e_lfanew.to_le_bytes());
        out.extend_from_slice(&[b'P', b'E', 0, 0]);
        // COFF: machine, sections, timestamp, symbol table, symbols, optional
        // header size, characteristics.
        out.extend_from_slice(&0x8664_u16.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&[0u8; 12]);
        out.extend_from_slice(&OPT_SIZE.to_le_bytes());
        out.extend_from_slice(&0x0022_u16.to_le_bytes());

        let mut opt: Vec<u8> = Vec::new();
        opt.extend_from_slice(&0x20B_u16.to_le_bytes()); // PE32+
        opt.extend_from_slice(&[14, 0]); // linker version
        opt.extend_from_slice(&0x200_u32.to_le_bytes()); // size of code
        opt.extend_from_slice(&0x400_u32.to_le_bytes()); // size of initialized data
        opt.extend_from_slice(&0_u32.to_le_bytes()); // size of uninitialized data
        opt.extend_from_slice(&0x1000_u32.to_le_bytes()); // entry point
        opt.extend_from_slice(&0x1000_u32.to_le_bytes()); // base of code
        opt.extend_from_slice(&image_base.to_le_bytes());
        opt.extend_from_slice(&0x1000_u32.to_le_bytes()); // section alignment
        opt.extend_from_slice(&FILE_ALIGN.to_le_bytes());
        opt.extend_from_slice(&[6, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0]); // os, image, subsystem versions
        opt.extend_from_slice(&0_u32.to_le_bytes()); // win32 version
        let size_of_image = sections
            .iter()
            .map(|(_, va, vsize, _)| va.wrapping_add(*vsize))
            .max()
            .unwrap_or(0x1000);
        opt.extend_from_slice(&size_of_image.to_le_bytes());
        opt.extend_from_slice(&headers_raw.to_le_bytes());
        opt.extend_from_slice(&0_u32.to_le_bytes()); // checksum
        opt.extend_from_slice(&3_u16.to_le_bytes()); // console subsystem
        opt.extend_from_slice(&0_u16.to_le_bytes()); // dll characteristics
        for reserve in [0x10_0000_u64, 0x1000, 0x10_0000, 0x1000] {
            opt.extend_from_slice(&reserve.to_le_bytes());
        }
        opt.extend_from_slice(&0_u32.to_le_bytes()); // loader flags
        opt.extend_from_slice(&16_u32.to_le_bytes()); // directory count
        opt.extend_from_slice(&[0u8; 16 * 8]);
        assert_eq!(opt.len(), usize::from(OPT_SIZE));
        out.extend_from_slice(&opt);

        let mut raw_ptr = headers_raw;
        for (name, va, vsize, raw_size) in sections {
            let mut header = [0u8; 40];
            header[..name.len().min(8)].copy_from_slice(&name.as_bytes()[..name.len().min(8)]);
            header[8..12].copy_from_slice(&vsize.to_le_bytes());
            header[12..16].copy_from_slice(&va.to_le_bytes());
            header[16..20].copy_from_slice(&raw_size.to_le_bytes());
            header[20..24].copy_from_slice(&raw_ptr.to_le_bytes());
            header[36..40].copy_from_slice(&0x6000_0020_u32.to_le_bytes());
            out.extend_from_slice(&header);
            raw_ptr += align(*raw_size);
        }
        out.resize(raw_ptr as usize, 0);
        out
    }

    fn open_synthetic(
        label: &str,
        image_base: u64,
        sections: &[Synthetic],
    ) -> anyhow::Result<Image> {
        let dir = TestDir::new(label);
        let path = dir.write("synthetic.exe", &synthetic_pe(image_base, sections));
        Image::open(&path)
    }

    /// The error text of an open that must fail.
    fn refusal(outcome: anyhow::Result<Image>, why: &str) -> String {
        match outcome {
            Ok(_) => panic!("{why}"),
            Err(err) => format!("{err:#}"),
        }
    }

    #[test]
    fn a_well_formed_synthetic_image_opens_and_maps_its_sections() {
        let image = open_synthetic(
            "pe-baseline",
            0x1_4000_0000,
            &[
                (".text", 0x1000, 0x200, 0x200),
                (".rdata", 0x2000, 0x200, 0x200),
            ],
        )
        .expect("a well-formed image opens");
        let rdata = image.section(".rdata").expect("the section is there");
        assert_eq!(rdata.start, 0x1_4000_2000);
        assert_eq!(rdata.raw_size, 0x200);
        assert!(image.offset_of(0x1_4000_2000).is_some());
    }

    /// An `ImageBase` near the top of the address space makes every section
    /// address wrap. The open fails with an error naming the field rather than
    /// panicking or aliasing the bottom of memory.
    #[test]
    fn a_crafted_image_base_that_overflows_is_a_clean_error() {
        let text = refusal(
            open_synthetic(
                "pe-imgbase",
                0xFFFF_FFFF_FFFF_F000,
                &[(".text", 0x1000, 0x200, 0x200)],
            ),
            "the section address cannot be represented",
        );
        assert!(text.contains("ImageBase"), "{text}");
        assert!(text.contains(".text"), "{text}");
    }

    /// A section whose virtual size runs past the end of the address space is
    /// refused at construction, so no later arithmetic can wrap.
    #[test]
    fn a_crafted_virtual_size_that_overflows_is_a_clean_error() {
        let text = refusal(
            open_synthetic(
                "pe-end-ovf",
                0xFFFF_FFFF_FFFF_0000,
                &[(".text", 0x1000, 0xFFFF_FFFF, 0x200)],
            ),
            "the section end cannot be represented",
        );
        assert!(text.contains("virtual size"), "{text}");
        assert!(text.contains(".text"), "{text}");
    }

    /// A section whose virtual size runs past its raw size has no bytes behind
    /// the tail, and a read there must fail rather than report zeros.
    #[test]
    fn a_read_past_the_raw_size_fails() {
        let image = image_with(
            vec![Section {
                name: ".data".to_string(),
                start: 0x1_4000_1000,
                virtual_size: 0x4000,
                raw_offset: 0,
                raw_size: 0x40,
            }],
            vec![0xAB; 0x40],
        );
        assert_eq!(image.offset_of(0x1_4000_1000), Some(0));
        assert_eq!(image.offset_of(0x1_4000_1038), Some(0x38));
        assert_eq!(
            image.offset_of(0x1_4000_1040),
            None,
            "the raw bytes end here"
        );
        assert_eq!(
            image.offset_of(0x1_4000_2000),
            None,
            "inside the virtual gap"
        );
        assert_eq!(image.qword_at(0x40), None);
    }

    #[test]
    fn offsets_and_addresses_round_trip_inside_the_raw_range() {
        let image = image_with(
            vec![Section {
                name: ".rdata".to_string(),
                start: 0x1_4000_2000,
                virtual_size: 0x100,
                raw_offset: 0x200,
                raw_size: 0x100,
            }],
            vec![0; 0x400],
        );
        for offset in [0x200_usize, 0x250, 0x2FF] {
            let va = image.va_of(offset).expect("offset is inside the section");
            assert_eq!(image.offset_of(va), Some(offset));
        }
        assert_eq!(image.va_of(0x300), None, "past the section");
    }

    /// The two renderings of one signature differ in the first eight bytes,
    /// and a lookup against the wrong one silently finds nothing.
    #[test]
    fn a_fingerprint_renders_in_file_order_and_as_a_pdb_guid() {
        let guid = [
            0xae, 0x33, 0xb4, 0x48, 0x63, 0x80, 0x3d, 0x4e, 0xb3, 0x1f, 0x31, 0xd4, 0xd8, 0x4a,
            0xe2, 0x42,
        ];
        let print = Fingerprint { guid, age: 293 };
        assert_eq!(print.guid_file_order(), "ae33b44863803d4eb31f31d4d84ae242");
        assert_eq!(print.pdb_guid(), "48b433ae-8063-4e3d-b31f-31d4d84ae242");
        assert_ne!(
            print.guid_file_order(),
            print.pdb_guid().replace('-', ""),
            "the two renderings must not be confusable"
        );
    }
}
