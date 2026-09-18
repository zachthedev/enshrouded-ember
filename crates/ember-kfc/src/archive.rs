//! The whole container set: the directory, the payload, and the `.dat` files.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

use crate::directory::{ChunkInfo, Directory};
use crate::error::{Error, Result};
use crate::hash::{ContentHash, ResourceId};

/// How many decompressed chunks stay resident.
///
/// Two, so a resource straddling one chunk boundary decompresses each of its
/// chunks once, and so a walk in stream order hits the cache on every read after
/// the first. Correctness does not depend on the cache: a miss decompresses the
/// chunk again and produces the same bytes.
const CACHE_SLOTS: usize = 2;

/// A container set opened for reading.
///
/// The directory is parsed once and held. The payload file stays open, because a
/// resource read seeks into it. The `.dat` containers are opened per content
/// read, which is a one-off in every path this crate serves.
#[derive(Debug)]
pub struct Archive {
    directory: Directory,
    folder: PathBuf,
    stem: String,
    payload_path: PathBuf,
    payload: File,
    cache: Vec<(usize, Vec<u8>)>,
}

impl Archive {
    /// Open the container set a `.kfc` directory names.
    ///
    /// The payload and the `.dat` containers are resolved beside it, by the
    /// directory's own file stem.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when the directory or the payload cannot be read,
    /// [`Error::PayloadSize`] when the payload file is not the size the chunk
    /// slots sum to, and whatever [`Directory::parse`] refuses.
    pub fn open(kfc: &Path) -> Result<Self> {
        let bytes = std::fs::read(kfc).map_err(|source| Error::Io {
            path: kfc.to_path_buf(),
            source,
        })?;
        let directory = Directory::parse(&bytes)?;
        drop(bytes);

        let folder = kfc.parent().unwrap_or(Path::new(".")).to_path_buf();
        let stem = kfc
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let payload_path = kfc.with_extension("kfc_resources");
        let payload = File::open(&payload_path).map_err(|source| Error::Io {
            path: payload_path.clone(),
            source,
        })?;

        let declared: u64 = directory.chunks().iter().map(|chunk| chunk.size).sum();
        let found = payload
            .metadata()
            .map_err(|source| Error::Io {
                path: payload_path.clone(),
                source,
            })?
            .len();
        if found != declared {
            return Err(Error::PayloadSize {
                path: payload_path,
                found,
                expected: declared,
            });
        }

        Ok(Self {
            directory,
            folder,
            stem,
            payload_path,
            payload,
            cache: Vec::with_capacity(CACHE_SLOTS),
        })
    }

    /// Open the container set beside a game executable.
    ///
    /// The directory carries the executable's stem and a `.kfc` extension, which
    /// holds for the dedicated server and for the client alike.
    ///
    /// # Errors
    ///
    /// The same as [`Self::open`].
    pub fn beside(executable: &Path) -> Result<Self> {
        Self::open(&executable.with_extension("kfc"))
    }

    /// The parsed directory.
    #[must_use]
    pub const fn directory(&self) -> &Directory {
        &self.directory
    }

    /// Where the `.dat` container with this index sits.
    ///
    /// Container files are named for the directory's stem and a three-digit
    /// index, which covers every count the format's power-of-two rule reaches in
    /// practice.
    #[must_use]
    pub fn container_path(&self, index: usize) -> PathBuf {
        self.folder
            .join(format!("{stem}_{index:03}.dat", stem = self.stem))
    }

    /// Check every `.dat` container against the size the directory declares.
    ///
    /// The declared size is the only cross-check the format offers between the
    /// directory and the containers beside it, so a build whose containers were
    /// replaced fails here rather than inside a blob read.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when a container cannot be read and
    /// [`Error::ContainerSize`] when one is not the declared size.
    pub fn verify_containers(&self) -> Result<()> {
        for (index, container) in self.directory.containers().iter().enumerate() {
            let path = self.container_path(index);
            let found = std::fs::metadata(&path)
                .map_err(|source| Error::Io {
                    path: path.clone(),
                    source,
                })?
                .len();
            if found != container.size {
                return Err(Error::ContainerSize {
                    path,
                    found,
                    expected: container.size,
                });
            }
        }
        Ok(())
    }

    /// Read one resource out of the decompressed stream.
    ///
    /// A resource can straddle a chunk boundary, so this decompresses every
    /// chunk its span touches and copies the overlapping bytes out of each.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownResource`] when the directory holds no such id,
    /// [`Error::ResourceRange`] when the span runs past the stream,
    /// [`Error::Decompress`] when a frame does not decompress, and
    /// [`Error::ChunkSize`] when a frame produces a length the directory does
    /// not declare.
    pub fn read_resource(&mut self, id: ResourceId) -> Result<Vec<u8>> {
        let Some(resource) = self.directory.resource(id).copied() else {
            return Err(Error::UnknownResource { id });
        };
        let stream = self.directory.stream().uncompressed_size;
        if resource.end() > stream {
            return Err(Error::ResourceRange {
                id,
                offset: resource.offset,
                end: resource.end(),
                stream,
            });
        }

        let size = usize::try_from(resource.size).map_err(|_| Error::TooLarge {
            what: "a resource",
            size: resource.size,
        })?;
        let spans = self.spans(resource.offset, resource.end());
        let mut out: Vec<u8> = Vec::with_capacity(size);
        for (index, chunk, from, to) in spans {
            let data = self.chunk_data(index, chunk)?;
            let slice = data.get(from..to).ok_or(Error::ChunkSize {
                index,
                found: data.len(),
                expected: chunk.stream_size,
            })?;
            out.extend_from_slice(slice);
        }
        Ok(out)
    }

    /// Read one content blob out of its `.dat` container.
    ///
    /// The blob's length comes from the hash's own first word, because nothing
    /// else in the directory records it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownContent`] when the directory holds no such blob,
    /// [`Error::ContainerIndex`] when the entry names a container the directory
    /// does not declare, [`Error::ContentRange`] when the blob runs past that
    /// container, and [`Error::Io`] when the container cannot be read.
    pub fn read_content(&self, hash: ContentHash) -> Result<Vec<u8>> {
        let Some(entry) = self.directory.content(hash).copied() else {
            return Err(Error::UnknownContent { hash });
        };
        let index = usize::from(entry.container_index);
        let Some(container) = self.directory.containers().get(index) else {
            return Err(Error::ContainerIndex {
                hash,
                index,
                count: self.directory.containers().len(),
            });
        };
        let size = u64::from(hash.size);
        let end = entry.offset.saturating_add(size);
        if end > container.size {
            return Err(Error::ContentRange {
                hash,
                index,
                offset: entry.offset,
                end,
                size: container.size,
            });
        }

        let path = self.container_path(index);
        let mut file = File::open(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        read_span(&mut file, &path, entry.offset, size, "a content blob")
    }

    /// Which chunks a stream span touches, and where inside each one it starts
    /// and ends.
    fn spans(&self, from: u64, to: u64) -> Vec<(usize, ChunkInfo, usize, usize)> {
        self.directory
            .chunks()
            .iter()
            .enumerate()
            .filter_map(|(index, chunk)| {
                if chunk.stream_end() <= from || chunk.stream_offset >= to {
                    return None;
                }
                let start = usize::try_from(from.max(chunk.stream_offset) - chunk.stream_offset)
                    .unwrap_or(usize::MAX);
                let end = usize::try_from(to.min(chunk.stream_end()) - chunk.stream_offset)
                    .unwrap_or(usize::MAX);
                Some((index, *chunk, start, end))
            })
            .collect()
    }

    /// The decompressed bytes of one chunk, from the cache or from the payload.
    fn chunk_data(&mut self, index: usize, chunk: ChunkInfo) -> Result<&[u8]> {
        if let Some(slot) = self.cache.iter().position(|(cached, _)| *cached == index) {
            self.cache[..=slot].rotate_right(1);
        } else {
            let expected = usize::try_from(chunk.stream_size).map_err(|_| Error::TooLarge {
                what: "a chunk",
                size: chunk.stream_size,
            })?;
            // The slot is padded past the frame, and zstd reads the padding as
            // the start of another frame, so only the frame is handed over.
            let frame = read_span(
                &mut self.payload,
                &self.payload_path,
                chunk.offset,
                chunk.frame_size,
                "a chunk",
            )?;
            let data = zstd::bulk::decompress(&frame, expected)
                .map_err(|source| Error::Decompress { index, source })?;
            if data.len() != expected {
                return Err(Error::ChunkSize {
                    index,
                    found: data.len(),
                    expected: chunk.stream_size,
                });
            }
            if self.cache.len() == CACHE_SLOTS {
                self.cache.pop();
            }
            self.cache.insert(0, (index, data));
        }
        self.cache
            .first()
            .map(|(_, data)| data.as_slice())
            .ok_or(Error::ChunkSize {
                index,
                found: 0,
                expected: chunk.stream_size,
            })
    }
}

/// Read an exact span out of an open file.
fn read_span(
    file: &mut File,
    path: &Path,
    offset: u64,
    len: u64,
    what: &'static str,
) -> Result<Vec<u8>> {
    let len = usize::try_from(len).map_err(|_| Error::TooLarge { what, size: len })?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let mut buffer = vec![0u8; len];
    file.read_exact(&mut buffer).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(buffer)
}
