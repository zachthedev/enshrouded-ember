import { describe, expect, test } from "bun:test";
import { readdir } from "node:fs/promises";
import { DESTINATION } from "./archive.ts";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

/**
 * What a workflow file holds, as far as these cases read it.
 *
 * @remarks
 * Only the keys under test are named. A workflow carries far more, and a
 * shape that named all of it would go stale the first time GitHub adds a key.
 */
interface Workflow {
  readonly on?: Record<string, unknown>;
  readonly env?: Record<string, unknown>;
  readonly jobs?: Record<string, Record<string, unknown>>;
}

/** One workflow, with the text it was parsed from. */
interface LoadedWorkflow {
  readonly name: string;
  readonly text: string;
  readonly parsed: Workflow;
}

/** The directory the workflows live in, resolved from this module. */
const workflowDir = fileURLToPath(
  new URL("../.github/workflows/", import.meta.url),
);

/** Every workflow in the repository. */
async function workflows(): Promise<LoadedWorkflow[]> {
  const names = (await readdir(workflowDir)).filter((name) =>
    /\.ya?ml$/.test(name),
  );
  const loaded: LoadedWorkflow[] = [];
  for (const name of names) {
    const text = await Bun.file(join(workflowDir, name)).text();
    loaded.push({ name, text, parsed: Bun.YAML.parse(text) as Workflow });
  }
  return loaded;
}

/** The secrets that reach the archive, split by what they can do to it. */
const READ_SECRETS = [
  "R2_ARCHIVE_READ_ACCESS_KEY_ID",
  "R2_ARCHIVE_READ_SECRET_ACCESS_KEY",
];
const WRITE_SECRETS = [
  "R2_ARCHIVE_WRITE_ACCESS_KEY_ID",
  "R2_ARCHIVE_WRITE_SECRET_ACCESS_KEY",
];

/**
 * Triggers that carry repository secrets on an event a fork can influence.
 *
 * @remarks
 * `pull_request` and `pull_request_target` are the obvious pair. The other
 * three are the ones actually used to smuggle a secret out: a `workflow_run`
 * chained to a workflow that does run on `pull_request` fires from the default
 * branch with full access to secrets, and anyone can comment on a public
 * repository's issue or discussion.
 */
const FORK_INFLUENCED = [
  "pull_request",
  "pull_request_target",
  "workflow_run",
  "issue_comment",
  "discussion_comment",
];

/**
 * The owners every action in this repository may come from.
 *
 * @remarks
 * A commit pin binds the bytes at `owner/repo@sha` and says nothing about
 * which `owner/repo` was written down. Repointing a pinned step to
 * `evilcorp/setup-bun` at some 40-hex commit of their own keeps the pin and
 * changes the code that runs.
 */
const ACTION_OWNERS = [
  "actions",
  "oven-sh",
  "Swatinem",
  "taiki-e",
  "EmbarkStudios",
];

const loaded = await workflows();

describe("the workflows", () => {
  test("there is at least one, so an empty directory cannot pass every case", () => {
    expect(loaded.length).toBeGreaterThan(0);
  });

  test.each(loaded.map((workflow) => [workflow.name] as const))(
    "%s parses as YAML with jobs",
    (name) => {
      const workflow = loaded.find(
        (one) => one.name === name,
      ) as LoadedWorkflow;
      expect(Object.keys(workflow.parsed.jobs ?? {}).length).toBeGreaterThan(0);
    },
  );

  /**
   * GitHub passes no secrets to a workflow triggered from a forked repository,
   * so an archive step on a pull request has no credential at all. Both
   * repositories are public, so that is the ordinary case rather than the
   * exception.
   */
  test.each(loaded.map((workflow) => [workflow.name] as const))(
    "%s reaches no archive credential if a pull request can trigger it",
    (name) => {
      const workflow = loaded.find(
        (one) => one.name === name,
      ) as LoadedWorkflow;
      const triggers = Object.keys(workflow.parsed.on ?? {});
      if (!triggers.some((trigger) => FORK_INFLUENCED.includes(trigger))) {
        return;
      }
      for (const secret of [...READ_SECRETS, ...WRITE_SECRETS]) {
        expect(
          workflow.text,
          `${name} runs on a fork-influenced trigger and names ${secret}`,
        ).not.toContain(secret);
      }
    },
  );

  /**
   * R2 has no object versioning and no Object Lock, so an overwrite is final.
   * The environment is the only control GitHub offers that a pull request
   * cannot reach, even by landing a workflow change.
   */
  test("every job holding a write secret declares the archive-write environment", () => {
    for (const workflow of loaded) {
      for (const [jobName, job] of Object.entries(workflow.parsed.jobs ?? {})) {
        const text = JSON.stringify(job);
        const holdsWrite = WRITE_SECRETS.some((secret) =>
          text.includes(secret),
        );
        if (!holdsWrite) {
          continue;
        }
        expect(
          job["environment"],
          `${workflow.name} job ${jobName} holds a write secret`,
        ).toBe("archive-write");
      }
    }
  });

  /**
   * The case above asks each job, so it sees nothing when the secret is not in
   * a job at all. Hoisting both write secrets into the workflow-level `env:`
   * block and deleting every `environment:` line gives every job in the file
   * the credential with no gate, and reads as tidying.
   *
   * Counting mentions in the whole file against mentions inside gated jobs is
   * what catches that: the hoist makes the first number two and the second
   * zero.
   */
  test("a write secret appears nowhere but inside a gated job", () => {
    for (const workflow of loaded) {
      const gated = Object.values(workflow.parsed.jobs ?? {})
        .filter((job) => job["environment"] === "archive-write")
        .map((job) => JSON.stringify(job))
        .join("\n");
      for (const secret of WRITE_SECRETS) {
        const inFile = workflow.text.split(secret).length - 1;
        const inGatedJobs = gated.split(secret).length - 1;
        expect(
          inGatedJobs,
          `${workflow.name} names ${secret} ${inFile} times and only ` +
            `${inGatedJobs} of those are inside a job declaring ` +
            "environment: archive-write",
        ).toBe(inFile);
      }
    }
  });

  /**
   * `archive.json` is the one place the destination is written down, and this
   * is what keeps it the one place.
   *
   * This repository already carries a case asserting that three copies of the
   * commit scope list agree, and it exists because that drift is what actually
   * happens. Rather than add a fourth instance of the same problem and a
   * fourth case to watch it, the destination has exactly one reader:
   * `archive.ts` builds the endpoint and names the bucket, and no workflow
   * mentions either value. This refuses the restatement that would start the
   * drift.
   */
  test("no workflow restates the archive destination", () => {
    for (const workflow of loaded) {
      for (const [field, value] of Object.entries(DESTINATION)) {
        expect(
          workflow.text,
          `${workflow.name} restates the ${field} from archive.json, which is ` +
            "the one place it is written down",
        ).not.toContain(value);
      }
    }
  });

  /**
   * A tag is resolved when the job runs, so whoever owns it chooses the image
   * on the day rather than on the day the line was written.
   */
  test("every container image is pinned by digest", () => {
    let checked = 0;
    for (const workflow of loaded) {
      for (const image of workflow.text.matchAll(
        /docker\s+run\b[^\n]*?(?:\$\{?(\w+)\}?|([\w./-]+(?::[\w.-]+|@sha256:[0-9a-f]{64})))/g,
      )) {
        const viaVariable = image[1];
        const literal = image[2];
        // `docker run "$IMAGE"` is the shape this file uses, so the pin lives
        // in the env block the variable names.
        const reference =
          viaVariable === undefined
            ? literal
            : (workflow.parsed.env ?? {})[viaVariable];
        checked += 1;
        expect(
          String(reference),
          `${workflow.name} runs a container that is not pinned by digest`,
        ).toMatch(/@sha256:[0-9a-f]{64}$/);
      }
    }
    expect(checked, "no workflow runs a container").toBeGreaterThan(0);
  });

  /**
   * Every artifact this pipeline downloads at run time is executed. The digest
   * check is the only thing binding the bytes, and deleting the line is a
   * one-character-looking change.
   */
  test("every downloaded artifact is checked against a digest before it is used", () => {
    let checked = 0;
    for (const workflow of loaded) {
      for (const job of Object.values(workflow.parsed.jobs ?? {})) {
        const steps = Array.isArray(job["steps"]) ? job["steps"] : [];
        for (const step of steps) {
          const script = (step as Record<string, unknown>)["run"];
          if (typeof script !== "string" || !script.includes("curl ")) {
            continue;
          }
          checked += 1;
          expect(
            script,
            `${workflow.name} downloads a file and does not check it:\n${script}`,
          ).toContain("sha256sum --check --strict");
        }
      }
    }
    expect(checked, "no workflow downloads anything").toBeGreaterThan(0);
  });

  /**
   * A commit pin binds the bytes and not the name above them, so a step moved
   * to another owner at one of their own commits stays pinned and runs
   * somebody else's code.
   */
  test("every action comes from a known owner", () => {
    for (const workflow of loaded) {
      for (const reference of actionReferences(workflow.parsed)) {
        if (reference.startsWith("./")) {
          continue;
        }
        const owner = reference.split("/")[0] as string;
        expect(
          ACTION_OWNERS,
          `${workflow.name} uses ${reference}, whose owner is not on the list`,
        ).toContain(owner);
      }
    }
  });

  /**
   * Bun runs the root `prepare` on install, which points `core.hooksPath` at
   * `.githooks`. That pre-push hook runs the whole gate, and no runner here
   * installs the tool belt, so the gate exits non-zero and the push is
   * refused. A workflow that installs and pushes has to keep the two apart.
   */
  test("a workflow that installs and pushes does not run install scripts", () => {
    for (const workflow of loaded) {
      const scripts = Object.values(workflow.parsed.jobs ?? {})
        .flatMap((job) => (Array.isArray(job["steps"]) ? job["steps"] : []))
        .map((step) => (step as Record<string, unknown>)["run"])
        .filter((script): script is string => typeof script === "string");
      const pushes = scripts.some((script) => /\bgit push\b/.test(script));
      const installs = scripts.filter((script) =>
        /\bbun(x)? install\b/.test(script),
      );
      if (!pushes || installs.length === 0) {
        continue;
      }
      for (const install of installs) {
        expect(
          install,
          `${workflow.name} pushes and installs, so the install has to skip ` +
            "the root prepare script that installs the pre-push hook",
        ).toContain("--ignore-scripts");
      }
    }
  });

  /**
   * A tag can be retargeted by its owner with no pull request and no cooldown.
   * A commit cannot.
   *
   * @remarks
   * Read out of the parsed document rather than matched against the text. A
   * line-anchored pattern walks straight past YAML flow style, so
   * `- { uses: actions/checkout@v7 }` would be an unpinned tag that passes.
   */
  test("every action is pinned to a commit", () => {
    let checked = 0;
    for (const workflow of loaded) {
      for (const reference of actionReferences(workflow.parsed)) {
        checked += 1;
        expect(reference, `${workflow.name} uses ${reference}`).toMatch(
          /@[0-9a-f]{40}$/,
        );
      }
    }
    expect(
      checked,
      "no workflow uses an action, so this case checked nothing",
    ).toBeGreaterThan(0);
  });

  /**
   * The reader has to see an action whatever style it is written in, which is
   * what the text-matching version it replaced could not do.
   */
  test("the reader finds an action written in flow style", () => {
    const parsed = Bun.YAML.parse(
      [
        "on: push",
        "jobs:",
        "  block:",
        "    steps:",
        "      - uses: actions/checkout@aaaa",
        "  flow:",
        "    steps:",
        "      - { uses: actions/setup-node@bbbb }",
        "  reusable:",
        "    uses: ./.github/workflows/other.yml@cccc",
      ].join("\n"),
    ) as Workflow;
    expect(actionReferences(parsed).sort()).toEqual([
      "./.github/workflows/other.yml@cccc",
      "actions/checkout@aaaa",
      "actions/setup-node@bbbb",
    ]);
  });
});

/**
 * Every action a workflow runs, from its jobs and from their steps.
 *
 * @param workflow - The parsed document.
 * @returns One reference per `uses` the document carries.
 */
function actionReferences(workflow: Workflow): string[] {
  const found: string[] = [];
  for (const job of Object.values(workflow.jobs ?? {})) {
    // A job-level `uses` is a reusable workflow, pinned the same way.
    const jobUses = job["uses"];
    if (typeof jobUses === "string") {
      found.push(jobUses);
    }
    const steps = job["steps"];
    if (!Array.isArray(steps)) {
      continue;
    }
    for (const step of steps) {
      const uses = (step as Record<string, unknown>)["uses"];
      if (typeof uses === "string") {
        found.push(uses);
      }
    }
  }
  return found;
}
