import { afterEach, describe, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  archiveClient,
  compareDigests,
  credentials,
  digestFile,
  DESTINATION,
  digestRow,
  DuplicateRowError,
  MissingCredentialError,
  objectKey,
} from "./archive.ts";
import { appendRecord, BuildDigestRecord } from "./records.ts";

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
  archivedAt: "2026-09-18",
  files: {
    "enshrouded_server.exe": { bytes: 3, sha256: ABC_SHA256 },
    "enshrouded_server.kfc": { bytes: 3, sha256: ABC_SHA256 },
  },
};

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
