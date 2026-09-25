/**
 * The depot manifest reader, and the attribution the archive client builds on
 * it.
 *
 * @remarks
 * Every manifest here is laid out byte by byte inside the case that uses it.
 * Nothing recovered from a Keen depot is committed, so a real manifest never
 * reaches this file, and a synthetic one is also the only way to write the
 * damaged and forged shapes the refusals exist for.
 *
 * The cases against the builds on disk are gated on `EMBER_ARCHIVE_DIR`, which
 * names the directory holding one subdirectory per archived build:
 *
 * ```text
 * EMBER_ARCHIVE_DIR=<path>\.cache\archive bun test tools/dd-manifest.test.ts
 * ```
 */

import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { compareToManifest, DdManifestError, parseDdManifest, readDdManifest, sha1File } from './dd-manifest.ts';
import { BuildDigestRecord, readRecords } from './records.ts';

/** Every sandbox this file made, removed once the case ends. */
const sandboxes: string[] = [];

/** A directory of this suite's own, under the temp root the session sets. */
async function sandbox(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), 'ember-manifest-'));
  sandboxes.push(dir);
  return dir;
}

afterEach(async () => {
  while (sandboxes.length > 0) {
    await rm(sandboxes.pop() as string, { recursive: true, force: true });
  }
});

/**
 * ///////////////////////////////////////////////
 * Laying out a manifest
 * ///////////////////////////////////////////////
 */

/** The SHA-1 of the three bytes "abc", from the FIPS 180-4 example. */
const ABC_SHA1 = 'a9993e364706816aba3e25717850c26c9cd0d89d';

/** The SHA-256 of the same three bytes, which the digest row pins. */
const ABC_SHA256 = 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad';

/** The depot the dedicated server ships in. */
const DEPOT = 2278521;

/** A manifest gid past 53 bits, which is where a number would lose digits. */
const GID = '5177045887918896292';

/** One protobuf varint, low group first, every group but the last flagged. */
function varint(value: number | bigint): number[] {
  const bytes: number[] = [];
  let left = BigInt(value);
  do {
    const group = Number(left & 0x7fn);
    left >>= 7n;
    bytes.push(left > 0n ? group | 0x80 : group);
  } while (left > 0n);
  return bytes;
}

/** One varint field: its tag, then its value. */
function scalar(field: number, value: number | bigint): number[] {
  return [...varint(field * 8), ...varint(value)];
}

/** One length-delimited field: its tag, its length, then its body. */
function delimited(field: number, body: Iterable<number>): number[] {
  const bytes = [...body];
  return [...varint(field * 8 + 2), ...varint(bytes.length), ...bytes];
}

/** The bytes one hex digest stands for. */
function digest(hex: string): number[] {
  return [...(Buffer.from(hex, 'hex') as Uint8Array)];
}

/** One file, as a manifest describes it. */
interface Mapping {
  /** The path inside the depot, where null leaves the field out. */
  readonly name: string | null;
  /** The size the manifest states. */
  readonly bytes: number;
  /** The content digest, where null leaves the field out. */
  readonly sha1?: string | null;
  /** Fields this reader is not taught, appended to the mapping. */
  readonly unknown?: number[];
}

/**
 * One `ContentManifestPayload.FileMapping`.
 *
 * @remarks
 * Field 4 is the digest of the file name, which the reader never looks at, so
 * it is laid out as zeroes to prove that.
 */
function mapping(one: Mapping): number[] {
  return [
    ...(one.name === null ? [] : delimited(1, new TextEncoder().encode(one.name))),
    ...scalar(2, one.bytes),
    ...scalar(3, 0),
    ...delimited(4, new Uint8Array(20)),
    ...(one.sha1 === null ? [] : delimited(5, digest(one.sha1 ?? ABC_SHA1))),
    ...(one.unknown ?? []),
  ];
}

/** What a synthetic manifest carries, and which parts of it to bend. */
interface Synthetic {
  readonly depotId?: number;
  readonly manifestId?: string;
  readonly encrypted?: boolean;
  readonly mappings?: Mapping[];
  /** Bytes for the signature section, where null leaves the section out. */
  readonly signature?: number[] | null;
  /** Replaces the word that opens the payload section. */
  readonly payloadMagic?: number;
  /** Whether the closing word is written. */
  readonly close?: boolean;
  /** Fields this reader is not taught, appended to the metadata. */
  readonly unknown?: number[];
  /** Bytes written after the closing word. */
  readonly trailer?: number[];
}

/** One section: its magic word, its length, then its body. */
function section(magic: number, body: number[]): number[] {
  const head = new Uint8Array(8);
  const view = new DataView(head.buffer);
  view.setUint32(0, magic, true);
  view.setUint32(4, body.length, true);
  return [...head, ...body];
}

/** The two files every archived build carries, as one manifest states them. */
const BOTH: Mapping[] = [
  { name: 'enshrouded_server.exe', bytes: 3 },
  { name: 'enshrouded_server.kfc', bytes: 3 },
];

/** A whole manifest file, in the layout DepotDownloader writes. */
function manifest(options: Synthetic = {}): Uint8Array {
  const payload = (options.mappings ?? BOTH).flatMap((one) => delimited(1, mapping(one)));
  const metadata = [
    ...scalar(1, options.depotId ?? DEPOT),
    ...scalar(2, BigInt(options.manifestId ?? GID)),
    ...scalar(3, 1776863233),
    ...scalar(4, options.encrypted === true ? 1 : 0),
    ...(options.unknown ?? []),
  ];
  const signature = options.signature === null ? [] : section(0x1b81b817, options.signature ?? []);
  const closing = options.close === false ? [] : [0xab, 0x15, 0xc4, 0x32];
  return new Uint8Array([
    ...section(options.payloadMagic ?? 0x71f617d0, payload),
    ...section(0x1f4812be, metadata),
    ...signature,
    ...closing,
    ...(options.trailer ?? []),
  ]);
}

/**
 * Write a manifest and the checksum beside it.
 *
 * @remarks
 * The checksum is computed rather than written down, because the manifest's
 * bytes change with every case. That it is the real SHA-1 rests on the case
 * that runs {@link sha1File} against the published vector for "abc".
 *
 * @param dir - The build directory the manifest belongs to.
 * @param bytes - The manifest.
 * @param options - `gid` names the file, and `checksum` replaces the bytes
 * written beside it.
 * @returns The path the manifest was written to.
 */
async function writeManifest(
  dir: string,
  bytes: Uint8Array,
  options: { gid?: string; checksum?: Uint8Array } = {},
): Promise<string> {
  const path = join(dir, '.DepotDownloader', `${DEPOT}_${options.gid ?? GID}.manifest`);
  await Bun.write(path, bytes);
  await Bun.write(
    `${path}.sha`,
    options.checksum ?? new Uint8Array(new Bun.CryptoHasher('sha1').update(bytes).digest()),
  );
  return path;
}

/** A build directory shaped like one DepotDownloader leaves behind. */
async function fetched(options: Synthetic & { gid?: string } = {}): Promise<{
  dir: string;
  path: string;
}> {
  const dir = await sandbox();
  await Bun.write(join(dir, 'enshrouded_server.exe'), 'abc');
  await Bun.write(join(dir, 'enshrouded_server.kfc'), 'abc');
  const path = await writeManifest(dir, manifest(options), {
    gid: options.gid,
  });
  return { dir, path };
}

/** The app manifest SteamCMD leaves, naming one gid for the server depot. */
async function writeAppManifest(dir: string, gid: string): Promise<string> {
  const path = join(dir, 'steamapps', 'appmanifest_2278520.acf');
  await Bun.write(
    path,
    [
      '"AppState"',
      '{',
      '\t"buildid"\t\t"23178631"',
      '\t"InstalledDepots"',
      '\t{',
      `\t\t"${DEPOT}"`,
      '\t\t{',
      `\t\t\t"manifest"\t\t"${gid}"`,
      '\t\t}',
      '\t}',
      '}',
    ].join('\n'),
  );
  return path;
}

/**
 * ///////////////////////////////////////////////
 * The reader
 * ///////////////////////////////////////////////
 */

describe('sha1File', () => {
  /**
   * The digest Valve's manifest states is SHA-1, and the published vector is
   * what says this reads the same function the manifest was written with.
   */
  test('the published vector for abc is what a file of abc hashes to', async () => {
    const path = join(await sandbox(), 'abc');
    await Bun.write(path, 'abc');
    expect(await sha1File(path)).toEqual({ bytes: 3, sha1: ABC_SHA1 });
  });
});

describe('parseDdManifest', () => {
  test('the depot, the manifest gid and every mapping are read', () => {
    const read = parseDdManifest(manifest(), '<laid out here>');
    expect(read.depotId).toBe(DEPOT);
    expect(read.manifestId).toBe(GID);
    expect(read.filenamesEncrypted).toBe(false);
    expect([...read.files.keys()]).toEqual(['enshrouded_server.exe', 'enshrouded_server.kfc']);
    expect(read.files.get('enshrouded_server.exe')).toEqual({
      name: 'enshrouded_server.exe',
      bytes: 3,
      sha1: ABC_SHA1,
    });
  });

  /**
   * A gid exceeds `Number.MAX_SAFE_INTEGER`, so a reader that made it a number
   * would change it and still look right.
   */
  test('a gid past 53 bits keeps every digit', () => {
    const read = parseDdManifest(manifest({ manifestId: '18446744073709551615' }), '<laid out here>');
    expect(read.manifestId).toBe('18446744073709551615');
  });

  test('an encrypted payload is reported rather than decoded', () => {
    expect(parseDdManifest(manifest({ encrypted: true }), '<laid out here>').filenamesEncrypted).toBe(true);
  });

  /** Valve extends these messages, and an older reader has to survive it. */
  test.each([
    ['a varint', [...scalar(31, 1)]],
    ['a length-delimited field', [...delimited(32, [1, 2, 3])]],
    ['a fixed 32-bit field', [0xfd, 0x01, 1, 2, 3, 4]],
    ['a fixed 64-bit field', [0xf9, 0x01, 1, 2, 3, 4, 5, 6, 7, 8]],
  ])('%s this reader does not know is skipped', (_name, unknown) => {
    const read = parseDdManifest(
      manifest({
        unknown,
        mappings: [{ name: 'enshrouded_server.exe', bytes: 3, unknown }],
      }),
      '<laid out here>',
    );
    expect(read.manifestId).toBe(GID);
    expect(read.files.get('enshrouded_server.exe')?.sha1).toBe(ABC_SHA1);
  });

  /**
   * The section is skipped rather than refused. Valve signs the payload, and
   * what reaches disk is DepotDownloader's own serialization, which drops the
   * signature. A manifest that carried one still has to read.
   */
  test('a signature section is skipped', () => {
    const read = parseDdManifest(manifest({ signature: [...delimited(1, [1, 2, 3, 4])] }), '<laid out here>');
    expect(read.manifestId).toBe(GID);
  });

  test('a manifest with no signature section reads', () => {
    expect(parseDdManifest(manifest({ signature: null }), '<laid out here>').manifestId).toBe(GID);
  });

  test.each([
    [
      'a section magic word this format does not use',
      manifest({ payloadMagic: 0x12345678 }),
      'opens a section with magic 0x12345678',
    ],
    ['the older binary manifest', manifest({ payloadMagic: 0x16349781 }), 'is a Steam3 binary manifest'],
    [
      'a length past the end of the file',
      manifest().subarray(0, 40),
      String.raw`claims \d+ bytes at offset 8 and holds 32`,
    ],
    ['no closing magic word', manifest({ close: false }), 'does not end with the closing magic word'],
    ['bytes after the closing magic word', manifest({ trailer: [1, 2, 3, 4] }), 'carries 4 bytes after it ends'],
    [
      'a content digest that is not a SHA-1',
      manifest({
        mappings: [
          {
            name: 'enshrouded_server.exe',
            bytes: 3,
            sha1: null,
            unknown: [...delimited(5, [1, 2, 3])],
          },
        ],
      }),
      'states a 3 byte content digest',
    ],
    [
      'a mapping whose name is empty',
      manifest({ mappings: [{ name: '', bytes: 3 }] }),
      'carries a file mapping with no name',
    ],
    [
      'a mapping with no name field',
      manifest({ mappings: [{ name: null, bytes: 3 }] }),
      'carries a file mapping with no name',
    ],
    ['a wire type protobuf removed', manifest({ unknown: [...varint(30 * 8 + 3)] }), 'carries wire type 3'],
  ])('%s is refused, naming the file', (_name, bytes, detail) => {
    expect(() => parseDdManifest(bytes, '<the file>')).toThrow(DdManifestError);
    // Each detail is a regular expression, and the file name opens every
    // message, so a refusal that dropped either one fails here.
    expect(() => parseDdManifest(bytes, '<the file>')).toThrow(new RegExp(`^<the file> ${detail}`));
  });

  /** A mapping with no size is a mapping this reader cannot check. */
  test('a mapping with no size is refused', () => {
    const bytes = new Uint8Array([
      ...section(0x71f617d0, delimited(1, [...delimited(1, new TextEncoder().encode('enshrouded_server.exe'))])),
      ...section(0x1f4812be, [...scalar(1, DEPOT), ...scalar(2, BigInt(GID))]),
      0xab,
      0x15,
      0xc4,
      0x32,
    ]);
    expect(() => parseDdManifest(bytes, '<the file>')).toThrow(
      '<the file> states no readable size for enshrouded_server.exe',
    );
  });

  test('a manifest with no metadata section is refused', () => {
    const bytes = new Uint8Array([
      ...section(0x71f617d0, delimited(1, mapping(BOTH[0] as Mapping))),
      0xab,
      0x15,
      0xc4,
      0x32,
    ]);
    expect(() => parseDdManifest(bytes, '<the file>')).toThrow('<the file> carries no metadata section');
  });

  test('a manifest with no payload section is refused', () => {
    const bytes = new Uint8Array([
      ...section(0x1f4812be, [...scalar(1, DEPOT), ...scalar(2, BigInt(GID))]),
      0xab,
      0x15,
      0xc4,
      0x32,
    ]);
    expect(() => parseDdManifest(bytes, '<the file>')).toThrow('<the file> carries no payload section');
  });
});

describe('readDdManifest', () => {
  test('a manifest and its checksum read', async () => {
    const { path } = await fetched();
    expect((await readDdManifest(path)).manifestId).toBe(GID);
  });

  test('a manifest that is not there is refused, naming it', async () => {
    const path = join(await sandbox(), 'absent.manifest');
    await expect(readDdManifest(path)).rejects.toThrow(`${path} is not there`);
  });

  test('a manifest with no checksum beside it is refused, naming it', async () => {
    const dir = await sandbox();
    const path = join(dir, `${DEPOT}_${GID}.manifest`);
    await Bun.write(path, manifest());
    await expect(readDdManifest(path)).rejects.toThrow(`${path}.sha is not there`);
  });

  /**
   * One byte of the manifest moved, which is what a hand-edited attribution
   * looks like when the checksum is left alone.
   */
  test('a manifest its checksum does not describe is refused, naming both', async () => {
    const dir = await sandbox();
    const good = manifest();
    const path = await writeManifest(dir, good);
    const bent = new Uint8Array(good);
    bent[bent.length - 5] = (bent[bent.length - 5] as number) ^ 0xff;
    await Bun.write(path, bent);
    const failure = readDdManifest(path);
    await expect(failure).rejects.toThrow(DdManifestError);
    await expect(failure).rejects.toThrow(/hashes to [0-9a-f]{40}, and .*\.sha says/);
  });

  test('a checksum that is not a raw SHA-1 is refused, naming its length', async () => {
    const dir = await sandbox();
    const path = await writeManifest(dir, manifest(), {
      checksum: new Uint8Array(4),
    });
    await expect(readDdManifest(path)).rejects.toThrow(`${path}.sha holds 4 bytes, and a raw SHA-1 is 20`);
  });
});

describe('compareToManifest', () => {
  const names = ['enshrouded_server.exe', 'enshrouded_server.kfc'];

  test('a build whose bytes match the manifest has no problem', async () => {
    const { dir, path } = await fetched();
    const read = await readDdManifest(path);
    expect(await compareToManifest(read, dir, names)).toEqual([]);
  });

  test('a file the manifest does not describe is a problem, naming it', async () => {
    const { dir, path } = await fetched({
      mappings: [{ name: 'enshrouded_server.exe', bytes: 3 }],
    });
    const read = await readDdManifest(path);
    expect(await compareToManifest(read, dir, names)).toEqual([
      {
        fileName: 'enshrouded_server.kfc',
        detail: `is not one of the files ${path} describes`,
      },
    ]);
  });

  test('a file the manifest describes and the directory lacks is a problem', async () => {
    const { dir, path } = await fetched();
    await rm(join(dir, 'enshrouded_server.kfc'));
    const read = await readDdManifest(path);
    expect(await compareToManifest(read, dir, names)).toEqual([
      { fileName: 'enshrouded_server.kfc', detail: `is not in ${dir}` },
    ]);
  });

  test('a file of another size is a problem, naming both sizes', async () => {
    const { dir, path } = await fetched();
    await Bun.write(join(dir, 'enshrouded_server.kfc'), 'abcd');
    const read = await readDdManifest(path);
    expect(await compareToManifest(read, dir, names)).toEqual([
      {
        fileName: 'enshrouded_server.kfc',
        detail: 'is 4 bytes, and the manifest says 3',
      },
    ]);
  });

  /** The same size and other bytes, which a size check alone lets through. */
  test('a file of the same size and other bytes is a problem', async () => {
    const { dir, path } = await fetched();
    await Bun.write(join(dir, 'enshrouded_server.kfc'), 'abd');
    const read = await readDdManifest(path);
    const [problem] = await compareToManifest(read, dir, names);
    expect(problem?.fileName).toBe('enshrouded_server.kfc');
    expect(problem?.detail).toMatch(new RegExp(`^hashes to [0-9a-f]{40}, and the manifest says ${ABC_SHA1}$`));
  });

  /**
   * A mapping with no content digest cannot be checked, and the chunk list is
   * not walked to rebuild one. A refusal sends it to a person.
   */
  test('a mapping with no content digest is a problem, naming the file', async () => {
    const { dir, path } = await fetched({
      mappings: [
        { name: 'enshrouded_server.exe', bytes: 3 },
        { name: 'enshrouded_server.kfc', bytes: 3, sha1: null },
      ],
    });
    const read = await readDdManifest(path);
    expect(await compareToManifest(read, dir, names)).toEqual([
      {
        fileName: 'enshrouded_server.kfc',
        detail: `has no content digest in ${path}`,
      },
    ]);
  });
});

/**
 * ///////////////////////////////////////////////
 * The attribution, driven through the command
 * ///////////////////////////////////////////////
 */

describe('the record command against a DepotDownloader fetch', () => {
  /**
   * The real command, pointed at a sandbox record so no case can append to the
   * repository's own. Running the command rather than a piece of it is the
   * point: the refusal lives in the command, and a case over an extracted
   * fragment would stay green if the command stopped calling it.
   */
  // No return annotation: the literal options below narrow stdout and stderr
  // to strings, and naming the general type throws that away.
  const record = (dir: string, recordPath: string, extra: string[] = []) =>
    Bun.spawnSync({
      cmd: [
        process.execPath,
        'run',
        fileURLToPath(new URL('archive.ts', import.meta.url)),
        'record',
        '--dir',
        dir,
        '--manifest',
        GID,
        '--record',
        recordPath,
        ...extra,
      ],
      env: { ...process.env, GITHUB_OUTPUT: '' },
      stdout: 'pipe',
      stderr: 'pipe',
    });

  test('a build its manifest describes is recorded', async () => {
    const { dir } = await fetched();
    const recordPath = join(await sandbox(), 'build-digests.jsonl');
    const run = record(dir, recordPath);
    expect(run.stdout.toString()).toContain("enshrouded_server.exe matches Valve's digest");
    expect(run.exitCode).toBe(0);
    const rows = await readRecords(recordPath, BuildDigestRecord);
    expect(rows.map((one) => one.manifestId)).toEqual([GID]);
    expect(rows[0]?.files['enshrouded_server.exe']?.sha256).toBe(ABC_SHA256);
  });

  test.each([
    [
      'one byte of the build flipped',
      async (dir: string): Promise<void> => {
        await Bun.write(join(dir, 'enshrouded_server.exe'), 'abd');
      },
      'enshrouded_server.exe hashes to',
    ],
    [
      'a file the row names cut out of the build',
      async (dir: string): Promise<void> => {
        await rm(join(dir, 'enshrouded_server.kfc'));
      },
      'enshrouded_server.kfc is not in',
    ],
  ])('%s is refused, and records nothing', async (_name, bend, detail) => {
    const { dir } = await fetched();
    await bend(dir);
    const recordPath = join(await sandbox(), 'build-digests.jsonl');
    const run = record(dir, recordPath);
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain(detail);
    expect(run.stdout.toString()).toContain("Valve's manifest");
    expect(await Bun.file(recordPath).exists()).toBe(false);
  });

  /**
   * The forgery the digests exist to catch: a manifest for one build renamed
   * to claim another. The gid in the file name matches the row, and the gid
   * inside the manifest does not.
   */
  test('a manifest renamed to claim another build is refused, naming both gids', async () => {
    const { dir } = await fetched({ manifestId: '954904204024183479' });
    const recordPath = join(await sandbox(), 'build-digests.jsonl');
    const run = record(dir, recordPath);
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain('states manifest 954904204024183479 inside, and its name says ' + GID);
    expect(await Bun.file(recordPath).exists()).toBe(false);
  });

  test('a manifest for another depot is refused, naming both depots', async () => {
    const { dir } = await fetched({ depotId: 1004 });
    const recordPath = join(await sandbox(), 'build-digests.jsonl');
    const run = record(dir, recordPath);
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain(`describes depot 1004, and this build is depot ${DEPOT}`);
    expect(await Bun.file(recordPath).exists()).toBe(false);
  });

  /** Encrypted names cannot be matched, so no file in the build is attested. */
  test('a manifest with encrypted file names is refused', async () => {
    const { dir } = await fetched({ encrypted: true });
    const recordPath = join(await sandbox(), 'build-digests.jsonl');
    const run = record(dir, recordPath);
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain('carries encrypted file names');
    expect(await Bun.file(recordPath).exists()).toBe(false);
  });

  test.each([
    ['truncated', (bytes: Uint8Array): Uint8Array => bytes.subarray(0, 40)],
    ['opened with another magic word', (): Uint8Array => manifest({ payloadMagic: 0x12345678 })],
  ])('a manifest %s is a parse refusal naming the file', async (_name, bend) => {
    const dir = await sandbox();
    await Bun.write(join(dir, 'enshrouded_server.exe'), 'abc');
    await Bun.write(join(dir, 'enshrouded_server.kfc'), 'abc');
    const path = await writeManifest(dir, bend(manifest()));
    const recordPath = join(await sandbox(), 'build-digests.jsonl');
    const run = record(dir, recordPath);
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain(path);
    expect(run.stdout.toString()).toContain('cannot be read');
    expect(await Bun.file(recordPath).exists()).toBe(false);
  });

  test('a manifest its checksum does not describe is refused, naming it', async () => {
    const dir = await sandbox();
    await Bun.write(join(dir, 'enshrouded_server.exe'), 'abc');
    await Bun.write(join(dir, 'enshrouded_server.kfc'), 'abc');
    const path = await writeManifest(dir, manifest(), {
      checksum: new Uint8Array(20),
    });
    const recordPath = join(await sandbox(), 'build-digests.jsonl');
    const run = record(dir, recordPath);
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain(path);
    expect(run.stdout.toString()).toContain('hashes to');
    expect(await Bun.file(recordPath).exists()).toBe(false);
  });

  /**
   * The name rule runs before the attribution, which hashes every name it is
   * handed. A caller's own path reaching that is a hash oracle over the
   * filesystem, so the refusal has to be the name rule rather than a later
   * one.
   */
  test('a file name that is not one plain name is refused, and nothing is hashed', async () => {
    const { dir } = await fetched();
    const recordPath = join(await sandbox(), 'build-digests.jsonl');
    const run = record(dir, recordPath, ['--file', '../enshrouded_server.exe']);
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain('is not one plain archived file name');
    expect(run.stdout.toString()).not.toMatch(/[0-9a-f]{40}/);
    expect(await Bun.file(recordPath).exists()).toBe(false);
  });

  /**
   * The flag covers a directory with no evidence. Evidence that is there and
   * disagrees is a fact rather than a gap, so no flag waves it through.
   *
   * There are two disagreements, and the flag sits beside the second one. This
   * case bends the gid inside the manifest, which `attestFetch` refuses.
   */
  test('--unattested does not wave a manifest that disagrees with itself through', async () => {
    const { dir } = await fetched({ manifestId: '954904204024183479' });
    const recordPath = join(await sandbox(), 'build-digests.jsonl');
    const run = record(dir, recordPath, ['--unattested']);
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain('states manifest');
    expect(await Bun.file(recordPath).exists()).toBe(false);
  });

  /**
   * The second disagreement: the evidence names a manifest the caller did not
   * ask for. This is the branch `--unattested` sits beside, so it is the one
   * an edit could widen the flag onto. The fixture renames the manifest file
   * rather than editing it, which is the only way to reach that branch.
   */
  test('--unattested does not wave a gid that disagrees with the caller through', async () => {
    const { dir } = await fetched({ gid: '954904204024183479' });
    const recordPath = join(await sandbox(), 'build-digests.jsonl');
    const run = record(dir, recordPath, ['--unattested']);
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain('says this build came from manifest');
    expect(await Bun.file(recordPath).exists()).toBe(false);
  });

  /**
   * A directory whose history mixed the two fetch routes carries both records.
   * Taking the first would let the app manifest stand for the cached manifest
   * beside it, and the bytes would go unhashed under a green row.
   */
  test('a build carrying both records is still attested', async () => {
    const { dir } = await fetched();
    await writeAppManifest(dir, GID);
    const recordPath = join(await sandbox(), 'build-digests.jsonl');
    const run = record(dir, recordPath);
    expect(run.stdout.toString()).toContain("enshrouded_server.exe matches Valve's digest");
    expect(run.exitCode).toBe(0);
    const rows = await readRecords(recordPath, BuildDigestRecord);
    expect(rows.map((one) => one.manifestId)).toEqual([GID]);
  });

  test('a build whose app manifest names another gid is refused, naming it', async () => {
    const { dir } = await fetched();
    const acf = await writeAppManifest(dir, '954904204024183479');
    const recordPath = join(await sandbox(), 'build-digests.jsonl');
    const run = record(dir, recordPath);
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain(acf);
    expect(run.stdout.toString()).toContain('says this build came from manifest 954904204024183479');
    expect(await Bun.file(recordPath).exists()).toBe(false);
  });

  /**
   * Two cached manifests state two gids, because the gid is in the file name.
   * One of them disagrees with the caller whatever the caller asked for, so
   * the directory is refused rather than resolved by directory order.
   */
  test('a second cached manifest is refused, naming it', async () => {
    const { dir } = await fetched();
    await writeManifest(dir, manifest({ manifestId: '954904204024183479' }), {
      gid: '954904204024183479',
    });
    const recordPath = join(await sandbox(), 'build-digests.jsonl');
    const run = record(dir, recordPath);
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain('says this build came from manifest 954904204024183479');
    expect(await Bun.file(recordPath).exists()).toBe(false);
  });

  /**
   * A cache that is there and cannot be listed is not a cache that is absent.
   * Reading it as absent would put the directory in the branch the flag waves
   * through, which is how evidence would be lost by making it unreadable.
   */
  test('a cache that cannot be listed is refused, even with --unattested', async () => {
    const dir = await sandbox();
    await Bun.write(join(dir, 'enshrouded_server.exe'), 'abc');
    await Bun.write(join(dir, 'enshrouded_server.kfc'), 'abc');
    await Bun.write(join(dir, '.DepotDownloader'), 'not a directory');
    const recordPath = join(await sandbox(), 'build-digests.jsonl');
    const run = record(dir, recordPath, ['--unattested']);
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain('could not be listed');
    expect(run.stdout.toString()).not.toContain("is the caller's word");
    expect(await Bun.file(recordPath).exists()).toBe(false);
  });
});

describe('the emit command against a DepotDownloader fetch', () => {
  /** The row a committed record already holds for this build. */
  const committed = {
    manifestId: GID,
    buildId: null,
    appId: 2278520,
    depotId: DEPOT,
    revision: null,
    branch: null,
    recordedAt: '2026-09-18',
    files: {
      'enshrouded_server.exe': { bytes: 3, sha256: ABC_SHA256 },
      'enshrouded_server.kfc': { bytes: 3, sha256: ABC_SHA256 },
    },
  };

  // No return annotation: the literal options below narrow stdout and stderr
  // to strings, and naming the general type throws that away.
  const emit = (dir: string, recordPath: string) =>
    Bun.spawnSync({
      cmd: [
        process.execPath,
        'run',
        fileURLToPath(new URL('archive.ts', import.meta.url)),
        'emit',
        '--dir',
        dir,
        '--manifest',
        GID,
        '--record',
        recordPath,
      ],
      env: { ...process.env, GITHUB_OUTPUT: '' },
      stdout: 'pipe',
      stderr: 'pipe',
    });

  /** A build with a row takes the same attribution as one without. */
  const recorded = async (): Promise<string> => {
    const recordPath = join(await sandbox(), 'build-digests.jsonl');
    await Bun.write(recordPath, `${JSON.stringify(committed)}\n`);
    return recordPath;
  };

  test('a build its manifest describes is handed on', async () => {
    const { dir } = await fetched();
    const run = emit(dir, await recorded());
    expect(run.stdout.toString()).toContain("enshrouded_server.exe matches Valve's digest");
    expect(run.exitCode).toBe(0);
  });

  test('a manifest renamed to claim another build is refused', async () => {
    const { dir } = await fetched({ manifestId: '954904204024183479' });
    const run = emit(dir, await recorded());
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain('states manifest 954904204024183479 inside');
  });
});

/**
 * ///////////////////////////////////////////////
 * The builds on disk
 * ///////////////////////////////////////////////
 */

/** The directory holding one subdirectory per archived build. */
const archiveDir = process.env['EMBER_ARCHIVE_DIR'];

describe('the archived builds', () => {
  test.skipIf(archiveDir === undefined || archiveDir.length === 0)(
    'every archived build is attributed by its own manifest',
    async () => {
      const { readdir } = await import('node:fs/promises');
      const gids = (await readdir(archiveDir as string, { withFileTypes: true }))
        .filter((entry) => entry.isDirectory() && /^\d{1,20}$/.test(entry.name))
        .map((entry) => entry.name);
      // A wrong path would leave nothing to check and report a pass.
      expect(gids.length).toBeGreaterThan(0);

      for (const gid of gids) {
        const dir = join(archiveDir as string, gid);
        const read = await readDdManifest(join(dir, '.DepotDownloader', `${DEPOT}_${gid}.manifest`));
        expect(read.manifestId).toBe(gid);
        expect(read.depotId).toBe(DEPOT);
        expect(read.filenamesEncrypted).toBe(false);
        expect(await compareToManifest(read, dir, ['enshrouded_server.exe', 'enshrouded_server.kfc'])).toEqual([]);
      }
    },
  );
});
