import { Database } from 'bun:sqlite';
import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describeMatch, describeRun, hex, matchFor, moved, parseAddress } from './bindiff-read.ts';

/** Every sandbox this file made, removed once the case ends. */
const sandboxes: string[] = [];

/** A directory of this suite's own, under the temp root the session sets. */
async function sandbox(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), 'ember-bindiff-'));
  sandboxes.push(dir);
  return dir;
}

afterEach(async () => {
  while (sandboxes.length > 0) {
    await rm(sandboxes.pop() as string, { recursive: true, force: true });
  }
});

/** One row of the `function` table, as the differ writes it. */
interface Match {
  readonly address1: number;
  readonly address2: number;
  readonly algorithm: number;
  readonly instructions: number;
}

/**
 * A result database holding the tables this tool reads.
 *
 * The column set is the differ's own, so a query that drifts from it fails here
 * rather than in front of somebody holding a real pair.
 */
async function result(matches: Match[]): Promise<string> {
  const path = join(await sandbox(), 'old_vs_new.BinDiff');
  const db = new Database(path, { create: true });
  db.run('create table metadata (version text, similarity real, confidence real, created text)');
  db.run("insert into metadata values ('BinDiff 8', 0.5, 0.9, '2026-09-19')");
  db.run('create table file (id integer primary key, filename text, functions int, basicblocks int, instructions int)');
  db.run("insert into file values (1, 'known.BinExport', 26248, 100, 1000)");
  db.run("insert into file values (2, 'wanted.BinExport', 26291, 101, 1001)");
  db.run('create table functionalgorithm (id integer primary key, name text)');
  db.run("insert into functionalgorithm values (1, 'call reference matching')");
  db.run(
    `create table function (address1 int, name1 text, address2 int, name2 text, similarity real,
       confidence real, algorithm int, basicblocks int, edges int, instructions int)`,
  );
  for (const match of matches) {
    db.run(`insert into function values (?, 'sub_a', ?, 'sub_b', 1.0, 0.993, ?, 60, 98, ?)`, [
      match.address1,
      match.address2,
      match.algorithm,
      match.instructions,
    ]);
  }
  db.close();
  return path;
}

/** The known build's address of the function the rehearsal followed. */
const CRAFT_ONCE = 0x1401aba90;

/** The address it sits at in the other build. */
const CRAFT_ONCE_MOVED = 0x1401ab570;

/** A program entry no analysis reaches, which is why seeding exists. */
const UNSEEDED_ENTRY = 0x14006c1a0;

describe('parseAddress', () => {
  test('reads hexadecimal with or without the prefix, and refuses anything else', () => {
    const cases: [string, number | null][] = [
      ['0x14006c1a0', 0x14006c1a0],
      ['0X14006C1A0', 0x14006c1a0],
      ['14006c1a0', 0x14006c1a0],
      ['', null],
      ['0x', null],
      ['zz', null],
      ['0x14006c1a0z', null],
      ['-1', null],
      ['1400 6c1a0', null],
      // Past 2^53, where a JavaScript number stops holding every integer. An
      // address that silently rounds would query a row nobody asked about.
      ['ffffffffffffff', null],
    ];
    for (const [text, want] of cases) {
      expect(parseAddress(text), `address ${text}`).toBe(want);
    }
  });
});

describe('matchFor', () => {
  test('finds a function by the address of the side it belongs to', async () => {
    const path = await result([{ address1: CRAFT_ONCE, address2: CRAFT_ONCE_MOVED, algorithm: 1, instructions: 316 }]);
    const db = new Database(path, { readonly: true });

    const primary = matchFor(db, 'primary', CRAFT_ONCE);
    expect(primary?.address2).toBe(CRAFT_ONCE_MOVED);
    expect(primary?.algorithm).toBe('call reference matching');

    const secondary = matchFor(db, 'secondary', CRAFT_ONCE_MOVED);
    expect(secondary?.address1).toBe(CRAFT_ONCE);

    // The same address on the other side is a different question, and the
    // answer to this one is that there is no such function.
    expect(matchFor(db, 'secondary', CRAFT_ONCE)).toBeNull();
    db.close();
  });

  test('an address the export never carried has no proposal at all', async () => {
    const path = await result([{ address1: CRAFT_ONCE, address2: CRAFT_ONCE_MOVED, algorithm: 1, instructions: 316 }]);
    const db = new Database(path, { readonly: true });

    expect(matchFor(db, 'primary', UNSEEDED_ENTRY)).toBeNull();
    db.close();
  });
});

describe('describeRun', () => {
  test('counts the matches and says how many of them moved', async () => {
    const path = await result([
      { address1: CRAFT_ONCE, address2: CRAFT_ONCE_MOVED, algorithm: 1, instructions: 316 },
      { address1: 0x140100000, address2: 0x140100000, algorithm: 1, instructions: 12 },
    ]);
    const db = new Database(path, { readonly: true });

    const lines = describeRun(db).join('\n');
    expect(lines).toContain('matched functions 2');
    expect(lines).toContain('matched at a different address 1');
    expect(lines).toContain('primary known.BinExport');
    expect(lines).toContain('secondary wanted.BinExport');
    db.close();
  });
});

describe('moved', () => {
  test('returns only the matches that moved, largest first', async () => {
    const path = await result([
      { address1: CRAFT_ONCE, address2: CRAFT_ONCE_MOVED, algorithm: 1, instructions: 316 },
      { address1: 0x140200000, address2: 0x140200500, algorithm: 1, instructions: 900 },
      { address1: 0x140100000, address2: 0x140100000, algorithm: 1, instructions: 4000 },
    ]);
    const db = new Database(path, { readonly: true });

    const rows = moved(db, 5);
    expect(rows.map((row) => row.instructions)).toEqual([900, 316]);
    db.close();
  });
});

describe('describeMatch', () => {
  test('carries both addresses, the scores and the algorithm', async () => {
    const path = await result([{ address1: CRAFT_ONCE, address2: CRAFT_ONCE_MOVED, algorithm: 1, instructions: 316 }]);
    const db = new Database(path, { readonly: true });
    const match = matchFor(db, 'primary', CRAFT_ONCE);
    db.close();

    const line = describeMatch(match as NonNullable<typeof match>);
    expect(line).toContain(hex(CRAFT_ONCE));
    expect(line).toContain(hex(CRAFT_ONCE_MOVED));
    expect(line).toContain('similarity 1.0000');
    expect(line).toContain('confidence 0.9930');
    expect(line).toContain('call reference matching');
    expect(line).toContain('instructions 316');
  });
});

describe('the command, driven end to end', () => {
  /** Run the tool the way `cargo xtask sig read` runs it. */
  function read(path: string, extra: string[]): { stdout: string; code: number } {
    const run = Bun.spawnSync({
      cmd: [process.execPath, 'run', fileURLToPath(new URL('bindiff-read.ts', import.meta.url)), path, ...extra],
      stdout: 'pipe',
      stderr: 'pipe',
    });
    return { stdout: `${run.stdout.toString()}${run.stderr.toString()}`, code: run.exitCode ?? -1 };
  }

  test('an address with a proposal prints it and the command passes', async () => {
    const path = await result([{ address1: CRAFT_ONCE, address2: CRAFT_ONCE_MOVED, algorithm: 1, instructions: 316 }]);

    const run = read(path, ['--side', 'primary', '0x1401aba90']);

    expect(run.stdout).toContain('0x1401ab570');
    expect(run.stdout).toContain('1 of 1 addresses have a proposal');
    expect(run.code).toBe(0);
  });

  /**
   * The answer that matters most. An address Ghidra never made a function is
   * absent from the export, so the differ proposes nothing for it, and a run
   * that printed nothing at all would read as a pass.
   */
  test('an address with no proposal is named as a miss and fails the command', async () => {
    const path = await result([{ address1: CRAFT_ONCE, address2: CRAFT_ONCE_MOVED, algorithm: 1, instructions: 316 }]);

    const run = read(path, ['--side', 'primary', '0x14006c1a0']);

    expect(run.stdout).toContain('0x14006c1a0 no match');
    expect(run.stdout).toContain('0 of 1 addresses have a proposal');
    expect(run.code).toBe(1);
  });

  test('a side that is not one of the two is refused rather than guessed', async () => {
    const path = await result([]);

    const run = read(path, ['--side', 'tertiary', '0x1401aba90']);

    expect(run.stdout).toContain('--side is primary or secondary');
    expect(run.code).toBe(2);
  });

  test('an argument that is not an address is refused rather than read as zero', async () => {
    const path = await result([]);

    const run = read(path, ['--side', 'primary', 'craft_once']);

    expect(run.stdout).toContain('not a hexadecimal address');
    expect(run.code).toBe(2);
  });
});
