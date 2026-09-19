/**
 * The append-only records under `data/`, and the reader the tools share.
 *
 * @remarks
 * `steam-builds.jsonl` is what Steam advertises, gaining a row whenever a build
 * id or a manifest gid moves. `build-digests.jsonl` is
 * what each recorded build's files hash to, and it is the only thing standing
 * between a swapped object in the archive and a developer who runs it.
 *
 * Neither file says whether the archive holds a build. A digest row pins bytes
 * and is not a receipt for an upload. Only the bucket knows what it holds, so
 * `archive status` asks it.
 *
 * Both are JSON Lines rather than JSON. A row is appended and never rewritten,
 * so a diff shows one added line, and prettier never reflows a file that two
 * writers append to.
 *
 * Every Steam identifier is kept as a decimal string. A manifest gid such as
 * 2174935030716737236 is larger than `Number.MAX_SAFE_INTEGER`, so a reader
 * that converts it to a number silently changes it.
 */

import { appendFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { z } from "zod";

/**
 * ///////////////////////////////////////////////
 * Shared field schemas
 * ///////////////////////////////////////////////
 */

/**
 * A Steam application, depot, build or manifest identifier.
 *
 * @remarks
 * Bounded at twenty digits, the ceiling for the 64-bit value each of these is.
 * One reaches a path component, an issue title and a command-line argument, so
 * an unbounded one reaches each of them.
 */
const steamId = z
  .string()
  .regex(/^\d{1,20}$/, "a Steam identifier is up to twenty decimal digits");

/** A SHA-256 digest, lowercase hex, as `sha256sum` and `Bun.CryptoHasher` write it. */
const sha256 = z
  .string()
  .regex(/^[0-9a-f]{64}$/, "a SHA-256 digest is 64 lowercase hex characters");

/**
 * The files an archived build carries, and the order they are handled in.
 *
 * @remarks
 * A digest row has to name every one of these. A row naming a subset verifies
 * only that subset, and `pull` fetches only that subset while still reporting
 * the build complete, which is the quieter half of naming none at all.
 */
export const ARCHIVE_FILES = [
  "enshrouded_server.exe",
  "enshrouded_server.kfc",
] as const;

/**
 * One file inside an archived build.
 *
 * @remarks
 * The name becomes both an object key and a path under the output directory,
 * so it is one plain file name. A row naming `../enshrouded_server.exe` would
 * otherwise write outside the directory the caller asked for.
 *
 * `__proto__` is refused separately from the alphabet, because it passes any
 * character rule and then vanishes: `JSON.parse` makes it an own property, a
 * record parse drops it, and the row is left naming nothing.
 */
const archiveFileName = z
  .string()
  .regex(
    /^[A-Za-z0-9][A-Za-z0-9._-]*$/,
    "an archived file name is one plain file name",
  )
  .refine(
    (name) => !["__proto__", "constructor", "prototype"].includes(name),
    "an archived file name is not a property of Object",
  );

/**
 * Whether a name is one the digest record would accept as an archived file.
 *
 * @remarks
 * The same rule the row enforces, reachable before a file is opened. A caller
 * that hashes first and validates afterwards has already read whatever path it
 * was handed.
 *
 * @param name - The candidate file name.
 * @returns True when a digest row could carry it.
 */
export function isArchiveFileName(name: string): boolean {
  return archiveFileName.safeParse(name).success;
}

/**
 * A Steam branch name.
 *
 * @remarks
 * A branch name reaches a workflow output and an issue body, so the alphabet
 * is narrow enough that a name Steam invents cannot become shell. Every branch
 * Steam has served for these applications is inside it.
 */
const branchName = z
  .string()
  .regex(
    /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/,
    "a branch name is letters, digits, dot, hyphen and underscore",
  );

/**
 * The branch path in a build's `enshrouded_server.kfc` header.
 *
 * @remarks
 * Keen keeps the game content tree in Subversion, so the path is caret syntax
 * and always starts with `^/`. Requiring that catches a shell that rewrote the
 * leading slash into a filesystem path on its way to the recorder.
 */
const branchPath = z
  .string()
  .regex(/^\^\/[A-Za-z0-9._/-]+$/, "a branch path is Subversion caret syntax");

/**
 * ///////////////////////////////////////////////
 * Record shapes
 * ///////////////////////////////////////////////
 */

/**
 * Branch name to build id, as Steam advertises it.
 *
 * @remarks
 * Exported so the parser can check what it read against the same rule the
 * record enforces. A run where nothing moved appends no row, so a parser that
 * left validation to the append would leave that run's values unchecked while
 * still handing them to a workflow.
 */
export const BranchTable = z.record(branchName, steamId);

/** Depot id to manifest gid, on the public branch. */
export const ManifestTable = z.record(steamId, steamId);

/** One observation of what Steam advertises for an application. */
export const SteamBuildRecord = z.object({
  /** When the observation was taken. */
  observedAt: z.iso.datetime(),
  /** The Steam application the row describes. */
  appId: z.number().int().positive(),
  /**
   * The PICS change number at the time of the observation.
   *
   * @remarks
   * Recorded for provenance and never used as a trigger. It advances for
   * reasons unrelated to builds: it moved on 2026-09-17 while app 2278520 sat
   * on the build it has carried since May.
   */
  changeNumber: steamId.nullable(),
  /** Branch name to build id, for every branch the application advertises. */
  branches: BranchTable,
  /** Depot id to manifest gid, on the public branch. */
  manifests: ManifestTable,
});

/** One observation of what Steam advertises for an application. */
export type SteamBuildRecord = z.infer<typeof SteamBuildRecord>;

/** The size and digest of one archived file. */
export const FileDigest = z.object({
  bytes: z.number().int().nonnegative(),
  sha256,
});

/** The size and digest of one archived file. */
export type FileDigest = z.infer<typeof FileDigest>;

/**
 * What one build's files hash to.
 *
 * @remarks
 * Keyed by depot manifest gid, which is the anchor that proves where the bytes
 * came from and the only identifier every recorded build has. A build pulled
 * from a historical manifest carries no recoverable Steam build id, so
 * `buildId` is filled only for a build fetched while its manifest was current.
 *
 * Strict, so a key this shape does not name is refused rather than dropped. A
 * dropped key still sits in the committed file, asserting something nothing
 * reads or checks.
 */
export const BuildDigestRecord = z.strictObject({
  manifestId: steamId,
  buildId: steamId.nullable(),
  appId: z.number().int().positive(),
  depotId: z.number().int().positive(),
  /** The revision in the first 512 bytes of `enshrouded_server.kfc`. */
  revision: z.number().int().positive().nullable(),
  /** The branch path the same header carries. */
  branch: branchPath.nullable(),
  /**
   * The day the files were hashed.
   *
   * @remarks
   * It dates the digests and says nothing about the archive. Whether the
   * bucket holds this build is asked of the bucket.
   */
  recordedAt: z.iso.date(),
  /**
   * What each archived file hashes to.
   *
   * @remarks
   * Every name in {@link ARCHIVE_FILES} has to be here. A row is free to name
   * more, because a later build may archive more, and every extra one is
   * verified the same way. A row naming fewer would make `verify` and `pull`
   * report success over a build they only partly checked.
   */
  files: z
    .record(archiveFileName, FileDigest)
    .refine(
      (files) => ARCHIVE_FILES.every((name) => Object.hasOwn(files, name)),
      `a digest row names every archived file: ${ARCHIVE_FILES.join(", ")}`,
    ),
});

/** What one build's files hash to. */
export type BuildDigestRecord = z.infer<typeof BuildDigestRecord>;

/**
 * ///////////////////////////////////////////////
 * File locations
 * ///////////////////////////////////////////////
 */

/** The directory holding both records, resolved from this module. */
const dataDir = new URL("../data/", import.meta.url);

/** Every observation of what Steam advertises. */
export const STEAM_BUILDS_PATH = fileURLToPath(
  new URL("steam-builds.jsonl", dataDir),
);

/** Every recorded build's file digests. */
export const BUILD_DIGESTS_PATH = fileURLToPath(
  new URL("build-digests.jsonl", dataDir),
);

/**
 * ///////////////////////////////////////////////
 * Reading and appending
 * ///////////////////////////////////////////////
 */

/** A row that failed to parse, named by the line it sits on. */
export class RecordError extends Error {
  constructor(path: string, line: number, detail: string) {
    super(`${path} line ${line}: ${detail}`);
    this.name = "RecordError";
  }
}

/**
 * Read every row of a JSON Lines file.
 *
 * @param path - The file to read. An absent file reads as no rows.
 * @param schema - The shape every row has to match.
 * @returns The rows, in file order.
 * @throws {@link RecordError} When a row is not JSON, or does not match.
 */
export async function readRecords<T>(
  path: string,
  schema: z.ZodType<T>,
): Promise<T[]> {
  const file = Bun.file(path);
  if (!(await file.exists())) {
    return [];
  }
  const rows: T[] = [];
  const lines = (await file.text()).split("\n");
  for (const [index, line] of lines.entries()) {
    if (line.trim().length === 0) {
      continue;
    }
    let value: unknown;
    try {
      value = JSON.parse(line);
    } catch (error) {
      throw new RecordError(
        path,
        index + 1,
        `is not JSON: ${(error as Error).message}`,
      );
    }
    const parsed = schema.safeParse(value);
    if (!parsed.success) {
      throw new RecordError(path, index + 1, z.prettifyError(parsed.error));
    }
    rows.push(parsed.data);
  }
  return rows;
}

/**
 * Append one row, after checking it against the same shape a read enforces.
 *
 * @param path - The file to append to. It is created when absent.
 * @param schema - The shape the row has to match.
 * @param record - The row to write.
 * @throws {@link RecordError} When the row does not match the shape.
 */
export async function appendRecord<T>(
  path: string,
  schema: z.ZodType<T>,
  record: T,
): Promise<void> {
  const parsed = schema.safeParse(record);
  if (!parsed.success) {
    throw new RecordError(path, 0, z.prettifyError(parsed.error));
  }
  await appendFile(path, `${JSON.stringify(parsed.data)}\n`, "utf8");
}
