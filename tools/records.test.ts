import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { z } from 'zod';
import {
  appendRecord,
  BUILD_DIGESTS_PATH,
  BuildDigestRecord,
  RecordError,
  readRecords,
  SteamBuildRecord,
} from './records.ts';

/** Every sandbox this file made, removed once the suite ends. */
const sandboxes: string[] = [];

/** A directory of this suite's own, under the temp root the session sets. */
async function sandbox(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), 'ember-records-'));
  sandboxes.push(dir);
  return dir;
}

afterEach(async () => {
  while (sandboxes.length > 0) {
    await rm(sandboxes.pop() as string, { recursive: true, force: true });
  }
});

/** A row that matches `SteamBuildRecord`, for a case to bend one field of. */
const steamRow = {
  observedAt: '2026-09-18T00:00:00.000Z',
  appId: 2278520,
  changeNumber: '38966542',
  branches: { public: '23178631' },
  manifests: { '2278521': '2174935030716737236' },
};

/** A digest of the right shape, for a case that does not care which file. */
const anyDigest = {
  bytes: 22557696,
  sha256: '001c1b40ed091d8c1aee583adde3800d7c858ae2c7f4dff54fca2938b2be1637',
};

/** A row that matches `BuildDigestRecord`, for a case to bend one field of. */
const digestRow = {
  manifestId: '2174935030716737236',
  buildId: '23178631',
  appId: 2278520,
  depotId: 2278521,
  revision: 1024233,
  branch: '^/game38/branches/ea_update_08',
  recordedAt: '2026-09-18',
  files: {
    'enshrouded_server.exe': anyDigest,
    'enshrouded_server.kfc': { ...anyDigest, bytes: 4415488 },
  },
};

describe('readRecords', () => {
  test('a file that is not there reads as no rows', async () => {
    const dir = await sandbox();
    expect(await readRecords(join(dir, 'absent.jsonl'), SteamBuildRecord)).toEqual([]);
  });

  test('blank lines are skipped and the rest keep file order', async () => {
    const dir = await sandbox();
    const path = join(dir, 'rows.jsonl');
    const second = { ...steamRow, branches: { public: '23178632' } };
    await Bun.write(path, `${JSON.stringify(steamRow)}\n\n${JSON.stringify(second)}\n`);
    const rows = await readRecords(path, SteamBuildRecord);
    expect(rows.map((row) => row.branches['public'])).toEqual(['23178631', '23178632']);
  });

  test.each([
    ['not JSON at all', '{', 1],
    ['JSON that is not a row', '42', 1],
    ['a row missing a field', JSON.stringify({ appId: 2278520 }), 1],
  ])('%s is refused, naming its line', async (_name, bad, line) => {
    const dir = await sandbox();
    const path = join(dir, 'rows.jsonl');
    await Bun.write(path, `${JSON.stringify(steamRow)}\n${bad}\n`);
    const read = readRecords(path, SteamBuildRecord);
    await expect(read).rejects.toBeInstanceOf(RecordError);
    await expect(read).rejects.toThrow(`line ${line + 1}`);
  });
});

describe('appendRecord', () => {
  test('a row round-trips through the file it was written to', async () => {
    const dir = await sandbox();
    const path = join(dir, 'rows.jsonl');
    await appendRecord(path, SteamBuildRecord, steamRow);
    await appendRecord(path, SteamBuildRecord, steamRow);
    expect(await readRecords(path, SteamBuildRecord)).toEqual([steamRow, steamRow]);
  });

  test('a row that does not match the shape is refused, and nothing is written', async () => {
    const dir = await sandbox();
    const path = join(dir, 'rows.jsonl');
    const bad = { ...steamRow, appId: -1 } as unknown as SteamBuildRecord;
    await expect(appendRecord(path, SteamBuildRecord, bad)).rejects.toBeInstanceOf(RecordError);
    expect(await Bun.file(path).exists()).toBe(false);
  });
});

describe('SteamBuildRecord', () => {
  /**
   * A manifest gid is larger than Number.MAX_SAFE_INTEGER. A row carrying one
   * as a number has already lost digits, so the shape refuses it rather than
   * recording a gid that was never served.
   */
  test('a Steam identifier is refused as a number', () => {
    const numeric = {
      ...steamRow,
      manifests: { '2278521': 2174935030716737236 },
    };
    expect(SteamBuildRecord.safeParse(numeric).success).toBe(false);
  });

  test.each([
    ['an empty branch table', { ...steamRow, branches: {} }, true],
    ['a non-numeric build id', { ...steamRow, branches: { public: 'x' } }, false],
    ['a null change number', { ...steamRow, changeNumber: null }, true],
    ['a non-integer app id', { ...steamRow, appId: 1.5 }, false],
  ])('%s parses: %o', (_name, value, want) => {
    expect(SteamBuildRecord.safeParse(value).success).toBe(want);
  });

  /**
   * A branch name is the one value Valve controls that reaches
   * `$GITHUB_OUTPUT` and an issue body. GitHub takes the last value for a
   * repeated output key, so a name carrying a newline could set `changed` to
   * false after the real line set it true, and the record would never be
   * committed.
   *
   * The value beside the key is covered above; this covers the key.
   */
  test.each([
    ['a newline', 'public\nchanged=false'],
    ['a carriage return', 'public\rchanged=false'],
    ['a space', 'default old'],
    ['a leading hyphen', '-public'],
    ['a shell substitution', '$(id)'],
    ['a backtick', 'pub`id`lic'],
    ['a semicolon', 'public;id'],
    ['an equals sign', 'public=1'],
    ['a name of 65 characters', 'b'.repeat(65)],
    ['an empty name', ''],
  ])('a branch named with %s is refused', (_name, branch) => {
    const value = { ...steamRow, branches: { [branch]: '23178631' } };
    expect(SteamBuildRecord.safeParse(value).success).toBe(false);
  });

  test.each(['public', 'default_old', 'ea_update_08', 'beta.2', 'b'.repeat(64)])(
    'the branch name %p is accepted',
    (branch) => {
      const value = { ...steamRow, branches: { [branch]: '23178631' } };
      expect(SteamBuildRecord.safeParse(value).success).toBe(true);
    },
  );

  /** A Steam identifier is a 64-bit value, so twenty digits is the ceiling. */
  test.each([
    ['twenty digits', '9'.repeat(20), true],
    ['twenty-one digits', '9'.repeat(21), false],
    ['two hundred thousand digits', '9'.repeat(200000), false],
  ])('a build id of %s parses: %o', (_name, buildId, want) => {
    const value = { ...steamRow, branches: { public: buildId } };
    expect(SteamBuildRecord.safeParse(value).success).toBe(want);
  });
});

describe('BuildDigestRecord', () => {
  /**
   * A file name from a row becomes a path under the directory a pull was asked
   * to fill. A name carrying a separator would write outside it.
   */
  test.each([
    '../enshrouded_server.exe',
    'nested/enshrouded_server.exe',
    'nested\\enshrouded_server.exe',
    '/absolute',
    '.hidden',
    '',
  ])('the file name %p is refused', (fileName) => {
    const value = {
      ...digestRow,
      files: { ...digestRow.files, [fileName]: anyDigest },
    };
    expect(BuildDigestRecord.safeParse(value).success).toBe(false);
  });

  /**
   * `__proto__` passes any character rule and then disappears: `JSON.parse`
   * makes it an own property and a record parse drops it, which is one route
   * to a row that names nothing at all.
   */
  test.each(['constructor', 'prototype'])('the file name %p is refused by name', (fileName) => {
    const value = {
      ...digestRow,
      files: { ...digestRow.files, [fileName]: anyDigest },
    };
    expect(BuildDigestRecord.safeParse(value).success).toBe(false);
  });

  /**
   * `__proto__` is the one name a record parse drops rather than refuses, so a
   * row naming it loses that entry silently. What has to hold is that the loss
   * cannot leave a row naming fewer files than the archive holds, and that
   * reading the row touches no prototype.
   */
  test('a file named __proto__ is dropped, and cannot empty the row', () => {
    const onlyProto = JSON.parse(
      `{"manifestId":"1","buildId":null,"appId":1,"depotId":1,"revision":null,` +
        `"branch":null,"recordedAt":"2026-09-18","files":{"__proto__":` +
        `{"bytes":1,"sha256":"${'a'.repeat(64)}"}}}`,
    ) as unknown;
    expect(BuildDigestRecord.safeParse(onlyProto).success).toBe(false);
    expect(({} as Record<string, unknown>)['bytes'], 'reading the row polluted Object.prototype').toBeUndefined();

    const alongside = { ...digestRow, files: { ...digestRow.files } };
    Object.defineProperty(alongside.files, '__proto__', {
      value: anyDigest,
      enumerable: true,
      configurable: true,
    });
    const parsed = BuildDigestRecord.safeParse(alongside);
    expect(parsed.success).toBe(true);
    expect(parsed.success && Object.hasOwn(parsed.data.files, '__proto__')).toBe(false);
  });

  /**
   * A row that names no file makes `verify` report success having compared
   * nothing, and makes `pull` report a build present in an empty directory.
   * A row naming only one of the two is the quieter version of the same thing.
   */
  test.each([
    ['no files at all', {}],
    ['only the executable', { 'enshrouded_server.exe': anyDigest }],
    ['only the container', { 'enshrouded_server.kfc': anyDigest }],
    ['a file that is not part of a build', { 'readme.txt': anyDigest }],
  ])('a row naming %s is refused', (_name, files) => {
    expect(BuildDigestRecord.safeParse({ ...digestRow, files }).success).toBe(false);
  });

  /**
   * A key the shape does not name would parse and be dropped, and still sit
   * in the committed file asserting something nothing reads. A row claiming a
   * build is archived is the one that matters: only the bucket can say so.
   */
  test.each([
    ['archivedAt', '2026-09-18'],
    ['archived', true],
    ['uploadedAt', '2026-09-18'],
  ])('a row carrying %s is refused, and the key is named', (key, value) => {
    const parsed = BuildDigestRecord.safeParse({ ...digestRow, [key]: value });
    expect(parsed.success).toBe(false);
    expect(parsed.success ? '' : z.prettifyError(parsed.error)).toContain(key);
  });

  test('a row naming both archived files, and more, is accepted', () => {
    const value = {
      ...digestRow,
      files: { ...digestRow.files, '2278521.manifest': anyDigest },
    };
    expect(BuildDigestRecord.safeParse(value).success).toBe(true);
  });

  test.each([
    ['a build id that is not known', { ...digestRow, buildId: null }, true],
    ['a revision that is not known', { ...digestRow, revision: null }, true],
    ['an uppercase digest', uppercaseDigest(), false],
    ['a short digest', shortDigest(), false],
    ['a date with a time on it', { ...digestRow, recordedAt: '2026-09-18T00:00:00Z' }, false],
  ])('%s parses: %o', (_name, value, want) => {
    expect(BuildDigestRecord.safeParse(value).success).toBe(want);
  });

  /**
   * A shell that rewrites paths turns the caret path into a filesystem path on
   * its way to the recorder, and the result still looks like a branch.
   */
  test.each([
    ['^C:/Program Files/Git/game38/branches/ea_update_08', false],
    ['/game38/branches/ea_update_08', false],
    ['game38/branches/ea_update_08', false],
    ['^/game38/branches/ea_update_08', true],
    ['^/game38/trunk', true],
  ])('the branch path %p parses: %o', (branch, want) => {
    expect(BuildDigestRecord.safeParse({ ...digestRow, branch }).success).toBe(want);
  });
});

/** The row with its digest in the casing `sha256sum` never writes. */
function uppercaseDigest(): unknown {
  return {
    ...digestRow,
    files: {
      'enshrouded_server.exe': {
        bytes: 22557696,
        sha256: '001C1B40ED091D8C1AEE583ADDE3800D7C858AE2C7F4DFF54FCA2938B2BE1637',
      },
    },
  };
}

/** The row with a digest one character short of SHA-256. */
function shortDigest(): unknown {
  return {
    ...digestRow,
    files: {
      'enshrouded_server.exe': { bytes: 22557696, sha256: '001c1b40' },
    },
  };
}

describe('the committed digest record', () => {
  /**
   * Every row is read by `status`, `emit`, `push` and `pull`, and a row that
   * does not parse stops each of them. A hand edit is the way one gets in, and
   * nothing else in the gate reads the committed file.
   */
  test('every committed row parses, one row per manifest', async () => {
    const rows = await readRecords(BUILD_DIGESTS_PATH, BuildDigestRecord);
    expect(rows.length).toBeGreaterThan(0);
    const manifests = rows.map((row) => row.manifestId);
    expect(new Set(manifests).size).toBe(manifests.length);
  });
});
