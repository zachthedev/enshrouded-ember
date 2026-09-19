/**
 * Records what Steam advertises for one application, and says whether it moved.
 *
 * @remarks
 * Input is the raw output of SteamCMD `app_info_print <appid>` under an
 * anonymous login. Valve serves that directly, so the record depends on no
 * scraper and no third-party mirror.
 *
 * A row is appended only when a build id or a manifest gid moves. The PICS
 * change number advances for unrelated reasons, so it rides along for
 * provenance and never triggers anything.
 *
 * SteamCMD prints a banner before the KeyValues block and its command help
 * after it, so the block is cut out by line before a parser sees it.
 *
 * @example
 * ```sh
 * bun run tools/watch-builds.ts 2278520 appinfo-2278520.txt
 * ```
 */

import { appendFile } from "node:fs/promises";
import { parseArgs } from "node:util";
import * as VDF from "vdf-parser";
import { z } from "zod";
import {
  appendRecord,
  BranchTable,
  ManifestTable,
  readRecords,
  STEAM_BUILDS_PATH,
  SteamBuildRecord,
} from "./records.ts";

/**
 * ///////////////////////////////////////////////
 * Reading SteamCMD output
 * ///////////////////////////////////////////////
 */

/** A parsed KeyValues node: a leaf string, a table, or a repeated key. */
type VdfNode = string | { [key: string]: VdfNode | VdfNode[] };

/** A table of KeyValues children, which is what every branch of the walk sees. */
type VdfTable = Record<string, VdfNode | VdfNode[]>;

/** What one application's block says about its builds. */
export interface AppInfo {
  readonly changeNumber: string | null;
  readonly branches: Record<string, string>;
  readonly manifests: Record<string, string>;
}

/** A refusal that names what was wrong with the SteamCMD output. */
export class AppInfoError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "AppInfoError";
  }
}

/**
 * Cut the KeyValues block for one application out of SteamCMD's output.
 *
 * @param text - Raw SteamCMD stdout, with either line ending.
 * @param appId - The application whose block to take.
 * @returns The block, from the quoted app id to its closing brace.
 * @throws {@link AppInfoError} When no complete block for that application is
 * present. That is what a cold SteamCMD prints when the caller did not run
 * `app_info_update 1` first.
 */
export function sliceAppBlock(text: string, appId: number): string {
  const lines = text.split("\n").map((line) => line.replace(/\r$/, ""));
  const opening = `"${appId}"`;
  const start = lines.findIndex((line) => line.trimEnd() === opening);
  if (start === -1) {
    throw new AppInfoError(
      `SteamCMD printed no block for app ${appId}. A cold SteamCMD prints an ` +
        "empty block, so the caller runs app_info_update 1 first.",
    );
  }
  const end = lines.findIndex(
    (line, index) => index > start && line.trimEnd() === "}",
  );
  if (end === -1) {
    throw new AppInfoError(
      `SteamCMD's block for app ${appId} has no closing brace, so the output ` +
        "was cut short.",
    );
  }
  return lines.slice(start, end + 1).join("\n");
}

/** The children of a node, or nothing when it is a leaf or a repeated key. */
function asTable(node: VdfNode | VdfNode[] | undefined): VdfTable {
  return typeof node === "object" && node !== null && !Array.isArray(node)
    ? node
    : {};
}

/** The string a node carries, or null when it is anything else. */
function asString(node: VdfNode | VdfNode[] | undefined): string | null {
  return typeof node === "string" ? node : null;
}

/**
 * Read one application's build state out of SteamCMD's output.
 *
 * @param text - Raw SteamCMD stdout.
 * @param appId - The application to read.
 * @returns The branches, the depot manifests and the change number.
 * @throws {@link AppInfoError} When the block is absent, carries no branch
 * build id, or carries no depot manifest. An empty answer written as a row
 * would read as a build that moved.
 */
export function parseAppInfo(text: string, appId: number): AppInfo {
  const block = sliceAppBlock(text, appId);
  // Both options are load-bearing and neither is a preference.
  //
  // types: false keeps Valve's values as strings. Conversion rounds a manifest
  // gid, which is larger than Number.MAX_SAFE_INTEGER, into a gid Valve never
  // served.
  //
  // arrayify: true is what keeps the parser off Object.prototype. On a key
  // named `__proto__` the parser finds the slot already occupied and descends
  // into what is there. With arrayify off that is Object.prototype itself, and
  // the block's children are written onto it for the life of the process. With
  // it on, an array is assigned through the setter instead and nothing global
  // is touched. It also turns a repeated key into an array, which the readers
  // below refuse rather than silently taking one of the two.
  const root = VDF.parse<VdfTable>(block, { types: false, arrayify: true });
  const depots = asTable(asTable(root[String(appId)])["depots"]);

  const branches: Record<string, string> = {};
  for (const [name, node] of Object.entries(asTable(depots["branches"]))) {
    const buildId = asString(asTable(node)["buildid"]);
    if (buildId !== null) {
      branches[name] = buildId;
    }
  }

  const manifests: Record<string, string> = {};
  for (const [depotId, node] of Object.entries(depots)) {
    if (!/^\d+$/.test(depotId)) {
      continue;
    }
    const publicManifest = asTable(
      asTable(asTable(node)["manifests"])["public"],
    );
    const gid = asString(publicManifest["gid"]);
    if (gid !== null) {
      manifests[depotId] = gid;
    }
  }

  if (Object.keys(branches).length === 0) {
    throw new AppInfoError(
      `app ${appId} advertises no branch build id, so the block is empty or ` +
        "truncated.",
    );
  }
  if (Object.keys(manifests).length === 0) {
    throw new AppInfoError(
      `app ${appId} advertises no depot manifest, so the block is empty or ` +
        "truncated.",
    );
  }

  // Scoped to this application, because one SteamCMD run can print several.
  const line = new RegExp(
    `^AppID\\s*:\\s*${appId}\\s*,\\s*change number\\s*:\\s*(\\d+)`,
    "m",
  ).exec(text);

  // Checked here rather than at the append, because a run where nothing moved
  // appends no row and still hands these values to a workflow.
  const checked = z
    .object({ branches: BranchTable, manifests: ManifestTable })
    .safeParse({ branches, manifests });
  if (!checked.success) {
    throw new AppInfoError(
      `app ${appId} advertises something this tool will not record: ` +
        z.prettifyError(checked.error),
    );
  }

  return {
    changeNumber: line?.[1] ?? null,
    branches: checked.data.branches,
    manifests: checked.data.manifests,
  };
}

/**
 * ///////////////////////////////////////////////
 * Deciding whether it moved
 * ///////////////////////////////////////////////
 */

/** The fields that decide whether a build actually moved. */
type BuildIdentity = Pick<SteamBuildRecord, "branches" | "manifests">;

/**
 * A stable key for comparing two observations, insensitive to key ordering.
 *
 * @param record - The observation to key.
 * @returns A string that is equal for two observations of the same state.
 */
export function identityKey(record: BuildIdentity): string {
  const sorted = (
    table: Readonly<Record<string, string>>,
  ): [string, string][] =>
    Object.entries(table).sort(([a], [b]) => a.localeCompare(b));
  return JSON.stringify({
    branches: sorted(record.branches),
    manifests: sorted(record.manifests),
  });
}

/**
 * The newest row for one application.
 *
 * @param records - Every row, in file order.
 * @param appId - The application to look for.
 * @returns The last row for that application, or null when it has none.
 */
export function lastRecordFor(
  records: readonly SteamBuildRecord[],
  appId: number,
): SteamBuildRecord | null {
  let found: SteamBuildRecord | null = null;
  for (const record of records) {
    if (record.appId === appId) {
      found = record;
    }
  }
  return found;
}

/** What one run of the watcher concluded. */
export interface WatchResult {
  readonly record: SteamBuildRecord;
  readonly previous: SteamBuildRecord | null;
  readonly changed: boolean;
}

/**
 * Compare an observation against the record, and append it when it differs.
 *
 * @param path - The JSON Lines record.
 * @param record - The observation just taken.
 * @returns What was concluded, and the row it was compared against.
 */
export async function recordIfChanged(
  path: string,
  record: SteamBuildRecord,
): Promise<WatchResult> {
  const existing = await readRecords(path, SteamBuildRecord);
  const previous = lastRecordFor(existing, record.appId);
  const changed =
    previous === null || identityKey(previous) !== identityKey(record);
  if (changed) {
    await appendRecord(path, SteamBuildRecord, record);
  }
  return { record, previous, changed };
}

/** The branch whose build id every downstream job is named for. */
export const PUBLIC_BRANCH = "public";

/**
 * Every branch whose build id differs from the row this one is compared to.
 *
 * @remarks
 * `Object.hasOwn` rather than a lookup, because the previous row's branch table
 * inherits from `Object.prototype`. A branch Steam named `constructor` would
 * otherwise compare against a function and read as unmoved.
 *
 * @param result - What the run concluded.
 * @returns The branch names that moved, which is empty when only a gid did.
 */
export function movedBranches(result: WatchResult): string[] {
  const before = result.previous?.branches ?? {};
  return Object.entries(result.record.branches)
    .filter(
      ([name, buildId]) =>
        !Object.hasOwn(before, name) || before[name] !== buildId,
    )
    .map(([name]) => name);
}

/**
 * ///////////////////////////////////////////////
 * Command line
 * ///////////////////////////////////////////////
 */

/** Dim text, when the terminal takes color. */
function dim(text: string): string {
  return Bun.enableANSIColors ? `[2m${text}[0m` : text;
}

/**
 * The values a workflow step reads, as `name=value` lines.
 *
 * @remarks
 * A step that decides what to do next reads these rather than grepping this
 * command's own prose, which would make the wording load-bearing.
 *
 * Whether the build is archived is not among them. This job holds no archive
 * credential, so it cannot ask the bucket, and `archive status` answers that
 * in a job of its own.
 *
 * @param result - What the run concluded.
 * @param depotId - The depot whose manifest gid to publish, if any. The
 * archive job needs it to name the build it fetches.
 * @returns One line per value, in a fixed order.
 */
export function stepOutputs(
  result: WatchResult,
  depotId: string | undefined,
): string[] {
  const moved = movedBranches(result);
  return [
    `changed=${result.changed}`,
    `app_id=${result.record.appId}`,
    // Both follow the public branch, so both are read together with
    // public_moved and never with branches_moved. A beta branch moving is a
    // real event that names neither of these.
    `build_id=${result.record.branches[PUBLIC_BRANCH] ?? ""}`,
    `manifest_id=${manifestFor(result, depotId)}`,
    `change_number=${result.record.changeNumber ?? ""}`,
    `branches_moved=${moved.join(" ")}`,
    `public_moved=${moved.includes(PUBLIC_BRANCH)}`,
  ];
}

/**
 * The manifest gid for the depot a caller named.
 *
 * @param result - What the run concluded.
 * @param depotId - The depot to look up, if any.
 * @returns The gid, or an empty string when no depot was named.
 * @throws {@link AppInfoError} When a depot was named and carries no public
 * manifest. An empty gid makes the archive job's output directory the archive
 * root rather than one build's, and names a row after nothing.
 */
function manifestFor(result: WatchResult, depotId: string | undefined): string {
  if (depotId === undefined) {
    return "";
  }
  const gid = result.record.manifests[depotId];
  if (gid === undefined) {
    throw new AppInfoError(
      `app ${result.record.appId} advertises no public manifest for depot ` +
        `${depotId}, so there is no build to name.`,
    );
  }
  return gid;
}

/** Append the step outputs to the file a workflow named, when one did. */
async function emitStepOutputs(
  result: WatchResult,
  depotId: string | undefined,
): Promise<void> {
  const path = process.env["GITHUB_OUTPUT"];
  if (path === undefined || path.length === 0) {
    return;
  }
  const lines = stepOutputs(result, depotId);
  // A line break inside a value declares an output name of its own, and GitHub
  // takes the last value for a repeated name, so an injected `changed=false`
  // would win over the real one. Every field these lines are built from is
  // already checked against a schema that refuses a line break. This refuses
  // to write the file at all if that ever stops holding, because the way it
  // fails otherwise is silent.
  for (const line of lines) {
    if (/[\r\n]/.test(line)) {
      throw new AppInfoError(
        "a step output carries a line break, which would declare an output " +
          `name of its own: ${JSON.stringify(line)}`,
      );
    }
  }
  // Appended rather than read and rewritten. The runner hands each step a file
  // that may already hold another command's output, and a rewrite would lose
  // it, or merge into it when it ends without a newline.
  await appendFile(path, `${lines.join("\n")}\n`, "utf8");
}

/** Print what the run concluded. */
function report(result: WatchResult): void {
  console.log("build watch");
  console.log("");
  const glyph = result.changed ? "✓" : "·";
  const verdict = result.changed ? "recorded" : "unchanged";
  console.log(`  ${glyph} app ${result.record.appId} ${verdict}`);
  for (const [name, buildId] of Object.entries(result.record.branches)) {
    const before = result.previous?.branches[name];
    const from =
      before !== undefined && before !== buildId ? ` was ${before}` : "";
    console.log(`    ${dim("branch")} ${name} ${buildId}${from}`);
  }
  for (const [depotId, gid] of Object.entries(result.record.manifests)) {
    const before = result.previous?.manifests[depotId];
    const from = before !== undefined && before !== gid ? ` was ${before}` : "";
    console.log(`    ${dim("depot")} ${depotId} ${gid}${from}`);
  }

  console.log("");
  console.log(
    result.changed
      ? `  one row appended to ${STEAM_BUILDS_PATH}`
      : "  no row appended",
  );
}

if (import.meta.main) {
  const { values, positionals } = parseArgs({
    args: Bun.argv.slice(2),
    allowPositionals: true,
    options: { depot: { type: "string" } },
  });
  const [appIdText, infoPath] = positionals;
  if (appIdText === undefined || infoPath === undefined) {
    console.error(
      "usage: watch-builds.ts <appid> <app_info_print output> [--depot <id>]",
    );
    process.exit(2);
  }
  const appId = Number(appIdText);
  if (!Number.isSafeInteger(appId) || appId <= 0) {
    console.error(`${appIdText} is not a Steam application id`);
    process.exit(2);
  }

  const info = parseAppInfo(await Bun.file(infoPath).text(), appId);
  const result = await recordIfChanged(STEAM_BUILDS_PATH, {
    observedAt: new Date().toISOString(),
    appId,
    changeNumber: info.changeNumber,
    branches: info.branches,
    manifests: info.manifests,
  });
  await emitStepOutputs(result, values.depot);
  report(result);
}
