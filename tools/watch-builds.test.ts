import { afterEach, describe, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readRecords, SteamBuildRecord } from "./records.ts";
import {
  AppInfoError,
  identityKey,
  lastRecordFor,
  movedBranches,
  parseAppInfo,
  recordIfChanged,
  sliceAppBlock,
  stepOutputs,
  type WatchResult,
} from "./watch-builds.ts";

/** Every sandbox this file made, removed once the case ends. */
const sandboxes: string[] = [];

/** A directory of this suite's own, under the temp root the session sets. */
async function sandbox(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), "ember-watch-"));
  sandboxes.push(dir);
  return dir;
}

afterEach(async () => {
  while (sandboxes.length > 0) {
    await rm(sandboxes.pop() as string, { recursive: true, force: true });
  }
});

/**
 * SteamCMD's output around one application's block, trimmed to the parts the
 * watcher reads.
 *
 * @remarks
 * The banner before the block and the command help after it are what make the
 * slice necessary, so both are kept. Indentation is tabs, as SteamCMD writes.
 */
const CAPTURE = [
  "Steam Console Client (c) Valve Corporation - version 1788292693",
  "-- type 'quit' to exit --",
  "Loading Steam API...OK",
  "",
  "Connecting anonymously to Steam Public...OK",
  "AppID : 2278520, change number : 38966542/38966542, last change : Thu Sep 17 14:00:13 2026 ",
  '"2278520"',
  "{",
  '\t"common"',
  "\t{",
  '\t\t"name"\t\t"Enshrouded Dedicated Server"',
  "\t}",
  '\t"depots"',
  "\t{",
  '\t\t"1004"',
  "\t\t{",
  '\t\t\t"manifests"',
  "\t\t\t{",
  '\t\t\t\t"public"',
  "\t\t\t\t{",
  '\t\t\t\t\t"gid"\t\t"7604377918839582995"',
  "\t\t\t\t}",
  "\t\t\t}",
  "\t\t}",
  '\t\t"2278521"',
  "\t\t{",
  '\t\t\t"config"',
  "\t\t\t{",
  '\t\t\t\t"oslist"\t\t"windows"',
  "\t\t\t}",
  '\t\t\t"manifests"',
  "\t\t\t{",
  '\t\t\t\t"public"',
  "\t\t\t\t{",
  '\t\t\t\t\t"gid"\t\t"2174935030716737236"',
  '\t\t\t\t\t"size"\t\t"8756751690"',
  "\t\t\t\t}",
  "\t\t\t}",
  "\t\t}",
  '\t\t"branches"',
  "\t\t{",
  '\t\t\t"public"',
  "\t\t\t{",
  '\t\t\t\t"buildid"\t\t"23178631"',
  '\t\t\t\t"timeupdated"\t\t"1778508351"',
  "\t\t\t}",
  "\t\t}",
  '\t\t"privatebranches"\t\t"1"',
  "\t}",
  "}",
  "ConVars:",
  "Commands:",
  "Unloading Steam API...OK",
].join("\n");

/** What a cold SteamCMD prints when `app_info_update 1` was not called. */
const EMPTY_BLOCK = ['"2278520"', "{", "}", "Commands:"].join("\n");

describe("sliceAppBlock", () => {
  test("the block is cut out of the banner and the command help", () => {
    const block = sliceAppBlock(CAPTURE, 2278520);
    expect(block.startsWith('"2278520"')).toBe(true);
    expect(block.endsWith("\n}")).toBe(true);
    expect(block).not.toContain("Commands:");
    expect(block).not.toContain("Steam Console Client");
  });

  /** A capture taken on Windows arrives with CRLF, and CI reads it on Linux. */
  test("carriage returns do not hide the block", () => {
    const windows = CAPTURE.split("\n").join("\r\n");
    expect(sliceAppBlock(windows, 2278520)).toBe(
      sliceAppBlock(CAPTURE, 2278520),
    );
  });

  test.each([
    ["no block for that application", CAPTURE, 1203620],
    [
      "a block that was cut short",
      CAPTURE.slice(0, CAPTURE.indexOf('"depots"')),
      2278520,
    ],
  ])("%s is refused", (_name, text, appId) => {
    expect(() => sliceAppBlock(text, appId)).toThrow(AppInfoError);
  });
});

describe("parseAppInfo", () => {
  test("the branch, the depot manifests and the change number are read", () => {
    expect(parseAppInfo(CAPTURE, 2278520)).toEqual({
      changeNumber: "38966542",
      branches: { public: "23178631" },
      manifests: {
        "1004": "7604377918839582995",
        "2278521": "2174935030716737236",
      },
    });
  });

  /**
   * A manifest gid is larger than Number.MAX_SAFE_INTEGER. A parser that
   * converts values to numbers returns 2174935030716737200 for this one, which
   * names a manifest Valve never served.
   */
  test("a manifest gid keeps every digit Valve served", () => {
    const gid = parseAppInfo(CAPTURE, 2278520).manifests["2278521"];
    expect(gid).toBe("2174935030716737236");
    expect(Number(gid).toString()).not.toBe(gid);
  });

  /**
   * An empty block recorded as a row reads as every branch and every depot
   * disappearing at once, which is a change, which would archive nothing and
   * open an issue.
   */
  test("an empty block is refused rather than recorded", () => {
    expect(() => parseAppInfo(EMPTY_BLOCK, 2278520)).toThrow(AppInfoError);
  });

  test("a block with branches but no depot manifest is refused", () => {
    const stripped = CAPTURE.replace(/"gid"\t\t"\d+"/g, '"size"\t\t"0"');
    expect(() => parseAppInfo(stripped, 2278520)).toThrow(AppInfoError);
  });

  /** One SteamCMD run can print several applications, each with its own line. */
  test("the change number of another application is not taken", () => {
    const other = CAPTURE.replace(
      "AppID : 2278520, change number : 38966542",
      "AppID : 1203620, change number : 11111111",
    );
    expect(parseAppInfo(other, 2278520).changeNumber).toBeNull();
  });
});

describe("parseAppInfo, against input Valve did not send", () => {
  /**
   * A gid becomes a step output, a DepotDownloader `-dir` argument and an R2
   * object key prefix. It is checked at the boundary rather than at any of
   * those, because a run where nothing moved appends no row and so
   * reaches no later check at all.
   */
  test.each([
    ["path components", "1234567890123456789/../../../evil"],
    ["a leading dot dot", "../2174935030716737236"],
    ["a space", "1234 5678"],
    ["twenty-one digits", "9".repeat(21)],
    ["nothing at all", ""],
  ])("a manifest gid carrying %s is refused", (_name, gid) => {
    const crafted = CAPTURE.replace("2174935030716737236", gid);
    expect(() => parseAppInfo(crafted, 2278520)).toThrow(AppInfoError);
  });

  test.each([
    ["a space", "has space"],
    ["a shell substitution", "$(id)"],
    ["an equals sign", "public=1"],
  ])("a branch named with %s is refused", (_name, branch) => {
    // Anchored on the branches block. The manifests blocks carry a `public`
    // key too, one indent deeper, and renaming one of those would drop a
    // manifest rather than exercise the branch alphabet.
    const crafted = CAPTURE.replace(
      '\n\t\t\t"public"\n',
      `\n\t\t\t"${branch}"\n`,
    );
    expect(crafted).not.toBe(CAPTURE);
    expect(() => parseAppInfo(crafted, 2278520)).toThrow(AppInfoError);
  });

  /**
   * The parser is handed a block whose key is `__proto__`. With `arrayify`
   * off, vdf-parser walks into `Object.prototype` itself and writes the
   * block's children onto it for the life of the process. The option the
   * caller passes is the only thing that stops it.
   */
  test("a __proto__ block leaves Object.prototype alone", () => {
    const polluted = [
      '"2278520"',
      "{",
      '\t"__proto__"',
      "\t{",
      '\t\t"pwned"\t\t"yes"',
      "\t}",
      "}",
    ].join("\n");
    // The block advertises no branch, so a refusal is expected. What is under
    // test is the state of the prototype afterwards, not the refusal.
    expect(() => parseAppInfo(polluted, 2278520)).toThrow(AppInfoError);
    expect(
      ({} as Record<string, unknown>)["pwned"],
      "parsing a __proto__ block wrote onto Object.prototype",
    ).toBeUndefined();
  });
});

describe("identityKey", () => {
  test("key order does not change the identity", () => {
    const one = {
      branches: { public: "1", beta: "2" },
      manifests: { "10": "a", "20": "b" },
    };
    const other = {
      branches: { beta: "2", public: "1" },
      manifests: { "20": "b", "10": "a" },
    };
    expect(identityKey(one)).toBe(identityKey(other));
  });

  test.each([
    [
      "a build id moving",
      { branches: { public: "2" }, manifests: { "10": "a" } },
    ],
    ["a gid moving", { branches: { public: "1" }, manifests: { "10": "b" } }],
    [
      "a branch appearing",
      { branches: { public: "1", beta: "9" }, manifests: { "10": "a" } },
    ],
  ])("%s changes the identity", (_name, moved) => {
    const base = { branches: { public: "1" }, manifests: { "10": "a" } };
    expect(identityKey(base)).not.toBe(identityKey(moved));
  });
});

describe("recordIfChanged", () => {
  /** A row of the shape the file holds, for a case to bend one field of. */
  const observation = {
    observedAt: "2026-09-18T00:00:00.000Z",
    appId: 2278520,
    changeNumber: "38966542",
    branches: { public: "23178631" },
    manifests: { "2278521": "2174935030716737236" },
  };

  test("the first observation of an application is recorded", async () => {
    const path = join(await sandbox(), "steam-builds.jsonl");
    const result = await recordIfChanged(path, observation);
    expect(result.changed).toBe(true);
    expect(result.previous).toBeNull();
    expect(await readRecords(path, SteamBuildRecord)).toHaveLength(1);
  });

  test("the same state observed again appends nothing", async () => {
    const path = join(await sandbox(), "steam-builds.jsonl");
    await recordIfChanged(path, observation);
    const again = await recordIfChanged(path, {
      ...observation,
      observedAt: "2026-09-18T01:00:00.000Z",
    });
    expect(again.changed).toBe(false);
    expect(await readRecords(path, SteamBuildRecord)).toHaveLength(1);
  });

  /**
   * The change number advances for reasons unrelated to builds. It moved on
   * 2026-09-17 while app 2278520 sat on the build it has carried since May.
   */
  test("a change number moving on its own appends nothing", async () => {
    const path = join(await sandbox(), "steam-builds.jsonl");
    await recordIfChanged(path, observation);
    const again = await recordIfChanged(path, {
      ...observation,
      changeNumber: "39000000",
    });
    expect(again.changed).toBe(false);
  });

  test.each([
    ["a build id", { branches: { public: "23178632" } }],
    [
      "a depot manifest gid",
      { manifests: { "2278521": "9999999999999999999" } },
    ],
  ])("%s moving is recorded", async (_name, moved) => {
    const path = join(await sandbox(), "steam-builds.jsonl");
    await recordIfChanged(path, observation);
    const again = await recordIfChanged(path, { ...observation, ...moved });
    expect(again.changed).toBe(true);
    expect(await readRecords(path, SteamBuildRecord)).toHaveLength(2);
  });

  /** Two applications share one file and are compared to their own last row. */
  test("one application's move does not look like another's", async () => {
    const path = join(await sandbox(), "steam-builds.jsonl");
    const client = {
      ...observation,
      appId: 1203620,
      branches: { public: "23966345" },
      manifests: { "1203621": "5368460047775505242" },
    };
    await recordIfChanged(path, observation);
    await recordIfChanged(path, client);
    const server = await recordIfChanged(path, observation);
    expect(server.changed).toBe(false);
    expect(server.previous?.appId).toBe(2278520);
  });
});

describe("lastRecordFor", () => {
  test("the newest row for the application is taken, not the newest row", () => {
    const rows: SteamBuildRecord[] = [
      {
        observedAt: "2026-09-18T00:00:00.000Z",
        appId: 2278520,
        changeNumber: null,
        branches: { public: "1" },
        manifests: { "10": "a" },
      },
      {
        observedAt: "2026-09-18T01:00:00.000Z",
        appId: 2278520,
        changeNumber: null,
        branches: { public: "2" },
        manifests: { "10": "a" },
      },
      {
        observedAt: "2026-09-18T02:00:00.000Z",
        appId: 1203620,
        changeNumber: null,
        branches: { public: "9" },
        manifests: { "20": "b" },
      },
    ];
    expect(lastRecordFor(rows, 2278520)?.branches["public"]).toBe("2");
    expect(lastRecordFor(rows, 999)).toBeNull();
  });
});

describe("movedBranches", () => {
  /**
   * A client change must never open an issue claiming a new server build.
   * Hotfix 42 was client-only, and the server build has not moved since May.
   */
  test("a run where only a gid moved names no branch", () => {
    const record = {
      observedAt: "2026-09-18T00:00:00.000Z",
      appId: 2278520,
      changeNumber: "39000000",
      branches: { public: "23178631" },
      manifests: { "2278521": "9999999999999999999" },
    };
    const previous = {
      ...record,
      manifests: { "2278521": "2174935030716737236" },
    };
    expect(movedBranches({ record, previous, changed: true })).toEqual([]);
  });

  test("a first observation names every branch, because none was known", () => {
    const record = {
      observedAt: "2026-09-18T00:00:00.000Z",
      appId: 2278520,
      changeNumber: null,
      branches: { public: "23178631" },
      manifests: { "2278521": "2174935030716737236" },
    };
    expect(movedBranches({ record, previous: null, changed: true })).toEqual([
      "public",
    ]);
  });

  /**
   * zod's record output inherits from `Object.prototype`, so a branch Steam
   * named `constructor` compares against a function rather than against
   * nothing and reads as unmoved.
   */
  test("a branch named for a property of Object is seen to be new", () => {
    const record = {
      observedAt: "2026-09-18T00:00:00.000Z",
      appId: 2278520,
      changeNumber: null,
      branches: { constructor: "23178631" },
      manifests: { "2278521": "2174935030716737236" },
    };
    const previous = { ...record, branches: {} };
    expect(movedBranches({ record, previous, changed: true })).toEqual([
      "constructor",
    ]);
  });
});

describe("stepOutputs", () => {
  /** The observation the workflow's own gates are computed from. */
  const record = {
    observedAt: "2026-09-18T00:00:00.000Z",
    appId: 2278520,
    changeNumber: "38966542",
    branches: { public: "23178631", beta: "23000001" },
    manifests: { "2278521": "2174935030716737236" },
  };

  /** The outputs as a lookup, which is how a workflow reads them. */
  const outputs = (
    result: WatchResult,
    depot: string | undefined,
  ): Record<string, string> =>
    Object.fromEntries(
      stepOutputs(result, depot).map((line) => {
        const at = line.indexOf("=");
        return [line.slice(0, at), line.slice(at + 1)];
      }),
    );

  /**
   * A beta branch moving is Keen's ordinary publishing behavior. `build_id`
   * and `manifest_id` both follow `public`, so a job gated on any branch
   * moving would fetch a manifest that is already archived and open an issue
   * naming the build that did not move.
   */
  test("a beta branch moving does not read as the public branch moving", () => {
    const previous = { ...record, branches: { ...record.branches, beta: "1" } };
    const out = outputs({ record, previous, changed: true }, "2278521");
    expect(out["branches_moved"]).toBe("beta");
    expect(out["public_moved"]).toBe("false");
    expect(out["build_id"]).toBe("23178631");
  });

  test("the public branch moving says so", () => {
    const previous = {
      ...record,
      branches: { ...record.branches, public: "23000000" },
    };
    const out = outputs({ record, previous, changed: true }, "2278521");
    expect(out["public_moved"]).toBe("true");
    expect(out["build_id"]).toBe("23178631");
    expect(out["manifest_id"]).toBe("2174935030716737236");
  });

  /**
   * An empty `manifest_id` reaches DepotDownloader as `-manifest ""` and makes
   * `-dir` the archive root rather than one build's directory.
   */
  test("a depot with no public manifest is refused rather than emitted empty", () => {
    expect(() =>
      stepOutputs({ record, previous: null, changed: true }, "9999999"),
    ).toThrow(AppInfoError);
  });

  test("no depot named emits an empty manifest id", () => {
    const out = outputs({ record, previous: null, changed: true }, undefined);
    expect(out["manifest_id"]).toBe("");
  });

  /**
   * The watch job holds no archive credential, so it cannot know what the
   * bucket holds, and an answer it gave would come from a record that pins
   * digests rather than one that proves an upload. The archive job reads that
   * answer from the job that asks the bucket, and from nowhere else.
   */
  test("the watcher hands on no answer about the archive", () => {
    const out = outputs({ record, previous: null, changed: true }, "2278521");
    expect(Object.keys(out).filter((name) => /archive/.test(name))).toEqual([]);
  });

  /**
   * GitHub takes the last value for a repeated output name, so a line break in
   * any value sets an output of its own and wins.
   */
  test("no output line can carry a line break", () => {
    for (const line of stepOutputs(
      { record, previous: null, changed: true },
      "2278521",
    )) {
      expect(line).not.toMatch(/[\r\n]/);
    }
  });
});
