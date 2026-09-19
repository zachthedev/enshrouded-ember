import {
  afterAll,
  afterEach,
  beforeAll,
  describe,
  expect,
  test,
} from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  archiveClient,
  archiveState,
  ArchiveUnreachableError,
  bucketSizes,
  compareDigests,
  kfcVersion,
  credentials,
  digestFile,
  DESTINATION,
  digestRow,
  fetchedManifest,
  DuplicateRowError,
  MissingCredentialError,
  objectKey,
  objectSize,
  sweepArchive,
  uploadDecision,
} from "./archive.ts";
import { appendRecord, BuildDigestRecord, readRecords } from "./records.ts";

/** Every sandbox this file made, removed once the case ends. */
const sandboxes: string[] = [];

/** A directory of this suite's own, under the temp root the session sets. */
async function sandbox(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), "ember-archive-"));
  sandboxes.push(dir);
  return dir;
}

afterEach(async () => {
  while (sandboxes.length > 0) {
    await rm(sandboxes.pop() as string, { recursive: true, force: true });
  }
});

/** The SHA-256 of the three bytes "abc", from the FIPS 180-4 example. */
const ABC_SHA256 =
  "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

/** A digest row of the right shape, for a case to bend one field of. */
const row = {
  manifestId: "2174935030716737236",
  buildId: "23178631",
  appId: 2278520,
  depotId: 2278521,
  revision: 1024233,
  branch: "^/game38/branches/ea_update_08",
  recordedAt: "2026-09-18",
  files: {
    "enshrouded_server.exe": { bytes: 3, sha256: ABC_SHA256 },
    "enshrouded_server.kfc": { bytes: 3, sha256: ABC_SHA256 },
  },
};

/** Where a container's location records start, as the format puts them. */
const LOCATIONS_AT = 0x10;

/**
 * A KFC3 container header whose version slot points at one line.
 *
 * @remarks
 * Only the directory header is built here, because that is all the reader
 * touches: the magic, the location table, and the bytes the first record
 * points at. A real container carries megabytes of resources after it.
 *
 * @param line - The version line to place.
 * @param options - `magic` replaces the four opening bytes, and `relative`
 * replaces the offset the record carries, where zero marks an unused slot.
 * @returns The bytes of a container header.
 */
function container(
  line: string,
  options: { magic?: string; relative?: number } = {},
): Uint8Array {
  const text = new TextEncoder().encode(line);
  const at = LOCATIONS_AT + 16 * 8;
  const bytes = new Uint8Array(at + text.byteLength);
  bytes.set(new TextEncoder().encode(options.magic ?? "KFC3"), 0);
  const view = new DataView(bytes.buffer);
  view.setUint32(LOCATIONS_AT, options.relative ?? at - LOCATIONS_AT, true);
  view.setUint32(LOCATIONS_AT + 4, text.byteLength, true);
  bytes.set(text, at);
  return bytes;
}

describe("kfcVersion", () => {
  /** The line the current build carries, byte for byte. */
  const real = "1024233|^/game38/branches/ea_update_08|2026-05-11T10:44:54Z";

  test("the revision and the branch are read, and the timestamp is not", async () => {
    const path = join(await sandbox(), "enshrouded_server.kfc");
    await Bun.write(path, container(real));
    expect(await kfcVersion(path)).toEqual({
      revision: 1024233,
      branch: "^/game38/branches/ea_update_08",
    });
  });

  test.each([
    ["another magic word", container(real, { magic: "KFC2" })],
    ["an unused version slot", container(real, { relative: 0 })],
    ["a line that is not a version line", container("hello there")],
    ["a branch that is not caret syntax", container("1|game38/trunk|x")],
    ["a revision that is not decimal", container("ea_08|^/game38/trunk|x")],
  ])("a container with %s reads as nothing", async (_name, bytes) => {
    const path = join(await sandbox(), "enshrouded_server.kfc");
    await Bun.write(path, bytes);
    expect(await kfcVersion(path)).toBeNull();
  });

  /** The archived `.kfc` of a build this tool cannot parse is still archived. */
  test("a file too short to hold a header reads as nothing", async () => {
    const path = join(await sandbox(), "enshrouded_server.kfc");
    await Bun.write(path, "abc");
    expect(await kfcVersion(path)).toBeNull();
  });
});

describe("objectKey", () => {
  /**
   * Keyed by manifest gid, not build id. Four of the five builds in the
   * supported window were pulled from a historical manifest and carry no
   * recoverable build id, so a key built from one would have a hole in it.
   */
  test("a key is the manifest gid and the file name", () => {
    expect(objectKey("2174935030716737236", "enshrouded_server.exe")).toBe(
      "build/2174935030716737236/enshrouded_server.exe",
    );
  });
});

describe("archiveClient", () => {
  /**
   * Signing is local arithmetic, so a presigned URL says where a request would
   * have gone without one being sent. The key pair here is a throwaway and
   * reaches nothing.
   */
  const signed = (): URL =>
    new URL(
      archiveClient({
        accessKeyId: "AKIAEXAMPLE",
        secretAccessKey: "secretexample",
      }).presign(objectKey("2174935030716737236", "enshrouded_server.exe"), {
        method: "GET",
        expiresIn: 60,
      }),
    );

  /**
   * Both expectations are derived from `archive.json` rather than written out
   * here. A fork points that file at its own bucket and this suite stays
   * green, which is the reason the destination is a committed file rather than
   * a literal in this repository's TypeScript.
   */
  test("the endpoint is the configured account's R2 host", () => {
    expect(signed().host).toBe(
      `${DESTINATION.accountId}.r2.cloudflarestorage.com`,
    );
  });

  test("the path is the configured bucket and the manifest key", () => {
    expect(signed().pathname).toBe(
      `/${DESTINATION.bucket}/build/2174935030716737236/enshrouded_server.exe`,
    );
  });

  /** R2 rejects a signature scoped to any region but the one it documents. */
  test("the request is signed for the auto region and the s3 service", () => {
    const url = signed();
    expect(url.searchParams.has("X-Amz-Signature")).toBe(true);
    expect(url.searchParams.get("X-Amz-Credential")).toContain("/auto/s3/");
  });

  /**
   * Both halves of the destination come from `archive.json` and from nothing
   * else, so no environment can repoint them. Asserting the path from inside
   * this process cannot show that: a constant reading `process.env` with the
   * committed value as its default produces the same path whenever the
   * variable is unset. The check has to run in a process whose environment is
   * hostile, so it runs in one.
   *
   * This is the hole the whole arrangement avoids, and it is the reason there
   * is no local override. An override would have to be proved refused in
   * continuous integration, and no override at all is simpler than that proof.
   */
  test("no environment variable can repoint the bucket or the account", () => {
    const poisoned = Object.fromEntries(
      [
        "R2_BUCKET",
        "BUCKET",
        "AWS_BUCKET",
        "R2_ARCHIVE_BUCKET",
        "R2_ACCOUNT_ID",
        "AWS_ENDPOINT_URL",
        "AWS_ENDPOINT_URL_S3",
        "S3_ENDPOINT",
        "AWS_REGION",
      ].map((name) => [name, "repointed-by-the-environment"]),
    );
    const script = `
      const { archiveClient, objectKey } = await import(${JSON.stringify(
        fileURLToPath(new URL("archive.ts", import.meta.url)),
      )});
      const url = archiveClient({
        accessKeyId: "AKIAEXAMPLE",
        secretAccessKey: "secretexample",
      }).presign(objectKey("1", "enshrouded_server.exe"), { method: "GET" });
      console.log(new URL(url).host + new URL(url).pathname);
    `;
    const run = Bun.spawnSync({
      cmd: [process.execPath, "-e", script],
      env: { ...process.env, ...poisoned },
      stdout: "pipe",
      stderr: "pipe",
    });
    expect(run.stderr.toString()).toBe("");
    expect(run.stdout.toString().trim()).toBe(
      `${DESTINATION.accountId}.r2.cloudflarestorage.com` +
        `/${DESTINATION.bucket}/build/1/enshrouded_server.exe`,
    );
  });

  /**
   * The credential is the asset this whole dimension protects, and a leak is
   * the failure nothing else in the suite would notice. Sentinels rather than
   * real values, so the case carries no secret of its own.
   */
  test("building a client prints no part of the key pair", () => {
    const written: string[] = [];
    const log = console.log;
    const error = console.error;
    console.log = (...parts: unknown[]): void => {
      written.push(parts.join(" "));
    };
    console.error = console.log;
    try {
      archiveClient({
        accessKeyId: "SENTINEL-ACCESS-KEY-ID",
        secretAccessKey: "SENTINEL-SECRET-ACCESS-KEY",
      });
      credentials("write", {
        R2_ARCHIVE_WRITE_ACCESS_KEY_ID: "SENTINEL-ACCESS-KEY-ID",
        R2_ARCHIVE_WRITE_SECRET_ACCESS_KEY: "SENTINEL-SECRET-ACCESS-KEY",
      });
    } finally {
      console.log = log;
      console.error = error;
    }
    expect(written.join("\n")).not.toContain("SENTINEL-ACCESS-KEY-ID");
    expect(written.join("\n")).not.toContain("SENTINEL-SECRET-ACCESS-KEY");
  });
});

describe("credentials", () => {
  const full = {
    R2_ARCHIVE_READ_ACCESS_KEY_ID: "read-id",
    R2_ARCHIVE_READ_SECRET_ACCESS_KEY: "read-secret",
    R2_ARCHIVE_WRITE_ACCESS_KEY_ID: "write-id",
    R2_ARCHIVE_WRITE_SECRET_ACCESS_KEY: "write-secret",
  };

  test.each([
    ["read", "read-id", "read-secret"],
    ["write", "write-id", "write-secret"],
  ] as const)(
    "the %s token is read from its own two names",
    (access, id, secret) => {
      expect(credentials(access, full)).toEqual({
        accessKeyId: id,
        secretAccessKey: secret,
      });
    },
  );

  /**
   * An absent credential has to say so rather than signing a request with an
   * empty key, which is the state of every run triggered from a fork.
   */
  test("an empty environment names every variable that is absent", () => {
    const failed = (): unknown => credentials("read", {});
    expect(failed).toThrow(MissingCredentialError);
    try {
      failed();
    } catch (error) {
      const missing = (error as MissingCredentialError).missing;
      expect(missing).toEqual([
        "R2_ARCHIVE_READ_ACCESS_KEY_ID",
        "R2_ARCHIVE_READ_SECRET_ACCESS_KEY",
      ]);
      expect((error as Error).message).toContain("minted by hand");
    }
  });

  /**
   * The refusal is printed, and on a runner it is printed into a log. It names
   * the variables and must never carry what was in them, including through a
   * property somebody adds later for debugging.
   */
  test("the refusal carries no value from any environment in reach", () => {
    // Two sentinels, because a leak has two sources. One is the environment
    // this call was handed. The other is the real process environment, which
    // on a runner holds the credential and which a line added here for
    // debugging would read directly.
    const previous = process.env["R2_ARCHIVE_READ_ACCESS_KEY_ID"];
    process.env["R2_ARCHIVE_READ_ACCESS_KEY_ID"] = "SENTINEL-FROM-PROCESS-ENV";
    try {
      credentials("read", {
        R2_ARCHIVE_READ_ACCESS_KEY_ID: "SENTINEL-FROM-ARGUMENT",
        R2_ARCHIVE_READ_SECRET_ACCESS_KEY: "",
      });
      throw new Error("the credential must be refused");
    } catch (error) {
      const own = Object.getOwnPropertyNames(error as object);
      const seen = [
        (error as Error).message,
        (error as Error).stack ?? "",
        JSON.stringify(error),
        JSON.stringify(own),
        JSON.stringify(
          own.map(
            (name) => (error as unknown as Record<string, unknown>)[name],
          ),
        ),
        String(error),
      ].join("\n");
      expect(seen).not.toContain("SENTINEL-FROM-ARGUMENT");
      expect(seen).not.toContain("SENTINEL-FROM-PROCESS-ENV");
    } finally {
      if (previous === undefined) {
        delete process.env["R2_ARCHIVE_READ_ACCESS_KEY_ID"];
      } else {
        process.env["R2_ARCHIVE_READ_ACCESS_KEY_ID"] = previous;
      }
    }
  });

  /** A secret that GitHub did not pass arrives as an empty string, not absent. */
  test.each([
    ["absent", undefined],
    ["empty, as a fork run receives it", ""],
  ])("a secret that is %s is refused", (_name, value) => {
    const partial = { ...full, R2_ARCHIVE_READ_SECRET_ACCESS_KEY: value };
    try {
      credentials("read", partial);
      throw new Error("the credential must be refused");
    } catch (error) {
      expect(error).toBeInstanceOf(MissingCredentialError);
      expect((error as MissingCredentialError).missing).toEqual([
        "R2_ARCHIVE_READ_SECRET_ACCESS_KEY",
      ]);
    }
  });

  /** The read token must never reach a write, and the two names never blur. */
  test("the write names do not satisfy a read", () => {
    const writeOnly = {
      R2_ARCHIVE_WRITE_ACCESS_KEY_ID: "write-id",
      R2_ARCHIVE_WRITE_SECRET_ACCESS_KEY: "write-secret",
    };
    expect(() => credentials("read", writeOnly)).toThrow(
      MissingCredentialError,
    );
  });
});

describe("digestFile", () => {
  test("a file hashes to its SHA-256 and reports its size", async () => {
    const dir = await sandbox();
    const path = join(dir, "abc.bin");
    await Bun.write(path, "abc");
    expect(await digestFile(path)).toEqual({ bytes: 3, sha256: ABC_SHA256 });
  });

  test("an empty file hashes to the empty digest", async () => {
    const dir = await sandbox();
    const path = join(dir, "empty.bin");
    await Bun.write(path, "");
    const digest = await digestFile(path);
    expect(digest.bytes).toBe(0);
    expect(digest.sha256).toBe(
      "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
  });
});

describe("compareDigests", () => {
  const expected = {
    "enshrouded_server.exe": { bytes: 3, sha256: ABC_SHA256 },
  };

  test("a file that matches produces no problem", () => {
    expect(
      compareDigests(
        { "enshrouded_server.exe": expected["enshrouded_server.exe"] },
        expected,
      ),
    ).toEqual([]);
  });

  test.each([
    ["absent", null, ["is absent"]],
    [
      "one byte different",
      { bytes: 3, sha256: `${ABC_SHA256.slice(0, 63)}e` },
      ["hashes to"],
    ],
    [
      "truncated",
      { bytes: 2, sha256: ABC_SHA256 },
      ["is 2 bytes, and the record says 3"],
    ],
  ])("a file that is %s is reported", (_name, actual, fragments) => {
    const problems = compareDigests(
      { "enshrouded_server.exe": actual },
      expected,
    );
    expect(problems).not.toHaveLength(0);
    for (const fragment of fragments) {
      expect(problems.map((problem) => problem.detail).join(" ")).toContain(
        fragment,
      );
    }
  });

  /**
   * A row naming two files and a directory holding one must not pass on the
   * strength of the file that is there.
   */
  test("every file the row names is checked", () => {
    const two = {
      ...expected,
      "enshrouded_server.kfc": { bytes: 3, sha256: ABC_SHA256 },
    };
    const problems = compareDigests(
      { "enshrouded_server.exe": expected["enshrouded_server.exe"] },
      two,
    );
    expect(problems.map((problem) => problem.fileName)).toEqual([
      "enshrouded_server.kfc",
    ]);
  });
});

describe("fetchedManifest", () => {
  /** A SteamCMD app manifest, which is KeyValues rather than JSON. */
  const acf = (depots: Record<string, string>): string =>
    [
      '"AppState"',
      "{",
      '\t"appid"\t\t"2278520"',
      '\t"buildid"\t\t"23178631"',
      '\t"InstalledDepots"',
      "\t{",
      ...Object.entries(depots).flatMap(([depot, manifest]) => [
        `\t\t"${depot}"`,
        "\t\t{",
        `\t\t\t"manifest"\t\t"${manifest}"`,
        '\t\t\t"size"\t\t"8756751690"',
        "\t\t}",
      ]),
      "\t}",
      "}",
    ].join("\n");

  test("a directory with no evidence reads as null", async () => {
    expect(await fetchedManifest(await sandbox(), 2278520, 2278521)).toBeNull();
  });

  /**
   * SteamCMD's `app_update` takes no manifest parameter and returns the head of
   * the branch, so this is the only thing that says which build arrived.
   */
  test("the app manifest names the depot's manifest", async () => {
    const dir = await sandbox();
    await Bun.write(
      join(dir, "steamapps", "appmanifest_2278520.acf"),
      acf({ "1004": "7604377918839582995", "2278521": "2174935030716737236" }),
    );
    const found = await fetchedManifest(dir, 2278520, 2278521);
    expect(found?.manifestId).toBe("2174935030716737236");
    expect(found?.source).toContain("appmanifest_2278520.acf");
  });

  /**
   * A file filter leaves depots listed that nothing was written from, so the
   * table is read by depot rather than by taking whichever entry comes first.
   */
  test("another depot's manifest is not taken", async () => {
    const dir = await sandbox();
    await Bun.write(
      join(dir, "steamapps", "appmanifest_2278520.acf"),
      acf({ "1004": "7604377918839582995" }),
    );
    expect(await fetchedManifest(dir, 2278520, 2278521)).toBeNull();
  });

  test("a gid that is not decimal is no evidence", async () => {
    const dir = await sandbox();
    await Bun.write(
      join(dir, "steamapps", "appmanifest_2278520.acf"),
      acf({ "2278521": "../../../evil" }),
    );
    expect(await fetchedManifest(dir, 2278520, 2278521)).toBeNull();
  });

  /** DepotDownloader puts the gid in the name of the manifest it cached. */
  test("the DepotDownloader cache names the manifest in its file name", async () => {
    const dir = await sandbox();
    await Bun.write(
      join(dir, ".DepotDownloader", "2278521_5177045887918896292.manifest"),
      "binary",
    );
    await Bun.write(join(dir, ".DepotDownloader", "depot.config"), "binary");
    const found = await fetchedManifest(dir, 2278520, 2278521);
    expect(found?.manifestId).toBe("5177045887918896292");
  });

  test("a cached manifest for another depot is not taken", async () => {
    const dir = await sandbox();
    await Bun.write(
      join(dir, ".DepotDownloader", "1004_7604377918839582995.manifest"),
      "binary",
    );
    expect(await fetchedManifest(dir, 2278520, 2278521)).toBeNull();
  });

  /**
   * A filtered SteamCMD fetch leaves a second application's manifest behind.
   * Only the application and depot asked for decide anything.
   */
  test("a second application's manifest is ignored", async () => {
    const dir = await sandbox();
    await Bun.write(
      join(dir, "steamapps", "appmanifest_228980.acf"),
      '"AppState"\n{\n\t"appid"\t\t"228980"\n}\n',
    );
    expect(await fetchedManifest(dir, 2278520, 2278521)).toBeNull();
  });
});

describe("the record command, driven end to end", () => {
  /** A SteamCMD app manifest naming one gid for the server depot. */
  const acf = (manifest: string): string =>
    [
      '"AppState"',
      "{",
      '\t"buildid"\t\t"23178631"',
      '\t"InstalledDepots"',
      "\t{",
      '\t\t"2278521"',
      "\t\t{",
      `\t\t\t"manifest"\t\t"${manifest}"`,
      "\t\t}",
      "\t}",
      "}",
    ].join("\n");

  /** A directory shaped like one a filtered SteamCMD fetch leaves behind. */
  const fetched = async (manifest: string): Promise<string> => {
    const dir = await sandbox();
    await Bun.write(join(dir, "enshrouded_server.exe"), "abc");
    await Bun.write(join(dir, "enshrouded_server.kfc"), "abc");
    await Bun.write(
      join(dir, "steamapps", "appmanifest_2278520.acf"),
      acf(manifest),
    );
    return dir;
  };

  /**
   * The real command, pointed at a sandbox record so no case can append to the
   * repository's own. Running the command rather than a piece of it is the
   * point: the refusal lives in the command, and a case over an extracted
   * fragment would stay green if the command stopped calling it.
   */
  // No return annotation: the literal options below narrow stdout and stderr
  // to strings, and naming the general type throws that away.
  const record = (
    dir: string,
    manifestId: string,
    recordPath: string,
    extra: string[] = [],
  ) =>
    Bun.spawnSync({
      cmd: [
        process.execPath,
        "run",
        fileURLToPath(new URL("archive.ts", import.meta.url)),
        "record",
        "--dir",
        dir,
        "--manifest",
        manifestId,
        "--record",
        recordPath,
        ...extra,
      ],
      env: { ...process.env, GITHUB_OUTPUT: "" },
      stdout: "pipe",
      stderr: "pipe",
    });

  test("a fetch that matches what was asked for is recorded", async () => {
    const dir = await fetched("2174935030716737236");
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    const run = record(dir, "2174935030716737236", recordPath);
    expect(run.stdout.toString()).toContain("confirmed by");
    expect(run.exitCode).toBe(0);
    expect(await readRecords(recordPath, BuildDigestRecord)).toHaveLength(1);
  });

  /**
   * `app_update` always fetches the head of the branch, so a Keen release
   * landing between the watch job and the archive job returns a build nobody
   * asked for. Recording it would key the new bytes under the old gid, which is
   * a wrong record rather than a failed run.
   */
  test("a fetch that returned another build is refused, and records nothing", async () => {
    const dir = await fetched("9999999999999999999");
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    const run = record(dir, "2174935030716737236", recordPath);
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain("9999999999999999999");
    expect(await Bun.file(recordPath).exists()).toBe(false);
  });

  /**
   * A file filter that matches nothing leaves SteamCMD reporting success over
   * an empty directory, so an absent file has to be a refusal rather than a
   * row naming less.
   */
  test("a fetch that wrote nothing is refused", async () => {
    const dir = await sandbox();
    await Bun.write(
      join(dir, "steamapps", "appmanifest_2278520.acf"),
      acf("2174935030716737236"),
    );
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    const run = record(dir, "2174935030716737236", recordPath);
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain("enshrouded_server.exe is not in");
    expect(await Bun.file(recordPath).exists()).toBe(false);
  });

  /**
   * SteamCMD leaves an app manifest and DepotDownloader leaves a cached one,
   * so bytes with neither arrived some other way. Recording them keys them to
   * whatever gid was typed, in a bucket with no versioning, and every later
   * check compares them against that same row.
   */
  test("a directory carrying no fetch evidence is refused, and records nothing", async () => {
    const dir = await sandbox();
    await Bun.write(join(dir, "enshrouded_server.exe"), "abc");
    await Bun.write(join(dir, "enshrouded_server.kfc"), "abc");
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    const run = record(dir, "2174935030716737236", recordPath);
    expect(run.exitCode).toBe(1);
    expect(run.stdout.toString()).toContain(
      "carries no record of which manifest it came from",
    );
    expect(run.stdout.toString()).toContain("--unattested");
    expect(await Bun.file(recordPath).exists()).toBe(false);
  });

  /** The flag records it, and says in the output whose word the gid is. */
  test("--unattested records a build with no evidence, and says so", async () => {
    const dir = await sandbox();
    await Bun.write(join(dir, "enshrouded_server.exe"), "abc");
    await Bun.write(join(dir, "enshrouded_server.kfc"), "abc");
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    const run = record(dir, "2174935030716737236", recordPath, [
      "--unattested",
    ]);
    expect(run.exitCode).toBe(0);
    expect(run.stdout.toString()).toContain("is the caller's word");
    const rows = await readRecords(recordPath, BuildDigestRecord);
    expect(rows.map((one) => one.manifestId)).toEqual(["2174935030716737236"]);
  });

  /**
   * The container names the content the build was cut from, and the supported
   * build table matches on that revision. No Steam field carries it, so a row
   * that does not read it carries null where a person looks.
   */
  test("the row takes its revision and branch from the container", async () => {
    const dir = await fetched("2174935030716737236");
    await Bun.write(
      join(dir, "enshrouded_server.kfc"),
      container("1024233|^/game38/branches/ea_update_08|2026-05-11T10:44:54Z"),
    );
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    expect(record(dir, "2174935030716737236", recordPath).exitCode).toBe(0);
    const [written] = await readRecords(recordPath, BuildDigestRecord);
    expect(written?.revision).toBe(1024233);
    expect(written?.branch).toBe("^/game38/branches/ea_update_08");
  });

  /** A caller reading a container this tool cannot parse still has a way in. */
  test("the flags win over the container", async () => {
    const dir = await fetched("2174935030716737236");
    await Bun.write(
      join(dir, "enshrouded_server.kfc"),
      container("1024233|^/game38/branches/ea_update_08|2026-05-11T10:44:54Z"),
    );
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    const run = record(dir, "2174935030716737236", recordPath, [
      "--revision",
      "999",
      "--branch",
      "^/game38/trunk",
    ]);
    expect(run.exitCode).toBe(0);
    const [written] = await readRecords(recordPath, BuildDigestRecord);
    expect(written?.revision).toBe(999);
    expect(written?.branch).toBe("^/game38/trunk");
  });

  /** A build that is already recorded is never recorded twice. */
  test("a manifest that already has a row is refused", async () => {
    const dir = await fetched("2174935030716737236");
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    expect(record(dir, "2174935030716737236", recordPath).exitCode).toBe(0);
    const again = record(dir, "2174935030716737236", recordPath);
    expect(again.exitCode).toBe(1);
    expect(again.stdout.toString()).toContain("already has a row");
    expect(await readRecords(recordPath, BuildDigestRecord)).toHaveLength(1);
  });
});

describe("uploadDecision", () => {
  const expected = { bytes: 22557696, sha256: ABC_SHA256 };

  /**
   * Stands in for downloading and hashing the object, and counts the calls.
   * The download is the expensive half, so when it happens is part of the
   * contract.
   */
  const hasher = (sha256: string) => {
    const calls: number[] = [];
    return {
      calls,
      hash: async (): Promise<string> => {
        calls.push(1);
        return sha256;
      },
    };
  };

  test("an absent object is uploaded, and nothing is downloaded", async () => {
    const existing = hasher(ABC_SHA256);
    expect(await uploadDecision(expected, null, existing.hash)).toEqual({
      action: "upload",
    });
    expect(existing.calls).toHaveLength(0);
  });

  /**
   * This is what makes a retry converge. A run that uploaded and then failed
   * before its row was committed leaves the build unrecorded, so the next run
   * fetches and pushes again, and so does a second run of the seed. Skipping an
   * object that already holds the recorded bytes is what stops that second
   * push from refusing, which would loop forever, or overwriting, which R2
   * cannot undo.
   */
  test("an object that already hashes to the record is skipped", async () => {
    const existing = hasher(ABC_SHA256);
    expect(await uploadDecision(expected, 22557696, existing.hash)).toEqual({
      action: "skip",
    });
    expect(existing.calls).toHaveLength(1);
  });

  /**
   * A matching size says nothing about the bytes. Skipped on its size alone,
   * this object would be reported archived while holding something else. It
   * is not overwritten either, because R2 has no versioning.
   */
  test("an object at the recorded size with other bytes is refused, and says why", async () => {
    const other = "c".repeat(64);
    const decided = await uploadDecision(
      expected,
      22557696,
      hasher(other).hash,
    );
    expect(decided.action).toBe("refuse");
    expect(decided.action === "refuse" && decided.detail).toContain(other);
    expect(decided.action === "refuse" && decided.detail).toContain(ABC_SHA256);
  });

  test.each([
    ["shorter", 1024],
    ["longer", 99999999],
    ["empty", 0],
  ])(
    "an object that is %s is refused without being downloaded",
    async (_n, bytes) => {
      const existing = hasher(ABC_SHA256);
      const decided = await uploadDecision(expected, bytes, existing.hash);
      expect(decided.action).toBe("refuse");
      expect(decided.action === "refuse" && decided.detail).toContain(
        String(bytes),
      );
      expect(existing.calls).toHaveLength(0);
    },
  );
});

describe("archiveState", () => {
  /** The bucket's answer for the two archived files, null where absent. */
  const sizes = (
    exe: number | null,
    kfc: number | null,
  ): Record<string, number | null> => ({
    "enshrouded_server.exe": exe,
    "enshrouded_server.kfc": kfc,
  });

  const exeKey = "build/2174935030716737236/enshrouded_server.exe";
  const kfcKey = "build/2174935030716737236/enshrouded_server.kfc";

  /**
   * A digest row pins what a build's files hash to, and a build can be hashed
   * from a local copy before anything reaches the bucket. The row is not a
   * receipt for an upload, so it cannot answer whether one happened.
   */
  test("a recorded build with nothing in the bucket still needs archiving", () => {
    const decided = archiveState("2174935030716737236", row, sizes(null, null));
    expect(decided.state).toBe("missing");
    expect(decided.state === "missing" && decided.detail).toContain(exeKey);
    expect(decided.state === "missing" && decided.detail).toContain(kfcKey);
  });

  /**
   * Objects with no row are an upload whose row never reached the record.
   * `pull` refuses a build it has nothing to check against, so the archive job
   * has to run again and finish the commit.
   */
  test("a build with no row needs archiving, whatever the bucket holds", () => {
    const decided = archiveState("2174935030716737236", null, sizes(3, 3));
    expect(decided.state).toBe("missing");
    expect(decided.state === "missing" && decided.detail).toContain(
      "no digest row",
    );
  });

  test("a build is archived once every file is there at the recorded size", () => {
    expect(archiveState("2174935030716737236", row, sizes(3, 3))).toEqual({
      state: "archived",
    });
  });

  test("one absent file leaves the build unarchived, and is the one named", () => {
    const decided = archiveState("2174935030716737236", row, sizes(3, null));
    expect(decided.state).toBe("missing");
    expect(decided.state === "missing" && decided.detail).toContain(kfcKey);
    expect(decided.state === "missing" && decided.detail).not.toContain(exeKey);
  });

  /**
   * push refuses to write over an object the record does not describe, so the
   * archive job cannot settle this, and running it every hour would only ask
   * for an approval that ends red.
   */
  test("an object at another size is a conflict, named with both sizes", () => {
    const decided = archiveState("2174935030716737236", row, sizes(3, 99));
    expect(decided.state).toBe("conflict");
    expect(decided.state === "conflict" && decided.detail).toContain(kfcKey);
    expect(decided.state === "conflict" && decided.detail).toContain(
      "holds 99 bytes, and the record says 3",
    );
  });

  test("a conflict outranks an absent file", () => {
    expect(
      archiveState("2174935030716737236", row, sizes(null, 99)).state,
    ).toBe("conflict");
  });
});

describe("asking a loopback bucket", () => {
  /**
   * A stand-in for R2 on the loopback interface, so the real S3 client reads
   * each answer. How Bun reports a missing key, a refused request and a failed
   * one is the thing under test, and a double of the client would only restate
   * it.
   */
  const answers: Record<string, number> = {
    "/loopback/build/1/enshrouded_server.exe": 200,
    "/loopback/build/1/enshrouded_server.kfc": 404,
    "/loopback/build/1/forbidden": 403,
    "/loopback/build/1/broken": 500,
  };
  const requests: string[] = [];
  let server: ReturnType<typeof Bun.serve> | undefined;
  let bucket: Bun.S3Client;

  beforeAll(() => {
    server = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      fetch(request) {
        const path = new URL(request.url).pathname;
        requests.push(`${request.method} ${path}`);
        const status = answers[path] ?? 404;
        return new Response(null, {
          status,
          headers: status === 200 ? { "content-length": "3" } : {},
        });
      },
    });
    bucket = new Bun.S3Client({
      accessKeyId: "SENTINEL-ACCESS-KEY-ID",
      secretAccessKey: "SENTINEL-SECRET-ACCESS-KEY",
      bucket: "loopback",
      region: "auto",
      endpoint: `http://127.0.0.1:${server.port}`,
    });
  });

  afterAll(async () => {
    await server?.stop(true);
  });

  test("an object that is there reads as its size", async () => {
    expect(await objectSize(bucket, "build/1/enshrouded_server.exe")).toBe(3);
  });

  test("a key that is not there reads as absent", async () => {
    expect(
      await objectSize(bucket, "build/1/enshrouded_server.kfc"),
    ).toBeNull();
  });

  /**
   * A token that lost its read access answers 403 for every key. Read as
   * absent, that would make every build look unarchived and hand push a key
   * it never looked inside.
   *
   * The refusal is printed into a runner log, so it must carry no credential.
   * The client that asked holds one sentinel pair, and the process
   * environment holds another, because on a runner the environment is where
   * the real token sits and a line added for debugging would read it there.
   */
  test.each([
    ["refused", "build/1/forbidden"],
    ["failed", "build/1/broken"],
  ])(
    "a request the bucket %s is thrown, named, and carries no credential",
    async (_name, key) => {
      const names = [
        "R2_ARCHIVE_READ_ACCESS_KEY_ID",
        "R2_ARCHIVE_READ_SECRET_ACCESS_KEY",
        "R2_ARCHIVE_WRITE_ACCESS_KEY_ID",
        "R2_ARCHIVE_WRITE_SECRET_ACCESS_KEY",
      ];
      const previous = names.map((name) => process.env[name]);
      for (const name of names) {
        process.env[name] = "SENTINEL-FROM-PROCESS-ENV";
      }
      try {
        const asked = objectSize(bucket, key);
        await expect(asked).rejects.toBeInstanceOf(ArchiveUnreachableError);
        const error = (await asked.catch((caught: unknown) => caught)) as Error;
        const seen = [error.message, error.stack ?? "", String(error)].join(
          "\n",
        );
        expect(error.message).toContain(key);
        expect(seen).not.toContain("SENTINEL");
      } finally {
        for (const [index, name] of names.entries()) {
          const value = previous[index];
          if (value === undefined) {
            delete process.env[name];
          } else {
            process.env[name] = value;
          }
        }
      }
    },
  );

  /**
   * The status check asks for every file the row names, under the key the
   * manifest gives it, and reports an absent one as absent.
   */
  test("every file a row names is asked for under its manifest key", async () => {
    requests.length = 0;
    const sizes = await bucketSizes(bucket, "1", { ...row, manifestId: "1" });
    expect(sizes).toEqual({
      "enshrouded_server.exe": 3,
      "enshrouded_server.kfc": null,
    });
    expect(requests.sort()).toEqual([
      "HEAD /loopback/build/1/enshrouded_server.exe",
      "HEAD /loopback/build/1/enshrouded_server.kfc",
    ]);
  });

  /**
   * The answer for a build with no row is settled without asking, and asking
   * anyway is what exercises the credential. A token revoked or scoped wrong
   * is then found in the hour it happens rather than at the next upload, which
   * is the whole reason the check runs hourly.
   */
  test("a build with no row is asked about under the archived file names", async () => {
    requests.length = 0;
    expect(await bucketSizes(bucket, "1", null)).toEqual({
      "enshrouded_server.exe": 3,
      "enshrouded_server.kfc": null,
    });
    expect(requests.sort()).toEqual([
      "HEAD /loopback/build/1/enshrouded_server.exe",
      "HEAD /loopback/build/1/enshrouded_server.kfc",
    ]);
  });

  /**
   * The archive job repairs the build at the head of the branch and no other.
   * The rest cannot be refetched from Steam at all, so they are swept every
   * hour, and the build already asked about is not asked about twice.
   */
  test("the sweep covers every recorded build except the one named", async () => {
    requests.length = 0;
    const present = { ...row, manifestId: "1" };
    const absent = {
      ...row,
      manifestId: "2",
      files: {
        ...row.files,
        "enshrouded_server.kfc": { bytes: 3, sha256: ABC_SHA256 },
      },
    };
    const swept = await sweepArchive(bucket, [present, absent], "2");
    expect(swept.map((one) => one.manifestId)).toEqual(["1"]);
    expect(swept[0]?.state.state).toBe("missing");
    expect(requests.every((request) => request.includes("/build/1/"))).toBe(
      true,
    );
  });

  test("a sweep over a record holding only the named build asks nothing", async () => {
    requests.length = 0;
    expect(
      await sweepArchive(bucket, [{ ...row, manifestId: "1" }], "1"),
    ).toEqual([]);
    expect(requests).toEqual([]);
  });
});

describe("the emit and append verbs", () => {
  /** A SteamCMD app manifest naming one gid for the server depot. */
  const acf = (manifest: string): string =>
    [
      '"AppState"',
      "{",
      '\t"InstalledDepots"',
      "\t{",
      '\t\t"2278521"',
      "\t\t{",
      `\t\t\t"manifest"\t\t"${manifest}"`,
      "\t\t}",
      "\t}",
      "}",
    ].join("\n");

  const fetched = async (manifest: string): Promise<string> => {
    const dir = await sandbox();
    await Bun.write(join(dir, "enshrouded_server.exe"), "abc");
    await Bun.write(join(dir, "enshrouded_server.kfc"), "abc");
    await Bun.write(
      join(dir, "steamapps", "appmanifest_2278520.acf"),
      acf(manifest),
    );
    return dir;
  };

  /**
   * The real command in a process of its own. `GITHUB_OUTPUT` is always set,
   * empty unless a case names a file, so the suite run inside a workflow step
   * never writes into that step's outputs.
   */
  const run = (args: string[], env: Record<string, string> = {}) =>
    Bun.spawnSync({
      cmd: [
        process.execPath,
        "run",
        fileURLToPath(new URL("archive.ts", import.meta.url)),
        ...args,
      ],
      env: { ...process.env, GITHUB_OUTPUT: "", ...env },
      stdout: "pipe",
      stderr: "pipe",
    });

  /** The one line of JSON a run prints, which is the row. */
  const emittedRow = (out: string): string =>
    out.split("\n").find((text) => text.startsWith("{")) as string;

  /**
   * The job that fetches holds the credential that can overwrite the archive,
   * and the job that commits holds a key that can push to main. Neither holds
   * the other, so the row crosses between them as a string, and `emit` is what
   * produces it without touching a file.
   */
  test("emit prints a row and writes no file", async () => {
    const dir = await fetched("2174935030716737236");
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    const emitted = run([
      "emit",
      "--dir",
      dir,
      "--manifest",
      "2174935030716737236",
      "--record",
      recordPath,
    ]);
    expect(emitted.exitCode).toBe(0);
    expect(await Bun.file(recordPath).exists()).toBe(false);

    const parsed = BuildDigestRecord.parse(
      JSON.parse(emittedRow(emitted.stdout.toString())),
    );
    expect(parsed.manifestId).toBe("2174935030716737236");
    expect(Object.keys(parsed.files).sort()).toEqual([
      "enshrouded_server.exe",
      "enshrouded_server.kfc",
    ]);
  });

  /** Every refusal `record` makes happens before a row can be emitted. */
  test("emit refuses a build that is not the one asked for", async () => {
    const dir = await fetched("9999999999999999999");
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    const emitted = run([
      "emit",
      "--dir",
      dir,
      "--manifest",
      "2174935030716737236",
      "--record",
      recordPath,
    ]);
    expect(emitted.exitCode).toBe(1);
    expect(emitted.stdout.toString()).toContain("9999999999999999999");
  });

  test("append writes the row emit produced", async () => {
    const dir = await fetched("2174935030716737236");
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    const emitted = run([
      "emit",
      "--dir",
      dir,
      "--manifest",
      "2174935030716737236",
      "--record",
      recordPath,
    ]);
    const appended = run([
      "append",
      "--row",
      emittedRow(emitted.stdout.toString()),
      "--record",
      recordPath,
    ]);
    expect(appended.exitCode).toBe(0);
    expect(await readRecords(recordPath, BuildDigestRecord)).toHaveLength(1);
  });

  /**
   * The row crossed a job boundary as a string, so it is checked again rather
   * than trusted.
   */
  test.each([
    ["not JSON at all", "{"],
    ["JSON that is not a row", '{"manifestId":"1"}'],
    ["a row naming only one archived file", partialRow()],
    ["a row with a corrupted digest", badDigestRow()],
  ])("append refuses a row that is %s, and says why", async (_name, line) => {
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    const appended = run(["append", "--row", line, "--record", recordPath]);
    expect(appended.exitCode).toBe(1);
    expect(await Bun.file(recordPath).exists()).toBe(false);
    // The refusal names the flag the bad value arrived on. `appendRecord`
    // would refuse this row too, so the exit code alone stays correct if the
    // check here is removed; what would go is the sentence that tells a
    // reader which input was wrong.
    expect(appended.stdout.toString()).toContain("--row");
  });

  /**
   * A rerun after the row already landed is not a failure. It is the shape a
   * retry takes once the commit finally succeeds.
   */
  test("append is quiet and green when the row is already recorded", async () => {
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    await appendRecord(recordPath, BuildDigestRecord, row);
    const again = run([
      "append",
      "--row",
      JSON.stringify(row),
      "--record",
      recordPath,
    ]);
    expect(again.exitCode).toBe(0);
    expect(again.stdout.toString()).toContain("already has a row");
    expect(await readRecords(recordPath, BuildDigestRecord)).toHaveLength(1);
  });

  /** The emit arguments for the build `row` describes. */
  const emitArgs = (dir: string, recordPath: string): string[] => [
    "emit",
    "--dir",
    dir,
    "--manifest",
    "2174935030716737236",
    "--record",
    recordPath,
  ];

  /**
   * A build can be recorded before it is archived, and the archive job runs
   * for any build the bucket lacks. Once the fetched bytes match the committed
   * row, that row is what is handed on, byte for byte, so the upload is proved
   * against the digests the record already pins and the commit adds nothing.
   * A row built afresh would carry today's date and no build id, so equality
   * with the committed line is what tells the two apart.
   */
  test("emit hands on the committed row once the fetched bytes match it", async () => {
    const dir = await fetched("2174935030716737236");
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    await appendRecord(recordPath, BuildDigestRecord, row);
    const committed = await Bun.file(recordPath).text();
    const outputs = join(await sandbox(), "github-output");

    const emitted = run(emitArgs(dir, recordPath), { GITHUB_OUTPUT: outputs });
    expect(emitted.exitCode).toBe(0);
    expect(emittedRow(emitted.stdout.toString())).toBe(committed.trim());
    expect(await Bun.file(outputs).text()).toBe(`row=${committed.trim()}\n`);
    expect(await Bun.file(recordPath).text()).toBe(committed);
  });

  /**
   * The row is the integrity record. A fetch that disagrees with it is either
   * a swapped build or a wrong row, and uploading settles neither.
   */
  test("emit refuses bytes the committed row does not pin, and hands nothing on", async () => {
    const dir = await fetched("2174935030716737236");
    await Bun.write(join(dir, "enshrouded_server.kfc"), "abd");
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    await appendRecord(recordPath, BuildDigestRecord, row);
    const outputs = join(await sandbox(), "github-output");

    const emitted = run(emitArgs(dir, recordPath), { GITHUB_OUTPUT: outputs });
    expect(emitted.exitCode).toBe(1);
    expect(emitted.stdout.toString()).toContain(
      "enshrouded_server.kfc hashes to",
    );
    expect(emitted.stdout.toString()).toContain(
      `the record says ${ABC_SHA256}`,
    );
    expect(await Bun.file(outputs).exists()).toBe(false);
  });

  test("emit still refuses another build when the one asked for has a row", async () => {
    const dir = await fetched("9999999999999999999");
    const recordPath = join(await sandbox(), "build-digests.jsonl");
    await appendRecord(recordPath, BuildDigestRecord, row);
    const outputs = join(await sandbox(), "github-output");

    const emitted = run(emitArgs(dir, recordPath), { GITHUB_OUTPUT: outputs });
    expect(emitted.exitCode).toBe(1);
    expect(emitted.stdout.toString()).toContain("9999999999999999999");
    expect(await Bun.file(outputs).exists()).toBe(false);
  });

  /**
   * The status check exists to ask the bucket. A run that cannot ask must not
   * read as either answer: false would skip a build that is not archived, and
   * true would ask for an approval on a guess. A build with no row needs no
   * request to answer, and is refused all the same.
   */
  test.each([
    ["a recorded build", true],
    ["a build with no row", false],
  ])(
    "status without the read token refuses for %s, and hands on no decision",
    async (_name, recorded) => {
      const recordPath = join(await sandbox(), "build-digests.jsonl");
      if (recorded) {
        await appendRecord(recordPath, BuildDigestRecord, row);
      }
      const outputs = join(await sandbox(), "github-output");

      const asked = run(
        ["status", "--manifest", "2174935030716737236", "--record", recordPath],
        {
          GITHUB_OUTPUT: outputs,
          R2_ARCHIVE_READ_ACCESS_KEY_ID: "",
          R2_ARCHIVE_READ_SECRET_ACCESS_KEY: "",
        },
      );
      expect(asked.exitCode).toBe(1);
      expect(asked.stdout.toString()).toContain(
        "R2_ARCHIVE_READ_ACCESS_KEY_ID, R2_ARCHIVE_READ_SECRET_ACCESS_KEY",
      );
      expect(await Bun.file(outputs).exists()).toBe(false);
    },
  );
});

/** A row naming one archived file, which the shape refuses. */
function partialRow(): string {
  return JSON.stringify({
    ...row,
    files: { "enshrouded_server.exe": { bytes: 3, sha256: ABC_SHA256 } },
  });
}

/** A row whose digest is not a SHA-256. */
function badDigestRow(): string {
  return JSON.stringify({
    ...row,
    files: {
      "enshrouded_server.exe": { bytes: 3, sha256: "nope" },
      "enshrouded_server.kfc": { bytes: 3, sha256: ABC_SHA256 },
    },
  });
}

describe("digestRow", () => {
  test("a manifest with no row reads as null", async () => {
    const path = join(await sandbox(), "build-digests.jsonl");
    expect(await digestRow("2174935030716737236", path)).toBeNull();
  });

  test("the row for the manifest asked for is the one returned", async () => {
    const path = join(await sandbox(), "build-digests.jsonl");
    await appendRecord(path, BuildDigestRecord, row);
    await appendRecord(path, BuildDigestRecord, {
      ...row,
      manifestId: "954904204024183479",
      buildId: null,
    });
    expect((await digestRow("954904204024183479", path))?.buildId).toBeNull();
    expect((await digestRow("2174935030716737236", path))?.buildId).toBe(
      "23178631",
    );
  });

  /**
   * A second row for a manifest that already has one is how a swapped object
   * would be blessed: it retires the digests the first row pinned, and it can
   * drop a file from the set that gets checked at all. An appended line is the
   * shape every legitimate change to this file has, so resolving the conflict
   * to either row is the wrong answer. Refusing is the right one.
   */
  test("two rows for one manifest are refused rather than resolved", async () => {
    const path = join(await sandbox(), "build-digests.jsonl");
    await appendRecord(path, BuildDigestRecord, row);
    await appendRecord(path, BuildDigestRecord, {
      ...row,
      files: {
        ...row.files,
        "enshrouded_server.exe": { bytes: 16, sha256: "c".repeat(64) },
      },
    });
    const read = digestRow("2174935030716737236", path);
    await expect(read).rejects.toBeInstanceOf(DuplicateRowError);
    await expect(read).rejects.toThrow("2174935030716737236");
  });

  /** A duplicate elsewhere in the file is still the file's problem. */
  test("a duplicate is refused whichever manifest is asked for", async () => {
    const path = join(await sandbox(), "build-digests.jsonl");
    const other = { ...row, manifestId: "954904204024183479" };
    await appendRecord(path, BuildDigestRecord, other);
    await appendRecord(path, BuildDigestRecord, other);
    await expect(digestRow("954904204024183479", path)).rejects.toBeInstanceOf(
      DuplicateRowError,
    );
  });
});

describe("the status command, against a loopback bucket", () => {
  /** Build 1 is in the bucket at the recorded size. Build 2 is not. */
  const present = [
    "/loopback/build/1/enshrouded_server.exe",
    "/loopback/build/1/enshrouded_server.kfc",
  ];
  let server: ReturnType<typeof Bun.serve> | undefined;

  beforeAll(() => {
    server = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      fetch(request) {
        const path = new URL(request.url).pathname;
        return present.includes(path)
          ? new Response(null, {
              status: 200,
              headers: { "content-length": "3" },
            })
          : new Response(null, { status: 404 });
      },
    });
  });

  afterAll(async () => {
    await server?.stop(true);
  });

  /**
   * The real command in a process of its own, with the bucket injected the way
   * `bucketSizes` takes one. The destination of a real run comes from
   * `archive.json` and from nowhere else, so this is the only way to reach the
   * answering path without a credential and without an override.
   */
  // Spawned rather than run in this process, because the command exits when it
  // is done, and awaited rather than blocking, because the bucket it asks is
  // this process serving the loopback port.
  const status = async (
    manifestId: string,
    recordPath: string,
    outputs: string,
  ): Promise<{ exitCode: number; stdout: string; stderr: string }> => {
    const script = `
      const { commandStatus } = await import(${JSON.stringify(
        fileURLToPath(new URL("archive.ts", import.meta.url)),
      )});
      const bucket = new Bun.S3Client({
        accessKeyId: "AKIAEXAMPLE",
        secretAccessKey: "secretexample",
        bucket: "loopback",
        region: "auto",
        endpoint: process.env["LOOPBACK_ENDPOINT"],
      });
      await commandStatus(
        { manifest: process.env["MANIFEST"], record: process.env["RECORD"] },
        bucket,
      );
    `;
    const run = Bun.spawn({
      cmd: [process.execPath, "-e", script],
      env: {
        ...process.env,
        LOOPBACK_ENDPOINT: `http://127.0.0.1:${server?.port ?? 0}`,
        MANIFEST: manifestId,
        RECORD: recordPath,
        GITHUB_OUTPUT: outputs,
      },
      stdout: "pipe",
      stderr: "pipe",
    });
    const [stdout, stderr, exitCode] = await Promise.all([
      new Response(run.stdout).text(),
      new Response(run.stderr).text(),
      run.exited,
    ]);
    return { exitCode, stdout, stderr };
  };

  /** A record holding a row for each named build, with three-byte files. */
  const recordOf = async (...manifests: string[]): Promise<string> => {
    const path = join(await sandbox(), "build-digests.jsonl");
    for (const manifestId of manifests) {
      await appendRecord(path, BuildDigestRecord, { ...row, manifestId });
    }
    return path;
  };

  /**
   * The archive job compares this output against the literal `true`. A
   * rename, a different casing or a stray space stops the archive for good,
   * and every other case in this file would stay green through it.
   */
  test("a build the bucket lacks hands on needs_archive=true", async () => {
    const outputs = join(await sandbox(), "github-output");
    const run = await status("2", await recordOf("2"), outputs);
    expect(run.stderr).toBe("");
    expect(run.exitCode).toBe(0);
    expect(await Bun.file(outputs).text()).toBe("needs_archive=true\n");
  });

  test("a build the bucket holds hands on needs_archive=false", async () => {
    const outputs = join(await sandbox(), "github-output");
    const run = await status("1", await recordOf("1"), outputs);
    expect(run.exitCode).toBe(0);
    expect(await Bun.file(outputs).text()).toBe("needs_archive=false\n");
  });

  /**
   * Steam serves none of the older builds any more, so one of them leaving the
   * bucket is the loss nothing else would report. The answer is handed on
   * first, so the build the archive job can still repair is never held back by
   * the ones it cannot.
   */
  test("another recorded build missing fails the run after the answer is out", async () => {
    const outputs = join(await sandbox(), "github-output");
    const run = await status("1", await recordOf("1", "2"), outputs);
    expect(run.exitCode).toBe(1);
    expect(run.stdout).toContain("2 build/2/enshrouded_server.exe");
    expect(await Bun.file(outputs).text()).toBe("needs_archive=false\n");
  });
});
