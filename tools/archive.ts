/**
 * Moves archived server builds in and out of the R2 archive, digest first.
 *
 * @remarks
 * Steam serves a depot manifest only while it is current, so a build that was
 * never archived while it was live cannot be fetched again without a Steam
 * credential. The archive is therefore the primary copy rather than a
 * convenience, and the bytes in it are what continuous integration and any
 * developer setting `EMBER_SERVER_EXE` will run.
 *
 * R2 has no object versioning and no Object Lock, so an overwrite is final and
 * nothing detects it. `data/build-digests.jsonl` is what does: every pull
 * hashes what it received and compares it to the committed row before the
 * bytes reach their final name.
 *
 * The destination is never a secret and is never written down here. Both
 * halves of it, the account that owns the endpoint and the bucket inside that
 * account, come from the committed `archive.json` and from nowhere else. A
 * destination read from a secret can be repointed by anyone who can write
 * secrets, and the change leaves no trace; a committed file changes only in a
 * diff a reviewer sees. A fork points the pipeline at its own bucket by
 * editing that one file. Only the key pairs come from the environment.
 *
 * `push` refuses a key that is already in the bucket, and that refusal is
 * advisory rather than atomic. R2 honors `If-None-Match: *` on a put, and Bun
 * 1.3.13's S3 client exposes no way to send it, so the check and the write are
 * two requests. Two writers racing the same key is the uncovered case, and
 * `concurrency: build-watch` is what keeps the scheduled job from being one of
 * them.
 *
 * @example
 * ```sh
 * bun run tools/archive.ts verify --dir .cache/archive/2174935030716737236 \
 *   --manifest 2174935030716737236
 * ```
 */

import { readFileSync } from "node:fs";
import { mkdir, readdir, rename, rm } from "node:fs/promises";
import { basename, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import * as VDF from "vdf-parser";
import { z } from "zod";
import {
  ARCHIVE_FILES,
  appendRecord,
  BUILD_DIGESTS_PATH,
  BuildDigestRecord,
  FileDigest,
  isArchiveFileName,
  readRecords,
  RecordError,
} from "./records.ts";

/**
 * ///////////////////////////////////////////////
 * The archive's fixed shape
 * ///////////////////////////////////////////////
 */

/** Where the archive lives, as `archive.json` states it. */
export interface Destination {
  /** The Cloudflare account whose R2 endpoint serves the bucket. */
  readonly accountId: string;
  /** The bucket inside that account. */
  readonly bucket: string;
}

/**
 * The shape `archive.json` has to have.
 *
 * @remarks
 * JSON carries no comments, so this refusal is where somebody who gets the
 * file wrong is told what it is for. It fires exactly when that happens, which
 * a comment in the file would not.
 */
const DestinationFile = z.object({
  accountId: z
    .string()
    .regex(
      /^[0-9a-f]{32}$/,
      "accountId is a Cloudflare account id: 32 lowercase hex characters, " +
        "which is the first label of the R2 endpoint host",
    ),
  bucket: z
    .string()
    .regex(
      /^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$/,
      "bucket is an R2 bucket name: lowercase letters, digits and hyphens, " +
        "3 to 63 characters",
    ),
});

/** Where `archive.json` sits, resolved from this module. */
const DESTINATION_PATH = fileURLToPath(
  new URL("../archive.json", import.meta.url),
);

/**
 * Read the destination out of the committed configuration file.
 *
 * @remarks
 * The one source for both halves of the destination, and the reason neither is
 * a secret and neither is written down here. A destination read from a secret
 * can be repointed by anyone who can write secrets, and the change leaves no
 * trace; this file is committed, so any change to it shows up in a diff and a
 * reviewer sees it. A fork points the pipeline at its own bucket by editing
 * this one file rather than by editing TypeScript.
 *
 * Nothing in this path reads the environment. That is asserted rather than
 * stated, by a case that runs a process with every plausible override name set
 * to a sentinel.
 *
 * @returns The account and bucket the archive commands reach.
 * @throws When the file is absent or does not match, naming what is wrong.
 */
function readDestination(): Destination {
  let raw: unknown;
  try {
    raw = JSON.parse(readFileSync(DESTINATION_PATH, "utf8"));
  } catch (error) {
    throw new Error(
      `${DESTINATION_PATH} could not be read: ${(error as Error).message}. ` +
        "It names the R2 account and bucket the archive commands reach, and " +
        "a fork points it at its own.",
    );
  }
  const parsed = DestinationFile.safeParse(raw);
  if (!parsed.success) {
    throw new Error(
      `${DESTINATION_PATH} is not a destination: ` +
        z.prettifyError(parsed.error),
    );
  }
  return parsed.data;
}

/** The account and bucket every command here reaches. */
export const DESTINATION: Destination = readDestination();

/** The Steam application every archived build belongs to. */
const APP_ID = 2278520;

/** The Windows depot that carries the dedicated server. */
const DEPOT_ID = 2278521;

/**
 * The key one archived file sits at.
 *
 * @remarks
 * Keyed by depot manifest gid rather than by Steam build id. The gid is the
 * anchor that proves where the bytes came from, and it is the only identifier
 * every archived build has: a build pulled from a historical manifest carries
 * no recoverable build id. The build id lives in the digest row, where it can
 * be null.
 *
 * @param manifestId - The depot manifest gid.
 * @param fileName - One plain file name, as the digest row spells it.
 * @returns The object key inside the bucket.
 */
export function objectKey(manifestId: string, fileName: string): string {
  return `build/${manifestId}/${fileName}`;
}

/**
 * ///////////////////////////////////////////////
 * Provenance
 * ///////////////////////////////////////////////
 */

/** Where a fetched build says which manifest it came from. */
export interface FetchedManifest {
  /** The depot manifest gid the fetching tool recorded. */
  readonly manifestId: string;
  /** The file that said so, so a reader can check it by hand. */
  readonly source: string;
}

/**
 * The manifest gid the tool that filled a directory says it fetched.
 *
 * @remarks
 * Both fetching routes leave this behind, in different places. SteamCMD writes
 * `steamapps/appmanifest_<app>.acf` beside the payload, whose `InstalledDepots`
 * table names a manifest per depot; it is read by depot, because that table
 * lists depots a file filter wrote nothing from. DepotDownloader names its
 * cached manifest `<depot>_<gid>.manifest` under `.DepotDownloader`, so the gid
 * is in the file name.
 *
 * It matters because a fetch can return a different build than the one asked
 * for. SteamCMD's `app_update` takes no manifest parameter and always fetches
 * the head of a branch, so a Keen release landing between the watch job and the
 * archive job would be stored under the previous gid. A digest row has to
 * describe what arrived rather than what was requested.
 *
 * @param dir - The directory the fetch filled.
 * @param appId - The Steam application that was fetched.
 * @param depotId - The depot whose manifest is wanted.
 * @returns What the directory says, or null when it carries no evidence.
 */
export async function fetchedManifest(
  dir: string,
  appId: number,
  depotId: number,
): Promise<FetchedManifest | null> {
  const acf = join(dir, "steamapps", `appmanifest_${appId}.acf`);
  if (await Bun.file(acf).exists()) {
    // The same two options the app info parser passes, for the same two
    // reasons: a gid rounds if it becomes a number, and arrayify keeps the
    // parser off Object.prototype.
    const root = VDF.parse<Record<string, unknown>>(
      await Bun.file(acf).text(),
      { types: false, arrayify: true },
    );
    const state = root["AppState"] as Record<string, unknown> | undefined;
    const installed = state?.["InstalledDepots"] as
      Record<string, unknown> | undefined;
    const entry = installed?.[String(depotId)] as
      Record<string, unknown> | undefined;
    const manifestId = entry?.["manifest"];
    if (typeof manifestId === "string" && /^\d{1,20}$/.test(manifestId)) {
      return { manifestId, source: acf };
    }
  }

  const cache = join(dir, ".DepotDownloader");
  const pattern = new RegExp(`^${depotId}_(\\d{1,20})\\.manifest$`);
  for (const name of await readdir(cache).catch(() => [] as string[])) {
    const match = pattern.exec(name);
    if (match !== null) {
      return { manifestId: match[1] as string, source: join(cache, name) };
    }
  }

  return null;
}

/**
 * ///////////////////////////////////////////////
 * Credentials
 * ///////////////////////////////////////////////
 */

/** Which of the two scoped tokens a command needs. */
export type Access = "read" | "write";

/**
 * The environment variables each access level reads, in report order.
 *
 * @remarks
 * The key pair only. The destination comes from `archive.json`, so nothing
 * that decides where a request goes comes from the environment.
 */
const CREDENTIAL_VARS: Record<Access, readonly [string, string]> = {
  read: ["R2_ARCHIVE_READ_ACCESS_KEY_ID", "R2_ARCHIVE_READ_SECRET_ACCESS_KEY"],
  write: [
    "R2_ARCHIVE_WRITE_ACCESS_KEY_ID",
    "R2_ARCHIVE_WRITE_SECRET_ACCESS_KEY",
  ],
};

/** A refusal raised when the archive credential is not in the environment. */
export class MissingCredentialError extends Error {
  /** The variables that were absent or empty. */
  readonly missing: readonly string[];

  constructor(access: Access, missing: readonly string[]) {
    super(
      `the R2 archive ${access} credential is not set: ${missing.join(", ")}. ` +
        "The tokens are minted by hand in the Cloudflare dashboard and set as " +
        "GitHub secrets, so a run without them reaches no archive.",
    );
    this.name = "MissingCredentialError";
    this.missing = missing;
  }
}

/** The key pair an S3 client signs with. */
export interface Credentials {
  readonly accessKeyId: string;
  readonly secretAccessKey: string;
}

/**
 * Read one access level's key pair out of the environment.
 *
 * @param access - Which token to read.
 * @param env - The environment to read, which a test supplies its own.
 * @returns The S3 key pair.
 * @throws {@link MissingCredentialError} When either half is absent or empty,
 * naming every one that is. A secret GitHub did not pass arrives as an empty
 * string rather than as an absent name, which is the state of a run triggered
 * from a fork.
 */
export function credentials(
  access: Access,
  env: Record<string, string | undefined> = process.env,
): Credentials {
  const [idName, secretName] = CREDENTIAL_VARS[access];
  const missing = [idName, secretName].filter(
    (name) => (env[name] ?? "").length === 0,
  );
  if (missing.length > 0) {
    throw new MissingCredentialError(access, missing);
  }
  return {
    accessKeyId: env[idName] as string,
    secretAccessKey: env[secretName] as string,
  };
}

/**
 * An S3 client for the archive bucket, from credentials already in hand.
 *
 * @remarks
 * Separate from {@link credentials} so the endpoint, the bucket and the key
 * layout can be checked without a real token. Signing is local arithmetic, so
 * a presigned URL built from a throwaway key pair says exactly where a request
 * would have gone, and reaches nothing.
 *
 * @param of - The key pair to sign with.
 * @returns A client for the bucket `archive.json` names, at its account.
 */
export function archiveClient(of: Credentials): Bun.S3Client {
  return new Bun.S3Client({
    accessKeyId: of.accessKeyId,
    secretAccessKey: of.secretAccessKey,
    bucket: DESTINATION.bucket,
    // R2 ignores the region and signs against it, so every request uses the
    // value Cloudflare documents.
    region: "auto",
    endpoint: `https://${DESTINATION.accountId}.r2.cloudflarestorage.com`,
  });
}

/**
 * An S3 client scoped to the archive bucket, signed with one of the two tokens.
 *
 * @param access - Which token to sign with.
 * @returns A client for the bucket `archive.json` names, at its account.
 * @throws {@link MissingCredentialError} When the credential is absent.
 */
function client(access: Access): Bun.S3Client {
  return archiveClient(credentials(access));
}

/**
 * ///////////////////////////////////////////////
 * Digests
 * ///////////////////////////////////////////////
 */

/**
 * Hash one file without holding it in memory.
 *
 * @param path - The file to read.
 * @returns Its size in bytes and its lowercase hex SHA-256.
 */
export async function digestFile(path: string): Promise<FileDigest> {
  const hasher = new Bun.CryptoHasher("sha256");
  let bytes = 0;
  for await (const chunk of Bun.file(path).stream()) {
    hasher.update(chunk);
    bytes += chunk.byteLength;
  }
  return { bytes, sha256: hasher.digest("hex") };
}

/** One way a file failed to match the row that describes it. */
export interface DigestProblem {
  readonly fileName: string;
  readonly detail: string;
}

/**
 * Compare what is on disk against what a row says it must be.
 *
 * @param actual - What each file hashed to, keyed by file name.
 * @param expected - The `files` table of the digest row.
 * @returns One problem per file that is absent, the wrong size, or the wrong
 * digest. An empty array means every named file matched.
 */
export function compareDigests(
  actual: Readonly<Record<string, FileDigest | null>>,
  expected: Readonly<Record<string, FileDigest>>,
): DigestProblem[] {
  const problems: DigestProblem[] = [];
  for (const [fileName, want] of Object.entries(expected)) {
    const got = actual[fileName] ?? null;
    if (got === null) {
      problems.push({ fileName, detail: "is absent" });
      continue;
    }
    if (got.bytes !== want.bytes) {
      problems.push({
        fileName,
        detail: `is ${got.bytes} bytes, and the record says ${want.bytes}`,
      });
    }
    if (got.sha256 !== want.sha256) {
      problems.push({
        fileName,
        detail: `hashes to ${got.sha256}, and the record says ${want.sha256}`,
      });
    }
  }
  return problems;
}

/** A refusal raised when the record holds two rows for one manifest gid. */
export class DuplicateRowError extends Error {
  constructor(manifestId: string, count: number, path: string) {
    super(
      `${path} holds ${count} rows for manifest ${manifestId}. A build is ` +
        "recorded once. A second row for a manifest that already has one " +
        "replaces the digests the first row pinned, which is how a swapped " +
        "object would be blessed.",
    );
    this.name = "DuplicateRowError";
  }
}

/**
 * The digest row for one manifest gid.
 *
 * @remarks
 * A second row for the same manifest is refused here rather than resolved.
 * Taking either one would let an appended line retire the digests the first
 * line pinned, and an appended line is the shape every legitimate change to
 * this file has.
 *
 * @param manifestId - The depot manifest gid.
 * @param path - The record to read.
 * @returns The row, or null when the archive has no row for that manifest.
 * @throws {@link DuplicateRowError} When more than one row names that manifest.
 */
export async function digestRow(
  manifestId: string,
  path: string = BUILD_DIGESTS_PATH,
): Promise<BuildDigestRecord | null> {
  const rows = await readRecords(path, BuildDigestRecord);
  const matching = rows.filter((row) => row.manifestId === manifestId);
  if (matching.length > 1) {
    throw new DuplicateRowError(manifestId, matching.length, path);
  }
  return matching[0] ?? null;
}

/**
 * Hash every file a row names, in one directory.
 *
 * @param dir - The directory holding the archived build.
 * @param row - The row naming the files.
 * @returns Each file's digest, or null where the file is absent.
 */
async function digestDirectory(
  dir: string,
  row: BuildDigestRecord,
): Promise<Record<string, FileDigest | null>> {
  const actual: Record<string, FileDigest | null> = {};
  for (const fileName of Object.keys(row.files)) {
    const path = join(dir, fileName);
    actual[fileName] = (await Bun.file(path).exists())
      ? await digestFile(path)
      : null;
  }
  return actual;
}

/**
 * ///////////////////////////////////////////////
 * Output
 * ///////////////////////////////////////////////
 */

/** Dim text, when the terminal takes color. */
function dim(text: string): string {
  return Bun.enableANSIColors ? `[2m${text}[0m` : text;
}

/** Print one result row. */
function row(ok: boolean, text: string): void {
  console.log(`  ${ok ? "✓" : "✗"} ${text}`);
}

/** Print the section label every command opens with. */
function section(name: string): void {
  console.log(`archive ${name}`);
  console.log("");
}

/** A count with its noun, so a summary line reads as English at one. */
function count(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** Print the closing line and pick the exit code. */
function summary(text: string, failed: boolean): never {
  console.log("");
  console.log(`  ${text}`);
  process.exit(failed ? 1 : 0);
}

/**
 * ///////////////////////////////////////////////
 * Commands
 * ///////////////////////////////////////////////
 */

/** Hash a local build and append its row to the digest record. */
async function commandRecord(options: Options): Promise<never> {
  section("record");
  const dir = required(options, "dir");
  const manifestId = required(options, "manifest");
  if ((await digestRow(manifestId)) !== null) {
    row(false, `${manifestId} already has a row`);
    summary(
      "a digest row is written once, because rewriting one would let a swap " +
        "be blessed",
      true,
    );
  }

  const appId = Number(options.app ?? APP_ID);
  const depotId = Number(options.depot ?? DEPOT_ID);

  // What the fetch says it got, against what it was asked for. A fetch that
  // cannot pin a manifest returns the head of the branch, so a Keen release
  // landing mid-run would otherwise be filed under the previous gid and the
  // row would describe a build that is not there.
  const fetched = await fetchedManifest(dir, appId, depotId);
  if (fetched === null) {
    row(true, dim(`${dir} carries no record of which manifest it came from`));
  } else if (fetched.manifestId !== manifestId) {
    row(
      false,
      `${fetched.source} says this build came from manifest ` +
        `${fetched.manifestId}, and the row would say ${manifestId}`,
    );
    summary(
      "nothing recorded. The bytes are a different build than the one asked " +
        "for, so the row would key them under the wrong manifest",
      true,
    );
  } else {
    row(true, `${manifestId} confirmed by ${dim(fetched.source)}`);
  }

  const files: Record<string, FileDigest> = {};
  for (const fileName of options.file ?? ARCHIVE_FILES) {
    // Checked before the file is opened. Validating it afterwards would print
    // the size and digest of whatever path was typed, which for `../secret`
    // is a hash oracle over the filesystem.
    if (!isArchiveFileName(fileName)) {
      row(false, `${fileName} is not one plain archived file name`);
      summary("nothing recorded", true);
    }
    const path = join(dir, fileName);
    if (!(await Bun.file(path).exists())) {
      row(false, `${fileName} is not in ${dir}`);
      summary("nothing recorded", true);
    }
    const digest = await digestFile(path);
    files[fileName] = digest;
    row(true, `${fileName} ${digest.bytes} bytes ${dim(digest.sha256)}`);
  }

  await appendRecord(BUILD_DIGESTS_PATH, BuildDigestRecord, {
    manifestId,
    buildId: options.build ?? null,
    appId,
    depotId,
    revision: options.revision === undefined ? null : Number(options.revision),
    branch: options.branch ?? null,
    archivedAt: (options.date ?? new Date().toISOString()).slice(0, 10),
    files,
  });
  summary(`one row appended to ${BUILD_DIGESTS_PATH}`, false);
}

/** Hash a local build and compare it to its committed row. */
async function commandVerify(options: Options): Promise<never> {
  section("verify");
  const dir = required(options, "dir");
  const manifestId = required(options, "manifest");
  const record = await digestRow(manifestId);
  if (record === null) {
    row(false, `${manifestId} has no row in ${BUILD_DIGESTS_PATH}`);
    summary("nothing to verify against, so the bytes are unproven", true);
  }

  const problems = compareDigests(
    await digestDirectory(dir, record),
    record.files,
  );
  for (const fileName of Object.keys(record.files)) {
    const failures = problems.filter(
      (problem) => problem.fileName === fileName,
    );
    if (failures.length === 0) {
      row(true, `${fileName} matches`);
      continue;
    }
    for (const failure of failures) {
      row(false, `${fileName} ${failure.detail}`);
    }
  }
  // One file can fail on both its size and its digest, so the summary counts
  // files rather than problems.
  const failed = new Set(problems.map((problem) => problem.fileName)).size;
  summary(
    failed === 0
      ? `${count(Object.keys(record.files).length, "file matches", "files match")} build ${manifestId}`
      : `${count(failed, "file does not match", "files do not match")} build ${manifestId}`,
    failed > 0,
  );
}

/** Verify a local build, then upload it under its manifest gid. */
async function commandPush(options: Options): Promise<never> {
  section("push");
  const dir = required(options, "dir");
  const manifestId = required(options, "manifest");
  const record = await digestRow(manifestId);
  if (record === null) {
    row(false, `${manifestId} has no row in ${BUILD_DIGESTS_PATH}`);
    summary(
      "run `archive record` first, so the upload has a digest to prove",
      true,
    );
  }

  const problems = compareDigests(
    await digestDirectory(dir, record),
    record.files,
  );
  if (problems.length > 0) {
    for (const problem of problems) {
      row(false, `${problem.fileName} ${problem.detail}`);
    }
    summary(
      "nothing uploaded, because the local copy is not the recorded one",
      true,
    );
  }

  // One key at a time, checked and written before the next is considered. A
  // run that dies between two files leaves the ones it wrote in place, and the
  // next run skips those and uploads the rest, so a half-finished archive
  // repairs itself rather than blocking the build forever.
  const bucket = client("write");
  let uploaded = 0;
  for (const fileName of Object.keys(record.files)) {
    const key = objectKey(manifestId, fileName);
    const existing = await bucket
      .file(key)
      .stat()
      .catch(() => null);
    if (existing !== null) {
      // The digests match the local copy, which was checked above, so a size
      // match means the object already holds the recorded bytes.
      if (existing.size === record.files[fileName]?.bytes) {
        row(true, `${key} is already archived`);
        continue;
      }
      row(
        false,
        `${key} holds ${existing.size} bytes, and the record says ${record.files[fileName]?.bytes}`,
      );
      summary(
        "nothing further uploaded. R2 has no versioning, so an overwrite is " +
          "final and this tool never takes that decision on its own",
        true,
      );
    }
    await bucket.write(key, Bun.file(join(dir, fileName)));
    row(true, `${key} uploaded`);
    uploaded += 1;
  }
  summary(
    uploaded === 0
      ? `build ${manifestId} was already archived`
      : `build ${manifestId} archived, ${count(uploaded, "file", "files")} uploaded`,
    false,
  );
}

/** Download a build, verify what arrived, and only then give it its name. */
async function commandPull(options: Options): Promise<never> {
  section("pull");
  const manifestId = required(options, "manifest");
  const out = required(options, "out");
  const record = await digestRow(manifestId);
  if (record === null) {
    row(false, `${manifestId} has no row in ${BUILD_DIGESTS_PATH}`);
    summary(
      "refusing to download bytes with nothing to check them against",
      true,
    );
  }

  // An output directory that already holds something is refused rather than
  // emptied. This command is pointed at `.cache/archive/<gid>`, so replacing
  // what is there is never something it decides on its own.
  const occupants = await readdir(out).catch(() => [] as string[]);
  if (occupants.length > 0) {
    row(
      false,
      `${out} already holds ${count(occupants.length, "file", "files")}`,
    );
    summary("refusing to replace a directory this command did not fill", true);
  }

  // The credential is read before anything is created, so a run without one
  // leaves the filesystem exactly as it found it.
  const bucket = client("read");

  // Every file lands in a staging directory beside the output, and the whole
  // directory is renamed once. A crash part way through leaves the output
  // directory as it was, rather than holding one file of this build and one of
  // whatever was there before.
  const staging = `${out}.partial`;
  await rm(staging, { recursive: true, force: true });
  await mkdir(staging, { recursive: true });
  const actual: Record<string, FileDigest | null> = {};
  for (const fileName of Object.keys(record.files)) {
    const expected = record.files[fileName] as FileDigest;
    const key = objectKey(manifestId, fileName);
    // The object's own size is checked before a byte is streamed. Comparing
    // digests afterwards catches the same object, but only once it is on the
    // disk, so an object far larger than the record says would fill the disk
    // before anything refused it.
    const stat = await bucket
      .file(key)
      .stat()
      .catch(() => null);
    if (stat === null || stat.size !== expected.bytes) {
      await rm(staging, { recursive: true, force: true });
      row(
        false,
        stat === null
          ? `${key} is not in the bucket`
          : `${key} advertises ${stat.size} bytes, and the record says ${expected.bytes}`,
      );
      summary("nothing was downloaded", true);
    }
    const partial = join(staging, fileName);
    await Bun.write(partial, bucket.file(key));
    actual[fileName] = await digestFile(partial);
  }

  const problems = compareDigests(actual, record.files);
  if (problems.length > 0) {
    await rm(staging, { recursive: true, force: true });
    for (const problem of problems) {
      row(false, `${problem.fileName} ${problem.detail}`);
    }
    summary(
      "the archive does not hold what the record says it holds, so nothing " +
        "was written",
      true,
    );
  }

  // The output directory is either absent or the empty one checked above, so
  // removing it costs nothing and lets the staging directory take its name in
  // one operation.
  await rm(out, { recursive: true, force: true });
  await rename(staging, out);
  for (const fileName of Object.keys(record.files)) {
    row(true, `${fileName} verified`);
  }
  summary(`build ${manifestId} is in ${out}`, false);
}

/**
 * ///////////////////////////////////////////////
 * Command line
 * ///////////////////////////////////////////////
 */

/** Every flag, as `parseArgs` hands them over. */
interface Options {
  readonly dir?: string;
  readonly out?: string;
  readonly manifest?: string;
  readonly build?: string;
  readonly app?: string;
  readonly depot?: string;
  readonly revision?: string;
  readonly branch?: string;
  readonly date?: string;
  readonly file?: string[];
}

/** The flag a command cannot run without, or a usage failure naming it. */
function required(options: Options, name: "dir" | "out" | "manifest"): string {
  const value = options[name];
  if (value === undefined || value.length === 0) {
    console.error(`--${name} is required`);
    process.exit(2);
  }
  return value;
}

/** What each verb does, for the usage text. */
const VERBS: Record<string, string> = {
  record: "hash a local build and append its digest row",
  verify: "check a local build against its digest row",
  push: "verify a local build, then upload it to the archive",
  pull: "download a build and verify it before it is named",
};

if (import.meta.main) {
  const { values, positionals } = parseArgs({
    args: Bun.argv.slice(2),
    allowPositionals: true,
    options: {
      dir: { type: "string" },
      out: { type: "string" },
      manifest: { type: "string" },
      build: { type: "string" },
      app: { type: "string" },
      depot: { type: "string" },
      revision: { type: "string" },
      branch: { type: "string" },
      date: { type: "string" },
      file: { type: "string", multiple: true },
    },
  });
  const verb = positionals[0];
  const options = values as Options;

  if (verb === undefined || !(verb in VERBS)) {
    console.error(`usage: ${basename(import.meta.file)} <verb> [flags]`);
    for (const [name, what] of Object.entries(VERBS)) {
      console.error(`  ${name}  ${what}`);
    }
    process.exit(2);
  }

  try {
    if (verb === "record") await commandRecord(options);
    if (verb === "verify") await commandVerify(options);
    if (verb === "push") await commandPush(options);
    if (verb === "pull") await commandPull(options);
  } catch (error) {
    // A refusal this tool raises on purpose prints as a result row. Anything
    // else is a defect and keeps its stack, because a stack is what a defect
    // needs and a refusal is not helped by one.
    if (
      error instanceof MissingCredentialError ||
      error instanceof DuplicateRowError ||
      error instanceof RecordError
    ) {
      row(false, error.message);
      summary(
        error instanceof MissingCredentialError
          ? "no archive was reached"
          : `${BUILD_DIGESTS_PATH} has to be repaired before this can run`,
        true,
      );
    }
    throw error;
  }
}
