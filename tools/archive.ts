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
 * Whether a build is archived is asked of the bucket and never read from the
 * digest record. `status` asks with the read token every hour, and its answer
 * is what decides whether the archive job runs at all. It sweeps every other
 * recorded build in the same run, because the archive job can refetch the head
 * of the branch and nothing else.
 *
 * `push` skips a key that already holds the recorded bytes and refuses one
 * that holds anything else, and that refusal is advisory rather than atomic.
 * R2 honors `If-None-Match: *` on a put, and Bun 1.3.13's S3 client exposes no
 * way to send it, so the check and the write are two requests. Two writers
 * racing the same key is the uncovered case, and `concurrency: build-watch` is
 * what keeps the scheduled job from being one of them.
 *
 * @example
 * ```sh
 * bun run tools/archive.ts verify --dir .cache/archive/2174935030716737236 \
 *   --manifest 2174935030716737236
 * ```
 */

import { readFileSync } from "node:fs";
import { appendFile, mkdir, readdir, rename, rm } from "node:fs/promises";
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
 * The version line the build carries
 * ///////////////////////////////////////////////
 */

/** What a build's resource container says about the content it was cut from. */
export interface KfcVersion {
  /** The content revision, which the supported-build table matches on. */
  readonly revision: number;
  /** The Subversion branch path the content came from. */
  readonly branch: string;
}

/** The ASCII bytes `KFC3`, which open a Keen resource container. */
const KFC_MAGIC = "KFC3";

/** Where the container's location records start. */
const KFC_LOCATIONS_AT = 0x10;

/** The most bytes a version line is read from, which is far more than it needs. */
const KFC_VERSION_CAP = 512;

/**
 * Read the version line out of `enshrouded_server.kfc`.
 *
 * @remarks
 * The container opens with `KFC3` and a table of location records, each a
 * relative offset and a count. The first record points at the version line,
 * which is the content revision, the branch path and a timestamp joined by
 * `|`. `ember-kfc` reads the same two fields the same way, and this is the
 * only reader on this side of the pipeline.
 *
 * Reading it here is what fills `revision` and `branch` on a row continuous
 * integration writes. They name the content a build was cut from, which is
 * what the supported-build table matches on, and no Steam field carries
 * either.
 *
 * @param path - The container to read.
 * @returns The revision and the branch, or null when the file is not a
 * container, does not use the slot, or does not carry the two fields.
 */
export async function kfcVersion(path: string): Promise<KfcVersion | null> {
  const file = Bun.file(path);
  const header = new Uint8Array(
    await file.slice(0, KFC_LOCATIONS_AT + 8).arrayBuffer(),
  );
  if (header.byteLength < KFC_LOCATIONS_AT + 8) {
    return null;
  }
  if (new TextDecoder().decode(header.slice(0, 4)) !== KFC_MAGIC) {
    return null;
  }
  const record = new DataView(
    header.buffer,
    header.byteOffset + KFC_LOCATIONS_AT,
    8,
  );
  // A relative offset of zero marks a location the build does not use, and the
  // offset is relative to the record that holds it.
  const relative = record.getUint32(0, true);
  const count = record.getUint32(4, true);
  if (relative === 0 || count === 0 || count > KFC_VERSION_CAP) {
    return null;
  }
  const at = KFC_LOCATIONS_AT + relative;
  const line = await file.slice(at, at + count).text();
  const parsed = /^(\d{1,9})\|(\^\/[A-Za-z0-9._/-]+)\|/.exec(line);
  if (parsed === null) {
    return null;
  }
  return { revision: Number(parsed[1]), branch: parsed[2] as string };
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
 * Asking the bucket
 * ///////////////////////////////////////////////
 */

/** Raised when the bucket answers with neither an object nor its absence. */
export class ArchiveUnreachableError extends Error {
  constructor(key: string, cause: unknown) {
    const code = (cause as { code?: unknown } | null)?.code;
    super(
      `${key} could not be checked in the archive` +
        (typeof code === "string" ? `, which answered ${code}` : "") +
        ". A token without read access, a revoked token and an R2 outage " +
        "all answer this way, so nothing was decided from it.",
    );
    this.name = "ArchiveUnreachableError";
  }
}

/**
 * The size of one object, or null when the bucket says the key is absent.
 *
 * @remarks
 * Only a missing key reads as null. Bun reports that as `NoSuchKey` and a
 * refused or failed request as something else, and anything else is thrown.
 * Read as absent, a token that lost its read access would make every build
 * look unarchived, and `push` would take that as leave to write over a key it
 * never looked inside.
 *
 * @param bucket - The client to ask. A test points one at a loopback server.
 * @param key - The object key.
 * @returns The object's size in bytes, or null when the key is absent.
 * @throws {@link ArchiveUnreachableError} When the bucket answered anything
 * other than the object or its absence.
 */
export async function objectSize(
  bucket: Bun.S3Client,
  key: string,
): Promise<number | null> {
  try {
    return (await bucket.file(key).stat()).size;
  } catch (error) {
    if ((error as { code?: unknown } | null)?.code === "NoSuchKey") {
      return null;
    }
    throw new ArchiveUnreachableError(key, error);
  }
}

/**
 * What the bucket holds for one build.
 *
 * @remarks
 * A build with no row is asked about under {@link ARCHIVE_FILES}, so every run
 * sends a request whatever the record says. The answer for such a build is
 * already settled, and asking is what exercises the credential: a token that
 * was revoked or scoped wrong is found in the hour it happens rather than at
 * the next upload.
 *
 * @param bucket - The client to ask. A test points one at a loopback server.
 * @param manifestId - The build to ask about.
 * @param record - The row naming the files, or null for a build with none.
 * @returns Each file's size in the bucket, keyed by file name, with null for
 * an absent object.
 * @throws {@link ArchiveUnreachableError} When the bucket could not be asked
 * about one of them.
 */
export async function bucketSizes(
  bucket: Bun.S3Client,
  manifestId: string,
  record: BuildDigestRecord | null,
): Promise<Record<string, number | null>> {
  const names = record === null ? ARCHIVE_FILES : Object.keys(record.files);
  const sizes: Record<string, number | null> = {};
  for (const fileName of names) {
    sizes[fileName] = await objectSize(bucket, objectKey(manifestId, fileName));
  }
  return sizes;
}

/** Whether the archive job has work to do for one build. */
export type ArchiveState =
  | { readonly state: "archived" }
  | { readonly state: "missing"; readonly detail: string }
  | { readonly state: "conflict"; readonly detail: string };

/**
 * Decide whether one build is archived, from its row and what the bucket holds.
 *
 * @remarks
 * A build is archived when it has a digest row and every file the row names
 * is in the bucket at the recorded size. It takes both halves. A row with no
 * objects is a build that was hashed and never uploaded. Objects with no row
 * are an upload whose row never reached the record, and `pull` refuses a build
 * it has nothing to check against. The archive job finishes either one by
 * running again.
 *
 * An object at a size the row does not state is a conflict rather than work.
 * `push` refuses to write over it, so running the archive job cannot settle
 * it, and it needs a person.
 *
 * Presence and size are all this reads. The bytes are checked by `pull`, which
 * hashes everything it fetches, and by `push`, which hashes an object before
 * it calls one archived.
 *
 * @param manifestId - The build's depot manifest gid.
 * @param record - The build's digest row, or null when it has none.
 * @param sizes - What the bucket reports for each file the row names, keyed by
 * file name, with null for an absent object.
 * @returns Archived, missing with the reason, or a conflict with the reason.
 */
export function archiveState(
  manifestId: string,
  record: BuildDigestRecord | null,
  sizes: Readonly<Record<string, number | null>>,
): ArchiveState {
  if (record === null) {
    return {
      state: "missing",
      detail: `${manifestId} has no digest row, so the build was never recorded`,
    };
  }
  const absent: string[] = [];
  const conflicts: string[] = [];
  for (const [fileName, want] of Object.entries(record.files)) {
    const key = objectKey(manifestId, fileName);
    const got = Object.hasOwn(sizes, fileName)
      ? (sizes[fileName] ?? null)
      : null;
    if (got === null) {
      absent.push(key);
    } else if (got !== want.bytes) {
      conflicts.push(
        `${key} holds ${got} bytes, and the record says ${want.bytes}`,
      );
    }
  }
  if (conflicts.length > 0) {
    return { state: "conflict", detail: conflicts.join("; ") };
  }
  if (absent.length > 0) {
    return {
      state: "missing",
      detail: `${absent.join(" and ")} ${absent.length === 1 ? "is" : "are"} not in the bucket`,
    };
  }
  return { state: "archived" };
}

/**
 * ///////////////////////////////////////////////
 * Digests
 * ///////////////////////////////////////////////
 */

/**
 * Hash a stream of bytes without holding it in memory.
 *
 * @param stream - The bytes, from a file on disk or an object in the bucket.
 * @returns Their count and their lowercase hex SHA-256.
 */
export async function digestStream(
  stream: ReadableStream<Uint8Array>,
): Promise<FileDigest> {
  const hasher = new Bun.CryptoHasher("sha256");
  let bytes = 0;
  for await (const chunk of stream) {
    hasher.update(chunk);
    bytes += chunk.byteLength;
  }
  return { bytes, sha256: hasher.digest("hex") };
}

/**
 * Hash one file without holding it in memory.
 *
 * @param path - The file to read.
 * @returns Its size in bytes and its lowercase hex SHA-256.
 */
export async function digestFile(path: string): Promise<FileDigest> {
  return digestStream(Bun.file(path).stream());
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

/**
 * Hand one value to the workflow step that will read it, when one is listening.
 *
 * @remarks
 * A line break in a value would declare an output name of its own, and GitHub
 * takes the last value for a repeated name. `JSON.stringify` never emits a raw
 * line break, so this refuses rather than escaping: a value that has one is a
 * value this code did not build.
 *
 * @param name - The output name a later step reads.
 * @param value - What to hand it.
 */
async function emitStepOutput(name: string, value: string): Promise<void> {
  const path = process.env["GITHUB_OUTPUT"];
  if (path === undefined || path.length === 0) {
    return;
  }
  if (/[\r\n]/.test(value)) {
    row(false, `the ${name} output carries a line break`);
    summary("nothing was handed to the next job", true);
  }
  await appendFile(path, `${name}=${value}\n`, "utf8");
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

/**
 * Refuse a directory whose fetch says it holds another build than the one named.
 *
 * @remarks
 * A fetch that cannot pin a manifest returns the head of the branch, so a Keen
 * release landing mid-run would otherwise be filed under the previous gid, and
 * the row would describe a build that is not there.
 *
 * A directory carrying no evidence at all is refused rather than trusted.
 * Both fetching routes leave some, so bytes without any arrived some other
 * way: a restore from backup, a copy that skipped hidden entries, an unpack
 * that dropped them. Recording those keys them to whatever gid was typed, in a
 * bucket with no versioning, and every later check compares them against that
 * same row. `--unattested` records them anyway, and puts the assumption in the
 * command that took it.
 *
 * @param dir - The directory the fetch filled.
 * @param manifestId - The manifest the caller asked for.
 * @param appId - The Steam application that was fetched.
 * @param depotId - The depot whose manifest is compared.
 * @param unattested - Whether the caller accepts a directory with no evidence.
 */
async function confirmProvenance(
  dir: string,
  manifestId: string,
  appId: number,
  depotId: number,
  unattested: boolean,
): Promise<void> {
  const fetched = await fetchedManifest(dir, appId, depotId);
  if (fetched === null) {
    if (!unattested) {
      row(false, `${dir} carries no record of which manifest it came from`);
      summary(
        "nothing recorded. SteamCMD leaves an app manifest and " +
          "DepotDownloader leaves a cached one, so bytes with neither are " +
          "attributed by whoever typed the gid. Pass --unattested to record " +
          "them on that basis",
        true,
      );
    }
    row(
      true,
      dim(`${manifestId} is the caller's word: ${dir} carries no fetch record`),
    );
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
}

/**
 * Hash a local build and build the row that describes it.
 *
 * @remarks
 * `record` appends what this returns and `emit` prints it, so the checks that
 * decide whether a build may be recorded at all happen once and in one place
 * rather than in whichever verb a caller happened to use.
 *
 * @param options - The flags the command was given.
 * @param recordPath - The record to check for an existing row.
 * @returns The row, after every check it has to pass.
 */
async function buildRow(
  options: Options,
  recordPath: string,
): Promise<BuildDigestRecord> {
  const dir = required(options, "dir");
  const manifestId = required(options, "manifest");
  if ((await digestRow(manifestId, recordPath)) !== null) {
    row(false, `${manifestId} already has a row`);
    summary(
      "a digest row is written once, because rewriting one would let a swap " +
        "be blessed",
      true,
    );
  }

  const appId = Number(options.app ?? APP_ID);
  const depotId = Number(options.depot ?? DEPOT_ID);
  await confirmProvenance(
    dir,
    manifestId,
    appId,
    depotId,
    options.unattested === true,
  );

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

  // The container names the content the build was cut from, and no Steam field
  // does. A flag still wins, because a caller reading a build this tool cannot
  // parse has to be able to say what it is.
  const version = await kfcVersion(join(dir, "enshrouded_server.kfc"));
  if (version === null) {
    row(true, dim("enshrouded_server.kfc carries no version line"));
  } else {
    row(true, `revision ${version.revision} on ${dim(version.branch)}`);
  }

  return BuildDigestRecord.parse({
    manifestId,
    buildId: options.build ?? null,
    appId,
    depotId,
    revision:
      options.revision === undefined
        ? (version?.revision ?? null)
        : Number(options.revision),
    branch: options.branch ?? version?.branch ?? null,
    recordedAt: (options.date ?? new Date().toISOString()).slice(0, 10),
    files,
  });
}

/**
 * Check a fetched build against the row already committed for it.
 *
 * @remarks
 * A build can be recorded before it is archived: hashed from a local copy, or
 * recorded by a run whose upload then failed. The archive job fetches such a
 * build again, and once the fetched bytes match the committed row, that row is
 * what it hands on, unchanged.
 *
 * Bytes that do not match are refused rather than recorded again. The row is
 * the integrity record, and a fetch that disagrees with it is either a swapped
 * build or a wrong row, which a person settles.
 *
 * @param options - The flags the command was given.
 * @param dir - The directory the fetch filled.
 * @param record - The committed row for the build that was asked for.
 * @returns The committed row, once every file it names matches.
 */
async function confirmRow(
  options: Options,
  dir: string,
  record: BuildDigestRecord,
): Promise<BuildDigestRecord> {
  await confirmProvenance(
    dir,
    record.manifestId,
    record.appId,
    record.depotId,
    options.unattested === true,
  );
  const problems = compareDigests(
    await digestDirectory(dir, record),
    record.files,
  );
  if (problems.length > 0) {
    for (const problem of problems) {
      row(false, `${problem.fileName} ${problem.detail}`);
    }
    summary(
      "nothing handed on. The fetched bytes are not the ones the committed " +
        `row pins for ${record.manifestId}`,
      true,
    );
  }
  for (const fileName of Object.keys(record.files)) {
    row(true, `${fileName} matches the committed row`);
  }
  return record;
}

/** Hash a local build and append its row to the digest record. */
async function commandRecord(options: Options): Promise<never> {
  section("record");
  const recordPath = options.record ?? BUILD_DIGESTS_PATH;
  const built = await buildRow(options, recordPath);
  await appendRecord(recordPath, BuildDigestRecord, built);
  summary(`one row appended to ${recordPath}`, false);
}

/**
 * Hash a local build and print its row, without touching the record.
 *
 * @remarks
 * A separate verb rather than a flag on `record`, because it is a different
 * seam. `--record` moves where the append lands and leaves `record` meaning
 * what it says. This changes whether an append happens at all, and a verb that
 * sometimes writes and sometimes does not is the kind of thing a reader has to
 * check the flags to understand.
 *
 * The job that fetches a build holds the credential that can overwrite the
 * archive, and the job that commits the row holds a key that can push to main.
 * Neither should hold the other, so the row crosses between them as a job
 * output and this is what puts it there.
 *
 * A build that already has a row gets that row back, once the fetched bytes
 * match it. The archive job runs for any build the bucket lacks, recorded or
 * not, and a recorded one is uploaded under the digests its row already pins.
 */
async function commandEmit(options: Options): Promise<never> {
  section("emit");
  const recordPath = options.record ?? BUILD_DIGESTS_PATH;
  const dir = required(options, "dir");
  const committed = await digestRow(required(options, "manifest"), recordPath);
  const built =
    committed === null
      ? await buildRow(options, recordPath)
      : await confirmRow(options, dir, committed);
  const line = JSON.stringify(built);

  // Every field here is committed to a public repository moments later, so
  // none of it is secret. A job output is readable by anyone who can read the
  // run, which for this row is the same audience as the file.
  await emitStepOutput("row", line);
  console.log("");
  console.log(line);
  summary(`the row for ${built.manifestId} was not written to any file`, false);
}

/**
 * Append a row that another job built.
 *
 * @remarks
 * The row arrives as a string from outside this process, so it is validated
 * against the same shape a read enforces, and checked for a duplicate again.
 * The record may have gained that manifest between the job that built the row
 * and this one.
 */
async function commandAppend(options: Options): Promise<never> {
  section("append");
  const recordPath = options.record ?? BUILD_DIGESTS_PATH;
  const raw = required(options, "row");

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    row(false, `--row is not JSON: ${(error as Error).message}`);
    summary("nothing appended", true);
  }
  const checked = BuildDigestRecord.safeParse(parsed);
  if (!checked.success) {
    row(false, `--row is not a digest row: ${z.prettifyError(checked.error)}`);
    summary("nothing appended", true);
  }

  if ((await digestRow(checked.data.manifestId, recordPath)) !== null) {
    row(true, `${checked.data.manifestId} already has a row`);
    summary(
      "nothing appended, and nothing is wrong: the row this job was handed " +
        "is already in the record",
      false,
    );
  }

  await appendRecord(recordPath, BuildDigestRecord, checked.data);
  for (const [fileName, digest] of Object.entries(checked.data.files)) {
    row(true, `${fileName} ${digest.bytes} bytes ${dim(digest.sha256)}`);
  }
  summary(`one row appended to ${recordPath}`, false);
}

/** Hash a local build and compare it to its committed row. */
async function commandVerify(options: Options): Promise<never> {
  section("verify");
  const recordPath = options.record ?? BUILD_DIGESTS_PATH;
  const dir = required(options, "dir");
  const manifestId = required(options, "manifest");
  const record = await digestRow(manifestId, recordPath);
  if (record === null) {
    row(false, `${manifestId} has no row in ${recordPath}`);
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

/** The step output the archive job's gate reads. */
export const NEEDS_ARCHIVE = "needs_archive";

/** What one recorded build looks like in the bucket. */
export interface ArchiveSweep {
  /** The build's depot manifest gid. */
  readonly manifestId: string;
  /** What the bucket holds for it. */
  readonly state: ArchiveState;
}

/**
 * The state of every recorded build except one.
 *
 * @remarks
 * The archive job can only ever fetch the head of the branch, so it repairs
 * the current build and no other. The rest cannot be refetched from Steam at
 * all, which makes them the builds whose loss is absolute and the ones worth
 * looking at every hour.
 *
 * @param bucket - The client to ask.
 * @param records - Every row in the digest record.
 * @param except - The build the caller already asked about.
 * @returns One entry per other recorded build, in record order.
 * @throws {@link ArchiveUnreachableError} When the bucket could not be asked.
 */
export async function sweepArchive(
  bucket: Bun.S3Client,
  records: readonly BuildDigestRecord[],
  except: string,
): Promise<ArchiveSweep[]> {
  const swept: ArchiveSweep[] = [];
  for (const record of records) {
    if (record.manifestId === except) {
      continue;
    }
    const sizes = await bucketSizes(bucket, record.manifestId, record);
    swept.push({
      manifestId: record.manifestId,
      state: archiveState(record.manifestId, record, sizes),
    });
  }
  return swept;
}

/**
 * Ask the bucket whether one build is archived, and tell the workflow.
 *
 * @remarks
 * The hourly gate on the archive job. It holds the read token and nothing
 * else, so it answers without waiting on the approval the write token sits
 * behind, and the archive job asks for that approval only when there is work.
 *
 * It hands on `needs_archive=true` for a build with work left and
 * `needs_archive=false` for one that is archived. It fails on a conflict, and
 * on a bucket it could not ask, and hands on nothing either way: a check that
 * could not decide must not read as a decision.
 *
 * The answer is handed on before the rest of the record is swept, so a
 * historical build that has gone missing turns this job red without stopping
 * the one build the archive job can still repair.
 *
 * @param options - The flags the command was given.
 * @param bucket - The client to ask, which defaults to the read token. A test
 * passes one pointed at a loopback server, the same seam {@link bucketSizes}
 * takes.
 */
export async function commandStatus(
  options: Options,
  bucket: Bun.S3Client = client("read"),
): Promise<never> {
  section("status");
  const recordPath = options.record ?? BUILD_DIGESTS_PATH;
  const manifestId = required(options, "manifest");
  const record = await digestRow(manifestId, recordPath);

  const sizes = await bucketSizes(bucket, manifestId, record);
  if (record === null) {
    row(false, `${manifestId} has no row in ${recordPath}`);
  }
  for (const [fileName, size] of Object.entries(sizes)) {
    const key = objectKey(manifestId, fileName);
    const want = record?.files[fileName];
    if (size === null) {
      row(false, `${key} is not in the bucket`);
    } else if (want !== undefined && size !== want.bytes) {
      row(
        false,
        `${key} holds ${size} bytes, and the record says ${want.bytes}`,
      );
    } else {
      row(true, `${key} ${size} bytes`);
    }
  }

  const decided = archiveState(manifestId, record, sizes);
  if (decided.state === "conflict") {
    summary(
      "the bucket holds bytes the record does not describe. push refuses to " +
        "write over them, so this needs a person rather than another run",
      true,
    );
  }
  const needsArchive = decided.state === "missing";
  row(
    !needsArchive,
    needsArchive
      ? `${manifestId} needs archiving: ${decided.detail}`
      : `${manifestId} is archived`,
  );
  await emitStepOutput(NEEDS_ARCHIVE, String(needsArchive));

  const swept = await sweepArchive(
    bucket,
    await readRecords(recordPath, BuildDigestRecord),
    manifestId,
  );
  for (const other of swept) {
    if (other.state.state === "archived") {
      row(true, dim(`${other.manifestId} is archived`));
    } else {
      row(false, `${other.manifestId} ${other.state.detail}`);
    }
  }

  const lost = swept.filter((other) => other.state.state !== "archived");
  const answer = needsArchive
    ? `build ${manifestId} needs archiving`
    : `build ${manifestId} is archived`;
  summary(
    lost.length === 0
      ? `${answer}, and every other recorded build is in the archive`
      : `${answer}, and ${count(lost.length, "other recorded build is", "other recorded builds are")} not. Steam serves none of them any more`,
    lost.length > 0,
  );
}

/** What `push` does about one key. */
export type UploadAction =
  | { readonly action: "upload" }
  | { readonly action: "skip" }
  | { readonly action: "refuse"; readonly detail: string };

/**
 * Decide what to do about one key, from what the bucket already holds there.
 *
 * @remarks
 * This is what makes a retry converge rather than loop. A run that uploaded
 * and then failed before its row was committed leaves the build unrecorded, so
 * the next run fetches and pushes again, and so does a second run of the
 * one-time seed. Skipping an object that already holds the recorded bytes is
 * what stops that second push from either refusing or overwriting. The bytes
 * are the irreplaceable half and they are already safe, so the retry costs one
 * fetch and finishes.
 *
 * An object is skipped only once it hashes to the record. A matching size
 * says nothing about the bytes, and an object skipped on its size alone would
 * be reported archived while holding something else.
 *
 * Anything else already at the key is refused rather than overwritten,
 * because R2 has no versioning and the object under a legitimate key is what
 * continuous integration and developers run.
 *
 * @param expected - What the digest row says the file is.
 * @param existingBytes - The size the bucket reports, or null when absent.
 * @param existingSha256 - Hashes the object in the bucket. It is called only
 * when the size already matches, because it downloads the object.
 * @returns Whether to upload, skip, or refuse and why.
 */
export async function uploadDecision(
  expected: FileDigest,
  existingBytes: number | null,
  existingSha256: () => Promise<string>,
): Promise<UploadAction> {
  if (existingBytes === null) {
    return { action: "upload" };
  }
  if (existingBytes !== expected.bytes) {
    return {
      action: "refuse",
      detail: `holds ${existingBytes} bytes, and the record says ${expected.bytes}`,
    };
  }
  const sha256 = await existingSha256();
  if (sha256 !== expected.sha256) {
    return {
      action: "refuse",
      detail: `hashes to ${sha256}, and the record says ${expected.sha256}`,
    };
  }
  return { action: "skip" };
}

/** Verify a local build, then upload it under its manifest gid. */
async function commandPush(options: Options): Promise<never> {
  section("push");
  const recordPath = options.record ?? BUILD_DIGESTS_PATH;
  const dir = required(options, "dir");
  const manifestId = required(options, "manifest");
  const record = await digestRow(manifestId, recordPath);
  if (record === null) {
    row(false, `${manifestId} has no row in ${recordPath}`);
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
    const decided = await uploadDecision(
      record.files[fileName] as FileDigest,
      await objectSize(bucket, key),
      async () => (await digestStream(bucket.file(key).stream())).sha256,
    );
    if (decided.action === "skip") {
      row(true, `${key} already holds the recorded bytes`);
      continue;
    }
    if (decided.action === "refuse") {
      row(false, `${key} ${decided.detail}`);
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
  const recordPath = options.record ?? BUILD_DIGESTS_PATH;
  const manifestId = required(options, "manifest");
  const out = required(options, "out");
  const record = await digestRow(manifestId, recordPath);
  if (record === null) {
    row(false, `${manifestId} has no row in ${recordPath}`);
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

  // Every object's own size is checked before a byte is streamed or a
  // directory is made. Comparing digests afterwards catches the same object,
  // but only once it is on the disk, so an object far larger than the record
  // says would fill the disk before anything refused it.
  for (const fileName of Object.keys(record.files)) {
    const expected = record.files[fileName] as FileDigest;
    const key = objectKey(manifestId, fileName);
    const size = await objectSize(bucket, key);
    if (size === null || size !== expected.bytes) {
      row(
        false,
        size === null
          ? `${key} is not in the bucket`
          : `${key} advertises ${size} bytes, and the record says ${expected.bytes}`,
      );
      summary("nothing was downloaded", true);
    }
  }

  // Every file lands in a staging directory beside the output, and the whole
  // directory is renamed once. A crash part way through leaves the output
  // directory as it was, rather than holding one file of this build and one of
  // whatever was there before.
  const staging = `${out}.partial`;
  await rm(staging, { recursive: true, force: true });
  await mkdir(staging, { recursive: true });
  const actual: Record<string, FileDigest | null> = {};
  for (const fileName of Object.keys(record.files)) {
    const partial = join(staging, fileName);
    await Bun.write(partial, bucket.file(objectKey(manifestId, fileName)));
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
  /**
   * The JSON Lines record to read and append to.
   *
   * @remarks
   * Defaults to the committed one. A test points it at a sandbox so a case can
   * drive the whole command without appending to the repository.
   */
  readonly record?: string;
  /** A digest row another job built, as one JSON line. */
  readonly row?: string;
  readonly file?: string[];
  /**
   * Whether to record a build whose directory carries no fetch evidence.
   *
   * @remarks
   * The gid is then the caller's word. Continuous integration never passes it,
   * and a case refuses a workflow that does.
   */
  readonly unattested?: boolean;
}

/** The flag a command cannot run without, or a usage failure naming it. */
function required(
  options: Options,
  name: "dir" | "out" | "manifest" | "row",
): string {
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
  emit: "hash a local build and print its digest row, or the committed one it matches",
  append: "append a digest row another job built",
  verify: "check a local build against its digest row",
  status: "ask the bucket about a build, and sweep every other recorded one",
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
      record: { type: "string" },
      row: { type: "string" },
      file: { type: "string", multiple: true },
      unattested: { type: "boolean" },
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
    if (verb === "emit") await commandEmit(options);
    if (verb === "append") await commandAppend(options);
    if (verb === "verify") await commandVerify(options);
    if (verb === "status") await commandStatus(options);
    if (verb === "push") await commandPush(options);
    if (verb === "pull") await commandPull(options);
  } catch (error) {
    // A refusal this tool raises on purpose prints as a result row. Anything
    // else is a defect and keeps its stack, because a stack is what a defect
    // needs and a refusal is not helped by one.
    if (
      error instanceof MissingCredentialError ||
      error instanceof ArchiveUnreachableError
    ) {
      row(false, error.message);
      summary(
        error instanceof MissingCredentialError
          ? "no archive was reached"
          : "the archive could not be asked, so nothing was decided",
        true,
      );
    }
    if (error instanceof DuplicateRowError || error instanceof RecordError) {
      row(false, error.message);
      summary(
        `${BUILD_DIGESTS_PATH} has to be repaired before this can run`,
        true,
      );
    }
    throw error;
  }
}
