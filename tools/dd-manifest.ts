/**
 * Reads the depot manifest DepotDownloader caches beside a fetched build.
 *
 * @remarks
 * A historical build is attributed to a manifest gid, and the gid in a file
 * name is a claim rather than evidence: renaming the file forges it. The
 * manifest itself carries Valve's SHA-1 over the bytes of every file in the
 * depot, so hashing the local copy against it turns the attribution into
 * something a reader can check.
 *
 * The file is a run of sections, each opening with a little-endian magic word.
 * The payload section and the metadata section each carry a length and then a
 * protobuf message. The signature section carries a length and bytes this
 * reader skips. The last magic word closes the file and carries nothing.
 *
 * Only the fields the attribution needs are decoded: the depot id and the
 * manifest gid out of the metadata, and each mapping's name, size and content
 * digest out of the payload.
 *
 * What it cannot prove is that Valve wrote the manifest. DepotDownloader
 * serializes the message itself rather than storing what the content server
 * sent, so the signature section reaching disk is empty, and Valve publishes no
 * key a signature could be checked against.
 *
 * @example
 * ```ts
 * const manifest = await readDdManifest(
 *   ".cache/archive/<manifest id>/.DepotDownloader/2278521_<manifest id>.manifest",
 * );
 * manifest.files.get("enshrouded_server.exe")?.sha1;
 * ```
 */

import { Buffer } from 'node:buffer';
import { join } from 'node:path';

/**
 * ///////////////////////////////////////////////
 * The file's fixed shape
 * ///////////////////////////////////////////////
 */

/** Opens the section holding the file mappings. */
const PAYLOAD_MAGIC = 0x71f617d0;

/** Opens the section holding the depot id and the manifest gid. */
const METADATA_MAGIC = 0x1f4812be;

/** Opens the section holding Valve's signature over the payload. */
const SIGNATURE_MAGIC = 0x1b81b817;

/** Closes the file, and carries no length and no body. */
const END_MAGIC = 0x32c415ab;

/** Opens the older binary manifest, which this reader does not decode. */
const STEAM3_MAGIC = 0x16349781;

/** Bytes a SHA-1 digest occupies, in the manifest and in the checksum file. */
const SHA1_BYTES = 20;

/** The extension DepotDownloader gives the checksum beside a cached manifest. */
const CHECKSUM_EXTENSION = '.sha';

/**
 * ///////////////////////////////////////////////
 * What a manifest says
 * ///////////////////////////////////////////////
 */

/** One file the depot carries, as the manifest describes it. */
export interface DdManifestFile {
  /** The path inside the depot, which for an archived file is a plain name. */
  readonly name: string;
  /** The file's size in bytes. */
  readonly bytes: number;
  /**
   * Valve's SHA-1 over the file's contents, lowercase hex.
   *
   * @remarks
   * Null for a mapping that states none, which a directory entry does. A file
   * with no digest cannot be attested, and the caller refuses rather than
   * reconstructing one from the chunk list.
   */
  readonly sha1: string | null;
}

/** What one depot manifest says about a depot at one point in its history. */
export interface DdManifest {
  /** The file it was read from, so a reader can check it by hand. */
  readonly source: string;
  /** The depot the manifest describes. */
  readonly depotId: number;
  /** The manifest gid, as a decimal string, because it exceeds 53 bits. */
  readonly manifestId: string;
  /**
   * Whether the file names in the payload are ciphertext.
   *
   * @remarks
   * Valve encrypts the names in a depot whose contents are not public, and the
   * key comes from a Steam session. Every manifest this project archives leaves
   * them clear.
   */
  readonly filenamesEncrypted: boolean;
  /** Every mapping the payload carries, by the name it states. */
  readonly files: ReadonlyMap<string, DdManifestFile>;
}

/** A refusal raised when a manifest file is absent, damaged or unreadable. */
export class DdManifestError extends Error {
  /** The file the refusal is about. */
  readonly source: string;

  constructor(source: string, detail: string) {
    super(`${source} ${detail}`);
    this.name = 'DdManifestError';
    this.source = source;
  }
}

/**
 * ///////////////////////////////////////////////
 * Reading the bytes
 * ///////////////////////////////////////////////
 */

/**
 * A cursor over one manifest file, which refuses to read past its end.
 *
 * @remarks
 * Every length in the file comes from the file, so a truncated or damaged
 * manifest asks for bytes that are not there. Each read is bounded here, once,
 * rather than at each of the places that decode a field.
 */
class Cursor {
  readonly #bytes: Uint8Array;
  readonly #view: DataView;
  readonly #source: string;
  #at: number;

  constructor(bytes: Uint8Array, source: string, at = 0, end = bytes.byteLength) {
    this.#bytes = bytes.subarray(0, end);
    this.#view = new DataView(bytes.buffer, bytes.byteOffset, end);
    this.#source = source;
    this.#at = at;
  }

  /** How many bytes are left to read. */
  get remaining(): number {
    return this.#bytes.byteLength - this.#at;
  }

  /** Where the next read starts, for a refusal that names a position. */
  get at(): number {
    return this.#at;
  }

  /** Refuse this file, naming what is wrong with it. */
  fail(detail: string): DdManifestError {
    return new DdManifestError(this.#source, detail);
  }

  /** Take `length` bytes, or refuse when the file is shorter than it claims. */
  take(length: number): Uint8Array {
    if (length < 0 || length > this.remaining) {
      throw this.fail(`claims ${length} bytes at offset ${this.#at} and holds ${this.remaining}`);
    }
    const slice = this.#bytes.subarray(this.#at, this.#at + length);
    this.#at += length;
    return slice;
  }

  /** Read one little-endian 32-bit word. */
  uint32(): number {
    const at = this.#at;
    this.take(4);
    return this.#view.getUint32(at, true);
  }

  /** Read one protobuf varint. */
  varint(): bigint {
    let value = 0n;
    let shift = 0n;
    // A 64-bit value reaches ten groups of seven bits, and a varint longer
    // than that is a damaged file rather than a larger number.
    for (let group = 0; group < 10; group += 1) {
      const byte = this.take(1)[0] as number;
      value |= BigInt(byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) {
        return value;
      }
      shift += 7n;
    }
    throw this.fail(`carries a varint at offset ${this.#at} with no end`);
  }

  /** A cursor over the next `length` bytes, for decoding one message. */
  section(length: number): Cursor {
    const at = this.#at;
    this.take(length);
    return new Cursor(this.#bytes, this.#source, at, at + length);
  }
}

/** One protobuf field, as its number and the bytes or number it carries. */
interface Field {
  readonly number: number;
  /** The value of a varint field, and zero for any other wire type. */
  readonly value: bigint;
  /** The body of a length-delimited field, and null for any other wire type. */
  readonly bytes: Uint8Array | null;
}

/**
 * Walk every field of one protobuf message.
 *
 * @remarks
 * Unknown fields are skipped by wire type, which is what lets this read a
 * message Valve extends without being taught the new field. The group wire
 * types are refused, because protobuf removed them and nothing in this format
 * uses them.
 *
 * @param cursor - A cursor bounded to the message.
 * @returns Each field in the order the message states it.
 */
function* fields(cursor: Cursor): Generator<Field> {
  while (cursor.remaining > 0) {
    const tag = cursor.varint();
    const number = Number(tag >> 3n);
    const wire = Number(tag & 7n);
    if (wire === 0) {
      yield { number, value: cursor.varint(), bytes: null };
    } else if (wire === 1) {
      cursor.take(8);
      yield { number, value: 0n, bytes: null };
    } else if (wire === 2) {
      yield { number, value: 0n, bytes: cursor.take(Number(cursor.varint())) };
    } else if (wire === 5) {
      cursor.take(4);
      yield { number, value: 0n, bytes: null };
    } else {
      throw cursor.fail(`carries wire type ${wire} at offset ${cursor.at}, which protobuf removed`);
    }
  }
}

/** Lowercase hex, as `sha1sum` and `Bun.CryptoHasher` write it. */
function hex(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString('hex');
}

/**
 * Decode one `ContentManifestPayload.FileMapping`.
 *
 * @param cursor - A cursor bounded to the mapping.
 * @returns The name, the size and the content digest it states.
 */
function fileMapping(cursor: Cursor): DdManifestFile {
  let name: string | null = null;
  let bytes: bigint | null = null;
  let sha1: string | null = null;
  for (const field of fields(cursor)) {
    if (field.number === 1 && field.bytes !== null) {
      name = new TextDecoder().decode(field.bytes);
    } else if (field.number === 2) {
      bytes = field.value;
    } else if (field.number === 5 && field.bytes !== null) {
      if (field.bytes.byteLength !== SHA1_BYTES) {
        throw cursor.fail(`states a ${field.bytes.byteLength} byte content digest, and a SHA-1 is ${SHA1_BYTES}`);
      }
      sha1 = hex(field.bytes);
    }
  }
  if (name === null || name.length === 0) {
    throw cursor.fail('carries a file mapping with no name');
  }
  if (bytes === null || bytes > BigInt(Number.MAX_SAFE_INTEGER)) {
    throw cursor.fail(`states no readable size for ${name}`);
  }
  return { name, bytes: Number(bytes), sha1 };
}

/**
 * Read a depot manifest out of the bytes of one.
 *
 * @param bytes - The whole file.
 * @param source - Where the bytes came from, for every refusal this raises.
 * @returns What the manifest says about the depot.
 * @throws {@link DdManifestError} When the bytes are not one manifest:
 * a section magic word this format does not use, a length past the end of the
 * file, a missing payload or metadata section, or anything after the closing
 * magic word.
 */
export function parseDdManifest(bytes: Uint8Array, source: string): DdManifest {
  const cursor = new Cursor(bytes, source);
  const files = new Map<string, DdManifestFile>();
  let payloadSeen = false;
  let metadata: Cursor | null = null;
  let closed = false;

  while (cursor.remaining > 0) {
    const magic = cursor.uint32();
    if (magic === END_MAGIC) {
      closed = true;
      break;
    }
    if (magic === STEAM3_MAGIC) {
      throw cursor.fail('is a Steam3 binary manifest, which this does not read');
    }
    if (magic !== PAYLOAD_MAGIC && magic !== METADATA_MAGIC && magic !== SIGNATURE_MAGIC) {
      throw cursor.fail(`opens a section with magic 0x${magic.toString(16)} at offset ${cursor.at - 4}`);
    }
    const section = cursor.section(cursor.uint32());
    if (magic === PAYLOAD_MAGIC) {
      payloadSeen = true;
      for (const field of fields(section)) {
        if (field.number === 1 && field.bytes !== null) {
          const mapping = fileMapping(new Cursor(field.bytes, source));
          files.set(mapping.name, mapping);
        }
      }
    } else if (magic === METADATA_MAGIC) {
      metadata = section;
    }
  }

  if (!closed) {
    throw cursor.fail('does not end with the closing magic word');
  }
  if (cursor.remaining > 0) {
    throw cursor.fail(`carries ${cursor.remaining} bytes after it ends`);
  }
  if (!payloadSeen) {
    throw cursor.fail('carries no payload section, so it names no file');
  }
  if (metadata === null) {
    throw cursor.fail('carries no metadata section, so it names no manifest');
  }

  let depotId: number | null = null;
  let manifestId: string | null = null;
  let filenamesEncrypted = false;
  for (const field of fields(metadata)) {
    if (field.number === 1) {
      depotId = Number(field.value);
    } else if (field.number === 2) {
      manifestId = field.value.toString();
    } else if (field.number === 4) {
      filenamesEncrypted = field.value !== 0n;
    }
  }
  if (depotId === null || manifestId === null) {
    throw cursor.fail('states no depot id and manifest gid of its own');
  }
  return { source, depotId, manifestId, filenamesEncrypted, files };
}

/**
 * Read the depot manifest at `path`, after checking it against its checksum.
 *
 * @remarks
 * DepotDownloader writes the raw SHA-1 of the manifest it cached into a file
 * beside it. Checking that first separates a manifest that arrived damaged from
 * one this reader cannot decode, and both refusals name the file.
 *
 * @param path - The cached manifest.
 * @returns What the manifest says about the depot.
 * @throws {@link DdManifestError} When either file is absent, when the
 * checksum does not describe the manifest, or when the manifest does not
 * decode.
 */
export async function readDdManifest(path: string): Promise<DdManifest> {
  const file = Bun.file(path);
  if (!(await file.exists())) {
    throw new DdManifestError(path, 'is not there');
  }
  const checksumPath = `${path}${CHECKSUM_EXTENSION}`;
  const checksum = Bun.file(checksumPath);
  if (!(await checksum.exists())) {
    throw new DdManifestError(checksumPath, 'is not there, and it is what says the manifest beside it arrived whole');
  }
  const declared = new Uint8Array(await checksum.arrayBuffer());
  if (declared.byteLength !== SHA1_BYTES) {
    throw new DdManifestError(checksumPath, `holds ${declared.byteLength} bytes, and a raw SHA-1 is ${SHA1_BYTES}`);
  }
  const bytes = new Uint8Array(await file.arrayBuffer());
  const actual = new Bun.CryptoHasher('sha1').update(bytes).digest('hex');
  if (actual !== hex(declared)) {
    throw new DdManifestError(path, `hashes to ${actual}, and ${checksumPath} says ${hex(declared)}`);
  }
  return parseDdManifest(bytes, path);
}

/**
 * ///////////////////////////////////////////////
 * Checking local bytes against a manifest
 * ///////////////////////////////////////////////
 */

/** One way a local file disagreed with the manifest that describes it. */
export interface ManifestProblem {
  /** The file the problem is about. */
  readonly fileName: string;
  /** What is wrong, as a clause that follows the file name. */
  readonly detail: string;
}

/** What one file on disk weighs and hashes to, in the digest Valve states. */
export interface Sha1Digest {
  /** The file's size in bytes, counted while it was hashed. */
  readonly bytes: number;
  /** The file's SHA-1, lowercase hex. */
  readonly sha1: string;
}

/**
 * Hash one file without holding it in memory.
 *
 * @remarks
 * SHA-1 because that is the digest Valve's manifest states. The archive's own
 * integrity rests on the SHA-256 in the digest record, which is a separate
 * pass over the same bytes.
 *
 * @param path - The file to read.
 * @returns Its size and its SHA-1, both from the one pass.
 */
export async function sha1File(path: string): Promise<Sha1Digest> {
  const hasher = new Bun.CryptoHasher('sha1');
  let bytes = 0;
  for await (const chunk of Bun.file(path).stream()) {
    hasher.update(chunk);
    bytes += chunk.byteLength;
  }
  return { bytes, sha1: hasher.digest('hex') };
}

/**
 * Compare the files in a directory against the manifest that attributes them.
 *
 * @remarks
 * Hashing every named file costs one pass over the build, which is the price of
 * turning a gid in a file name into a statement about the bytes. Only a build
 * seeded by hand from a historical manifest takes this path.
 *
 * @param manifest - The manifest read from beside the build.
 * @param dir - The directory holding the build.
 * @param fileNames - The files to check, which the caller has already accepted
 * as plain archived names.
 * @returns One problem per file that the manifest does not describe, that is
 * absent, or whose bytes disagree. An empty array means every file matched.
 */
export async function compareToManifest(
  manifest: DdManifest,
  dir: string,
  fileNames: readonly string[],
): Promise<ManifestProblem[]> {
  const problems: ManifestProblem[] = [];
  for (const fileName of fileNames) {
    const stated = manifest.files.get(fileName);
    if (stated === undefined) {
      problems.push({
        fileName,
        detail: `is not one of the files ${manifest.source} describes`,
      });
      continue;
    }
    if (stated.sha1 === null) {
      problems.push({
        fileName,
        detail: `has no content digest in ${manifest.source}`,
      });
      continue;
    }
    const path = join(dir, fileName);
    if (!(await Bun.file(path).exists())) {
      problems.push({ fileName, detail: `is not in ${dir}` });
      continue;
    }
    const local = await sha1File(path);
    if (local.bytes !== stated.bytes) {
      problems.push({
        fileName,
        detail: `is ${local.bytes} bytes, and the manifest says ${stated.bytes}`,
      });
      continue;
    }
    if (local.sha1 !== stated.sha1) {
      problems.push({
        fileName,
        detail: `hashes to ${local.sha1}, and the manifest says ${stated.sha1}`,
      });
    }
  }
  return problems;
}
