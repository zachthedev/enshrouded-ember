/**
 * Reads a BinDiff result database and prints what it proposes for an address.
 *
 * @remarks
 * The result of a diff is a SQLite database. `bindiff.exe` writes one per pair,
 * and every proposal this prints is a row in its `function` table: the primary
 * input's address in `address1`, the secondary input's in `address2`. The
 * primary is the build whose addresses are known, so a signature row for a new
 * build is read out of `address2`.
 *
 * **A proposal is a candidate, never an answer.** Similarity and confidence
 * describe the match and the algorithm that made it, not whether the function
 * is the one a hook wants. Confirm each one against the binary itself before it
 * reaches a signature row.
 *
 * An address with no row prints as a miss rather than as nothing, because that
 * is the answer that matters most: an address Ghidra never made a function is
 * absent from the export, and BinDiff can never match it.
 *
 * @example
 * ```sh
 * bun run tools/bindiff-read.ts result.BinDiff --side primary 0x1401aba90
 * ```
 */

import { Database } from 'bun:sqlite';
import { basename } from 'node:path';

/**
 * ///////////////////////////////////////////////
 * Rows the result database holds
 * ///////////////////////////////////////////////
 */

/** One matched pair of functions. */
export interface MatchRow {
  /** The function's address in the primary input. */
  readonly address1: number;
  /** The function's name in the primary input, which Ghidra usually invents. */
  readonly name1: string | null;
  /** The function's address in the secondary input. */
  readonly address2: number;
  /** The function's name in the secondary input. */
  readonly name2: string | null;
  /** How alike the two functions are, from 0 to 1. */
  readonly similarity: number;
  /** How much the matching algorithm is trusted, from 0 to 1. */
  readonly confidence: number;
  /** The algorithm that made the match. */
  readonly algorithm: string | null;
  /** Basic blocks in the matched function. */
  readonly basicblocks: number;
  /** Edges in the matched function. */
  readonly edges: number;
  /** Instructions in the matched function. */
  readonly instructions: number;
}

/** What the differ recorded about the run itself. */
interface MetadataRow {
  readonly version: string;
  readonly similarity: number;
  readonly confidence: number;
  readonly created: string;
}

/** One of the two inputs. */
interface FileRow {
  readonly filename: string;
  readonly functions: number;
  readonly basicblocks: number;
  readonly instructions: number;
}

/** Which input an address belongs to. */
export type Side = 'primary' | 'secondary';

/** The columns a match is read out of, per side. */
const ADDRESS_COLUMN: Record<Side, string> = {
  primary: 'address1',
  secondary: 'address2',
};

/** Every column a proposal carries, with its algorithm resolved to a name. */
const MATCH_COLUMNS = `f.address1, f.name1, f.address2, f.name2, f.similarity, f.confidence,
        a.name as algorithm, f.basicblocks, f.edges, f.instructions
   from function f left join functionalgorithm a on a.id = f.algorithm`;

/**
 * ///////////////////////////////////////////////
 * Reading
 * ///////////////////////////////////////////////
 */

/** An address as this tool prints it. */
export function hex(value: number): string {
  return `0x${value.toString(16)}`;
}

/**
 * What the differ says about the run and its two inputs.
 *
 * @param db - The result database.
 * @returns One line per fact, in the order they are printed.
 */
export function describeRun(db: Database): string[] {
  const lines: string[] = [];
  const metadata = db
    .query('select version, similarity, confidence, created from metadata')
    .get() as MetadataRow | null;
  if (metadata !== null) {
    lines.push(
      `version ${metadata.version} similarity ${metadata.similarity.toFixed(4)} ` +
        `confidence ${metadata.confidence.toFixed(4)} created ${metadata.created}`,
    );
  }
  const files = db
    .query('select filename, functions, basicblocks, instructions from file order by id')
    .all() as FileRow[];
  for (const [index, file] of files.entries()) {
    lines.push(
      `${index === 0 ? 'primary' : 'secondary'} ${file.filename} functions ${file.functions} ` +
        `basicblocks ${file.basicblocks} instructions ${file.instructions}`,
    );
  }
  const matched = db.query('select count(*) as count from function').get() as { count: number };
  const moved = db.query('select count(*) as count from function where address1 <> address2').get() as {
    count: number;
  };
  lines.push(`matched functions ${matched.count}`);
  lines.push(`matched at a different address ${moved.count}`);
  return lines;
}

/**
 * The match for one address, on the side it belongs to.
 *
 * @param db - The result database.
 * @param side - Which input the address belongs to.
 * @param address - The address, as a number.
 * @returns The match, or null when the differ proposes none.
 */
export function matchFor(db: Database, side: Side, address: number): MatchRow | null {
  return db.query(`select ${MATCH_COLUMNS} where f.${ADDRESS_COLUMN[side]} = ?`).get(address) as MatchRow | null;
}

/**
 * The largest matches that sit at a different address in the two builds.
 *
 * @remarks
 * A pair where nothing moved proves nothing about the procedure, so this is how
 * a run says whether it was a real test of it.
 *
 * @param db - The result database.
 * @param limit - How many to return.
 * @returns The matches, largest first.
 */
export function moved(db: Database, limit: number): MatchRow[] {
  return db
    .query(`select ${MATCH_COLUMNS} where f.address1 <> f.address2 order by f.instructions desc limit ?`)
    .all(limit) as MatchRow[];
}

/** One proposal, as a line. */
export function describeMatch(match: MatchRow): string {
  return (
    `${hex(match.address1)} ${match.name1 ?? '-'} -> ${hex(match.address2)} ${match.name2 ?? '-'} ` +
    `similarity ${match.similarity.toFixed(4)} confidence ${match.confidence.toFixed(4)} ` +
    `algorithm ${match.algorithm ?? '-'} basicblocks ${match.basicblocks} edges ${match.edges} ` +
    `instructions ${match.instructions}`
  );
}

/**
 * Read one address, with or without the `0x` prefix.
 *
 * @param text - The address as it was typed.
 * @returns The address, or null when the text is not one.
 */
export function parseAddress(text: string): number | null {
  const digits = /^0[xX]([0-9a-fA-F]+)$/.exec(text)?.[1] ?? (/^[0-9a-fA-F]+$/.test(text) ? text : null);
  if (digits === null) {
    return null;
  }
  const value = Number.parseInt(digits, 16);
  return Number.isSafeInteger(value) ? value : null;
}

/**
 * ///////////////////////////////////////////////
 * Command line
 * ///////////////////////////////////////////////
 */

/** What the flags asked for. */
interface Options {
  readonly database: string;
  readonly side: Side;
  readonly moved: number;
  readonly addresses: number[];
}

/** Print the closing line and pick the exit code. */
function summary(text: string, failed: boolean): never {
  console.log('');
  console.log(`  ${text}`);
  process.exit(failed ? 1 : 0);
}

/** Refuse the command line, naming what it should have been. */
function usage(detail: string): never {
  console.error(detail);
  console.error(
    `usage: ${basename(import.meta.file)} <result.BinDiff> [--side primary|secondary] [--moved N] <0xaddress>...`,
  );
  process.exit(2);
}

/**
 * Read the command line.
 *
 * @param argv - The arguments past the script name.
 * @returns What to do.
 */
function parse(argv: string[]): Options {
  const database = argv[0];
  if (database === undefined) {
    usage('the first argument is the result database');
  }
  let side: Side = 'primary';
  let count = 0;
  const addresses: number[] = [];
  for (let index = 1; index < argv.length; index += 1) {
    const argument = argv[index] as string;
    if (argument === '--side') {
      const value = argv[index + 1];
      if (value !== 'primary' && value !== 'secondary') {
        usage(`--side is primary or secondary, and this is ${String(value)}`);
      }
      side = value;
      index += 1;
      continue;
    }
    if (argument === '--moved') {
      const value = Number(argv[index + 1] ?? '');
      if (!Number.isInteger(value) || value < 0) {
        usage(`--moved is a whole number of examples, and this is ${String(argv[index + 1])}`);
      }
      count = value;
      index += 1;
      continue;
    }
    const address = parseAddress(argument);
    if (address === null) {
      usage(`${argument} is not a hexadecimal address`);
    }
    addresses.push(address);
  }
  return { database, side, moved: count, addresses };
}

if (import.meta.main) {
  const options = parse(Bun.argv.slice(2));
  const db = new Database(options.database, { readonly: true });

  console.log('bindiff result');
  console.log('');
  console.log(`  file ${options.database}`);
  for (const line of describeRun(db)) {
    console.log(`  ${line}`);
  }

  if (options.moved > 0) {
    console.log('');
    console.log('  largest matches that moved');
    for (const match of moved(db, options.moved)) {
      console.log(`  · ${describeMatch(match)}`);
    }
  }

  if (options.addresses.length === 0) {
    db.close();
    summary('no address was asked about', false);
  }

  console.log('');
  console.log(`  proposals for ${options.side} addresses`);
  let missing = 0;
  for (const address of options.addresses) {
    const match = matchFor(db, options.side, address);
    if (match === null) {
      missing += 1;
      // An address the exporter never carried as a function reads exactly like
      // one the differ could not place, and both are this line.
      console.log(`  ✗ ${hex(address)} no match`);
      continue;
    }
    console.log(`  ✓ ${describeMatch(match)}`);
  }
  db.close();

  const found = options.addresses.length - missing;
  summary(`${found} of ${options.addresses.length} addresses have a proposal`, missing > 0);
}
