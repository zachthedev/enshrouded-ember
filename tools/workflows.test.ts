import { describe, expect, test } from "bun:test";
import { readdir } from "node:fs/promises";
import { DESTINATION, NEEDS_ARCHIVE } from "./archive.ts";
import { ARCHIVE_FILES } from "./records.ts";
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
  readonly concurrency?: Record<string, unknown>;
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
 * The deploy key a job checks out with when it has to push to main.
 *
 * @remarks
 * Main requires a pull request, and a deploy key is the only actor that
 * bypasses that rule. A job's own `GITHUB_TOKEN` has no bypass, so a push over
 * HTTPS is refused.
 */
const PUSH_KEY_SECRET = "EMBER_CI_SSH_KEY";

/**
 * Triggers that carry repository secrets on an event a fork can influence.
 *
 * @remarks
 * `pull_request` and `pull_request_target` are the obvious pair. The rest are
 * the ones actually used to smuggle a secret out: a `workflow_run`
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
const ACTION_OWNERS = ["actions", "oven-sh", "Swatinem", "taiki-e"];

const loaded = await workflows();

describe("the workflows", () => {
  test("there is at least one, so an empty directory cannot pass every case", () => {
    expect(loaded.length).toBeGreaterThan(0);
  });

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
      for (const secret of [
        ...READ_SECRETS,
        ...WRITE_SECRETS,
        PUSH_KEY_SECRET,
      ]) {
        expect(
          workflow.text,
          `${name} runs on a fork-influenced trigger and names ${secret}`,
        ).not.toContain(secret);
      }
    },
  );

  /**
   * The deploy key and the push go together in both directions.
   *
   * A job that carries the key and does not push is holding an actor that
   * bypasses the pull request rule on main for no reason. A job that pushes
   * and does not carry it is refused at the push, and where that lands decides
   * how bad it is: the archive job's commit comes after the upload, so a
   * refused push there leaves objects in the bucket that no committed row
   * describes, and every later run re-fetches and re-uploads without ever
   * finishing.
   */
  test("the deploy key and a push go together, in both directions", () => {
    let pushing = 0;
    for (const workflow of loaded) {
      for (const [jobName, job] of Object.entries(workflow.parsed.jobs ?? {})) {
        const text = JSON.stringify(job);
        const steps = Array.isArray(job["steps"]) ? job["steps"] : [];
        const pushes = steps.some((step) => {
          const script = (step as Record<string, unknown>)["run"];
          return typeof script === "string" && /\bgit push\b/.test(script);
        });
        const carriesKey = text.includes(PUSH_KEY_SECRET);
        if (pushes) {
          pushing += 1;
        }
        expect(
          carriesKey,
          pushes
            ? `${workflow.name} job ${jobName} pushes without the deploy key, ` +
                "so main will refuse it"
            : `${workflow.name} job ${jobName} carries the deploy key and ` +
                "pushes nothing",
        ).toBe(pushes);
      }
    }
    expect(pushing, "no job pushes, so this case checked nothing").toBe(2);
  });

  /**
   * The two credentials never sit in the same job.
   *
   * The archive credential can overwrite an archived binary that continuous
   * integration and developers later run, and R2 has no versioning to undo
   * that. The deploy key bypasses the pull request rule on main. A job holding
   * both lets one compromise reach both, and the archive job is the one that
   * runs a container and third-party code.
   */
  test("no job holds both an archive credential and the deploy key", () => {
    let holdingArchive = 0;
    let holdingKey = 0;
    for (const workflow of loaded) {
      for (const [jobName, job] of Object.entries(workflow.parsed.jobs ?? {})) {
        const text = JSON.stringify(job);
        const archive = [...READ_SECRETS, ...WRITE_SECRETS].some((secret) =>
          text.includes(secret),
        );
        const key = text.includes(PUSH_KEY_SECRET);
        if (archive) {
          holdingArchive += 1;
        }
        if (key) {
          holdingKey += 1;
        }
        expect(
          archive && key,
          `${workflow.name} job ${jobName} holds an archive credential and ` +
            "the deploy key, so one compromise reaches the bucket and main",
        ).toBe(false);
      }
    }
    expect(
      holdingArchive,
      "no job holds an archive credential, so this case checked nothing",
    ).toBeGreaterThan(0);
    expect(
      holdingKey,
      "no job holds the deploy key, so this case checked nothing",
    ).toBeGreaterThan(0);
  });

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
   * The archive job waits on a maintainer's approval every time it starts, so
   * what starts it has to answer without waiting on one. One job hands on
   * `needs_archive`, and it asks the bucket with the read token alone: no
   * write secret, no deploy key and no environment, so a scheduled run never
   * sits in `waiting`. Every job holding a write secret starts on that answer,
   * and a second job answering would mean something other than the bucket was
   * deciding.
   */
  test("the archive job starts on the bucket's answer, which waits on no approval", () => {
    let checked = 0;
    for (const workflow of loaded) {
      const jobs = workflow.parsed.jobs ?? {};
      const deciders = Object.entries(jobs).filter(([, job]) =>
        JSON.stringify(job["outputs"] ?? {}).includes(NEEDS_ARCHIVE),
      );
      for (const [jobName, job] of Object.entries(jobs)) {
        const text = JSON.stringify(job);
        if (!WRITE_SECRETS.some((secret) => text.includes(secret))) {
          continue;
        }
        checked += 1;
        expect(
          deciders.map(([name]) => name),
          `${workflow.name} job ${jobName} holds a write secret, and exactly ` +
            "one job has to answer needs_archive",
        ).toHaveLength(1);
        const [deciderName, decider] = deciders[0] as [
          string,
          Record<string, unknown>,
        ];
        expect([job["needs"]].flat()).toContain(deciderName);
        expect(
          String(job["if"] ?? ""),
          `${workflow.name} job ${jobName} does not start on ${deciderName}`,
        ).toContain(`needs.${deciderName}.outputs.${NEEDS_ARCHIVE}`);

        const deciderText = JSON.stringify(decider);
        expect(
          decider["environment"],
          `${deciderName} declares an environment, so every scheduled run waits ` +
            "on an approval",
        ).toBeUndefined();
        for (const secret of READ_SECRETS) {
          expect(deciderText, `${deciderName} cannot ask the bucket`).toContain(
            secret,
          );
        }
        for (const secret of [...WRITE_SECRETS, PUSH_KEY_SECRET]) {
          expect(
            deciderText,
            `${deciderName} answers on every scheduled run and holds ${secret}`,
          ).not.toContain(secret);
        }
      }
    }
    expect(
      checked,
      "no job holds a write secret, so this case checked nothing",
    ).toBeGreaterThan(0);
  });

  /**
   * `actions/checkout` writes the job token into the checkout unless it is
   * told not to. A job that pushes needs it there. A job that does not is
   * running a container, a package install and third-party code beside a
   * credential it never uses, and in the archive job that credential sits
   * next to one that can overwrite an archived binary.
   *
   * Scoped to a workflow that holds a credential of its own, which is where
   * one compromise reaches two of them.
   */
  test("a checkout keeps the job token only where a push needs it", () => {
    let checked = 0;
    for (const workflow of loaded) {
      const secrets = [...READ_SECRETS, ...WRITE_SECRETS, PUSH_KEY_SECRET];
      if (!secrets.some((secret) => workflow.text.includes(secret))) {
        continue;
      }
      for (const [jobName, job] of Object.entries(workflow.parsed.jobs ?? {})) {
        const steps = Array.isArray(job["steps"]) ? job["steps"] : [];
        const pushes = steps.some((step) => {
          const script = (step as Record<string, unknown>)["run"];
          return typeof script === "string" && /\bgit push\b/.test(script);
        });
        for (const step of steps) {
          const uses = (step as Record<string, unknown>)["uses"];
          if (
            typeof uses !== "string" ||
            !uses.startsWith("actions/checkout")
          ) {
            continue;
          }
          checked += 1;
          const options = ((step as Record<string, unknown>)["with"] ??
            {}) as Record<string, unknown>;
          expect(
            options["persist-credentials"],
            `${workflow.name} job ${jobName} checks out ` +
              (pushes
                ? "for a push, so the token has to stay"
                : "and leaves the job token in the workspace"),
          ).toBe(pushes ? undefined : false);
        }
      }
    }
    expect(
      checked,
      "no job checks the repository out, so this case checked nothing",
    ).toBeGreaterThan(0);
  });

  /**
   * A job that waits for a maintainer's approval holds whatever concurrency
   * group its run belongs to for the whole wait. At the workflow level that
   * group covers the watcher too, so nothing reads Steam until somebody
   * clicks, and a build that moves in the meantime is past its manifest by the
   * time the fetch runs.
   *
   * Every group therefore sits on the job that needs it. A job that pushes
   * needs one, because two pushers racing the same record is the other way
   * this file loses work.
   */
  test("a workflow with an approval gate keeps concurrency on the jobs", () => {
    let checked = 0;
    for (const workflow of loaded) {
      const jobs = Object.entries(workflow.parsed.jobs ?? {});
      if (!jobs.some(([, job]) => job["environment"] !== undefined)) {
        continue;
      }
      checked += 1;
      expect(
        workflow.parsed.concurrency,
        `${workflow.name} holds a workflow-level concurrency group while one ` +
          "of its jobs waits for an approval",
      ).toBeUndefined();
      for (const [jobName, job] of jobs) {
        const steps = Array.isArray(job["steps"]) ? job["steps"] : [];
        const pushes = steps.some((step) => {
          const script = (step as Record<string, unknown>)["run"];
          return typeof script === "string" && /\bgit push\b/.test(script);
        });
        if (!pushes && job["environment"] === undefined) {
          continue;
        }
        expect(
          (job["concurrency"] as { group?: string } | undefined)?.group,
          `${workflow.name} job ${jobName} pushes or waits for an approval ` +
            "with no concurrency group of its own",
        ).toEqual(expect.any(String));
      }
    }
    expect(
      checked,
      "no workflow has an approval gate, so this case checked nothing",
    ).toBeGreaterThan(0);
  });

  /**
   * A job gated on another job's outputs is skipped when that job goes red,
   * because GitHub applies `success()` where no status function is written.
   * The watch job reads the client application, which steers nothing and is
   * allowed to fail the run, so the archive path has to say `!cancelled()` or
   * a client-side failure stops the archive while the message names only the
   * client.
   */
  test("every job gated on another job's outputs says !cancelled()", () => {
    let checked = 0;
    for (const workflow of loaded) {
      for (const [jobName, job] of Object.entries(workflow.parsed.jobs ?? {})) {
        const gate = String(job["if"] ?? "");
        if (!/needs\.[A-Za-z0-9_-]+\.(outputs|result)/.test(gate)) {
          continue;
        }
        checked += 1;
        expect(
          gate,
          `${workflow.name} job ${jobName} is gated on another job and takes ` +
            "the implicit success(), so an unrelated failure skips it",
        ).toContain("!cancelled()");
      }
    }
    expect(
      checked,
      "no job is gated on another job, so this case checked nothing",
    ).toBeGreaterThan(0);
  });

  /**
   * `--unattested` records a build whose directory carries no evidence of
   * which manifest it came from. That is a hand path, taken by a person who
   * knows what the bytes are. Continuous integration fetches, so it always has
   * the evidence, and a row it writes is never on anyone's word.
   */
  test("no workflow records a build on the caller's word", () => {
    for (const workflow of loaded) {
      expect(
        workflow.text,
        `${workflow.name} passes --unattested, so a row it writes rests on ` +
          "whoever typed the gid",
      ).not.toContain("--unattested");
    }
  });

  /**
   * `archive.json` is the one place the destination is written down, and this
   * is what keeps it the one place.
   *
   * This repository already carries a case asserting that the copies of the
   * commit scope list agree, and it exists because that drift is what actually
   * happens. Rather than add another instance of the same problem and another
   * case to watch it, the destination has exactly one reader:
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
   * A probe exists for the few minutes its branch does, and then the branch is
   * deleted. Its filename says so, and this is what makes that structural
   * rather than a matter of intent: it can only be reached by pushing a
   * `probe/` branch, so it cannot fire on the default branch and cannot fire
   * on a timer.
   *
   * It also uploads nothing. A probe is exactly where somebody would add an
   * artifact to see the output, and what these fetch out of Steam is a Keen
   * binary that must never leave the runner.
   */
  /**
   * No probe exists most of the time, so the rule is applied by a function and
   * the function is exercised against a document written here. A `test.each`
   * over an empty list reports zero cases and guards nothing, which is the
   * shape this suite exists to avoid.
   */
  test.each(loaded.filter((one) => one.name.startsWith("probe-")))(
    "$name can only run on a probe branch, and uploads nothing",
    (workflow: LoadedWorkflow) => {
      expect(disposableProbeProblems(workflow.parsed)).toEqual([]);
    },
  );

  test("the disposable-probe rule catches what it is for", () => {
    const onProbeBranch = {
      on: { push: { branches: ["probe/**"] } },
      jobs: { probe: { steps: [{ run: "echo hello" }] } },
    };
    expect(disposableProbeProblems(onProbeBranch)).toEqual([]);

    expect(
      disposableProbeProblems({
        ...onProbeBranch,
        on: {
          push: { branches: ["probe/**"] },
          schedule: [{ cron: "0 * * * *" }],
        },
      }),
    ).toContain("it runs on more than a push: push, schedule");

    expect(
      disposableProbeProblems({
        ...onProbeBranch,
        on: { push: { branches: ["main"] } },
      }),
    ).toContain("it can be pushed to main");

    expect(
      disposableProbeProblems({ ...onProbeBranch, on: { push: {} } }),
    ).toContain("it names no branch, so every branch reaches it");

    expect(
      disposableProbeProblems({
        ...onProbeBranch,
        jobs: {
          probe: { steps: [{ uses: "actions/upload-artifact@aaaa" }] },
        },
      }),
    ).toContain("it uploads an artifact: actions/upload-artifact@aaaa");
  });

  /**
   * SteamCMD's file filter has to name the files the digest record expects. It
   * is a semicolon-joined string on a command line, so it cannot come from
   * `ARCHIVE_FILES` directly the way the destination comes from
   * `archive.json`, and a second copy has to exist. This is what keeps the two
   * copies saying the same thing.
   *
   * The drift is quiet in the worst direction: a filter that matches nothing
   * leaves SteamCMD reporting success over an empty directory.
   */
  test("the SteamCMD file filter names exactly the archived files", () => {
    let checked = 0;
    for (const workflow of loaded) {
      const filter = (workflow.parsed.env ?? {})["STEAMCMD_FILE_FILTER"];
      if (filter === undefined) {
        continue;
      }
      checked += 1;
      expect(String(filter).split(";").sort()).toEqual(
        [...ARCHIVE_FILES].sort(),
      );
    }
    expect(checked, "no workflow sets a file filter").toBeGreaterThan(0);
  });

  /**
   * Depot 2278521 declares `oslist windows`, and a Linux SteamCMD selects
   * depots by client platform. Without the flag the fetch dies with "Missing
   * configuration" and writes nothing, measured on ubuntu-latest.
   */
  test("every app_update forces the platform the depot declares", () => {
    let checked = 0;
    for (const workflow of loaded) {
      for (const job of Object.values(workflow.parsed.jobs ?? {})) {
        const steps = Array.isArray(job["steps"]) ? job["steps"] : [];
        for (const step of steps) {
          const script = (step as Record<string, unknown>)["run"];
          if (typeof script !== "string" || !script.includes("+app_update")) {
            continue;
          }
          checked += 1;
          expect(
            script,
            `${workflow.name} runs app_update without forcing the platform`,
          ).toContain("+@sSteamCmdForcePlatformType");
        }
      }
    }
    expect(checked, "no workflow fetches a build").toBeGreaterThan(0);
  });

  /**
   * A tag is resolved when the job runs, so whoever owns it chooses the image
   * on the day rather than on the day the line was written.
   */
  test("every container image is pinned by digest", () => {
    let checked = 0;
    for (const workflow of loaded) {
      // Every image is named by a variable whose name ends in _IMAGE, and the
      // pin lives on that variable. Picking the image out of a `docker run`
      // line by pattern does not survive contact with the line: a volume mount
      // or a --user flag puts other words, and other variables, ahead of it.
      for (const [name, value] of Object.entries(workflow.parsed.env ?? {})) {
        if (!name.endsWith("_IMAGE")) {
          continue;
        }
        checked += 1;
        expect(
          String(value),
          `${workflow.name} sets ${name} to an image that is not pinned by digest`,
        ).toMatch(/@sha256:[0-9a-f]{64}$/);
      }

      for (const line of workflow.text.split("\n")) {
        // A comment naming the command is prose about it, not a run of it.
        if (/^\s*#/.test(line) || !/\bdocker\s+run\b/.test(line)) {
          continue;
        }
        expect(
          line,
          `${workflow.name} runs a container without naming a pinned *_IMAGE ` +
            `variable: ${line.trim()}`,
        ).toMatch(/\$\{?[A-Z0-9_]*_IMAGE\}?/);
      }
    }
    expect(checked, "no workflow names a container image").toBeGreaterThan(0);
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
   * somebody else's code. zizmor's `unpinned-uses` passes that step, because
   * its reference is a commit.
   *
   * The list is checked in the other direction too. An owner left on it after
   * its last action is gone is an allowance nothing uses, and it is the one a
   * repointed step would pass through.
   */
  test("every action comes from a known owner, and every known owner is used", () => {
    const used = new Set<string>();
    for (const workflow of loaded) {
      for (const reference of actionReferences(workflow.parsed)) {
        if (reference.startsWith("./")) {
          continue;
        }
        const owner = reference.split("/")[0] as string;
        used.add(owner);
        expect(
          ACTION_OWNERS,
          `${workflow.name} uses ${reference}, whose owner is not on the list`,
        ).toContain(owner);
      }
    }
    expect(
      ACTION_OWNERS.filter((owner) => !used.has(owner)),
      "an owner on the list has no action left in any workflow",
    ).toEqual([]);
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
   * The reader has to see an action whatever style it is written in. A
   * line-anchored text match cannot, which is why the reader parses the
   * document.
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

/** A file under `.github`, read as text. */
async function githubText(relative: string): Promise<string> {
  return Bun.file(
    fileURLToPath(new URL(`../.github/${relative}`, import.meta.url)),
  ).text();
}

/** A YAML file under `.github`, parsed. */
async function githubYaml(relative: string): Promise<unknown> {
  return Bun.YAML.parse(await githubText(relative));
}

/**
 * A value's own key, or `undefined` when the value is not an object that
 * carries it.
 */
function field(value: unknown, key: string): unknown {
  if (typeof value !== "object" || value === null) {
    return undefined;
  }
  return (value as Record<string, unknown>)[key];
}

/**
 * The inline zizmor ignore comments this repository has decided on, as
 * `file:audit`. Both answer `artipacked` on a checkout whose job pushes over
 * SSH, which needs the key to stay in the checkout.
 */
const ALLOWED_ZIZMOR_IGNORES = [
  "build-watch.yml:artipacked",
  "build-watch.yml:artipacked",
];

describe("the GitHub configuration", () => {
  /**
   * actionlint is a Go program, and neither runner image puts a Go on `PATH`
   * that builds it. The gate job installs one with `actions/setup-go` before
   * the step that reads `.github/go-tools`, at an exact release, because a
   * range resolves at run time and a resolved release is under no cooldown.
   * That step has to stop on a failed install, or the gate reports the tool
   * missing one step later and names the wrong cause.
   */
  test("the Go tools install under an exact Go release", () => {
    const ci = loaded.find((one) => one.name === "ci.yml");
    expect(ci, "there is no ci.yml").toBeDefined();
    const steps = field(ci?.parsed.jobs?.["gate"], "steps");
    expect(Array.isArray(steps), "the gate job lists no steps").toBe(true);
    const list = steps as unknown[];

    const setup = list.findIndex((step) =>
      String(field(step, "uses") ?? "").startsWith("actions/setup-go@"),
    );
    const install = list.findIndex((step) =>
      String(field(step, "run") ?? "").includes(".github/go-tools"),
    );
    expect(setup, "the gate job does not use actions/setup-go").not.toBe(-1);
    expect(install, "no gate step installs from .github/go-tools").not.toBe(-1);
    expect(
      setup,
      "setup-go runs after the step that needs its Go",
    ).toBeLessThan(install);

    const options = field(list[setup], "with");
    expect(
      field(options, "go-version"),
      "setup-go takes something other than one exact release",
    ).toMatch(/^\d+\.\d+\.\d+$/);
    expect(
      field(options, "cache"),
      "setup-go caches on a go.sum this repository does not have",
    ).toBe(false);
    expect(
      String(field(list[install], "run")),
      "the install step does not stop on a failed go install",
    ).toContain("$LASTEXITCODE -ne 0");
  });

  /**
   * Dependabot's cooldown is the wait between a version being published and a
   * pull request proposing it, and an ecosystem with no cooldown block waits
   * for nothing. zizmor's `dependabot-cooldown` refuses a cooldown shorter than
   * `.github/zizmor.yml` sets, and passes a block removed entirely, so this
   * reads every entry.
   */
  test("every dependabot ecosystem carries a cooldown zizmor can hold", async () => {
    const zizmor = await githubYaml("zizmor.yml");
    const threshold = field(
      field(field(field(zizmor, "rules"), "dependabot-cooldown"), "config"),
      "days",
    );
    expect(
      typeof threshold,
      ".github/zizmor.yml sets no dependabot-cooldown threshold in days",
    ).toBe("number");

    const updates = field(await githubYaml("dependabot.yml"), "updates");
    expect(Array.isArray(updates), "dependabot.yml lists no updates").toBe(
      true,
    );
    const short: string[] = [];
    for (const update of updates as unknown[]) {
      const ecosystem = String(field(update, "package-ecosystem"));
      const days = field(field(update, "cooldown"), "default-days");
      if (typeof days !== "number") {
        short.push(`${ecosystem} carries no cooldown default-days`);
      } else if (days < (threshold as number)) {
        short.push(`${ecosystem} waits ${days} days`);
      }
    }
    expect(short).toEqual([]);
  });

  /**
   * An inline `zizmor: ignore[...]` comment answers a finding with no review
   * beyond the diff that adds it, and a rule set to ignore or disable in
   * `.github/zizmor.yml` answers every finding of that audit. Each inline
   * answer has to be on the allowlist above, and the configuration may only
   * set thresholds.
   *
   * zizmor matches this exact spelling, one space and all, and skips an empty
   * entry between commas.
   */
  test("zizmor answers only the findings this repository allows", async () => {
    const github = fileURLToPath(new URL("../.github/", import.meta.url));
    const found: string[] = [];
    for (const relative of await readdir(github, { recursive: true })) {
      const path = join(github, relative);
      const stat = await Bun.file(path).stat();
      if (stat.isDirectory()) {
        continue;
      }
      const text = await Bun.file(path).text();
      const file = relative.split(/[\\/]/).pop() as string;
      for (const match of text.matchAll(/zizmor: ignore\[([^\]]*)\]/g)) {
        for (const audit of (match[1] as string).split(",")) {
          if (audit.trim() !== "") {
            found.push(`${file}:${audit.trim()}`);
          }
        }
      }
    }
    expect(found.sort()).toEqual([...ALLOWED_ZIZMOR_IGNORES].sort());

    const rules = field(await githubYaml("zizmor.yml"), "rules");
    const answering = Object.entries(
      (rules ?? {}) as Record<string, unknown>,
    ).filter(
      ([, rule]) =>
        field(rule, "ignore") !== undefined ||
        field(rule, "disable") !== undefined,
    );
    expect(
      answering.map(([name]) => name),
      ".github/zizmor.yml ignores or disables an audit outright",
    ).toEqual([]);
  });

  /**
   * The Bun release lives in `.bun-version` and nowhere else. setup-bun reads
   * it through `bun-version-file`, and a job that fetches Bun by hand builds
   * its download URL out of a shell variable that the same script sets from
   * that file. A workflow that writes a release down itself holds a second
   * copy, which the next bump leaves behind.
   *
   * A step that names the pin file and fetches something else is the shape
   * this reads for: the release in the URL has to be a variable, and that
   * variable has to be set from `.bun-version` in the same script.
   *
   * setup-bun falls back to `package.json`, and then to the newest release,
   * when the file names nothing it can read, so the file has to hold one exact
   * release.
   */
  test("every Bun release a workflow uses is read from .bun-version", async () => {
    const pin = (
      await Bun.file(
        fileURLToPath(new URL("../.bun-version", import.meta.url)),
      ).text()
    ).trim();
    expect(pin, ".bun-version holds what is not one exact release").toMatch(
      /^\d+\.\d+\.\d+$/,
    );

    let setups = 0;
    let fetches = 0;
    for (const workflow of loaded) {
      for (const [path, value] of keyedValues(workflow.parsed)) {
        const key = path[path.length - 1] as string;
        if (!/bun/i.test(key) || !/version/i.test(key)) {
          continue;
        }
        expect(
          key === "bun-version-file" && value === ".bun-version",
          `${workflow.name} sets ${path.join(".")} to ${String(value)} itself`,
        ).toBe(true);
      }
      expect(
        wholeVersionIn(workflow.text, pin),
        `${workflow.name} writes the release .bun-version pins`,
      ).toBe(false);

      for (const job of Object.values(workflow.parsed.jobs ?? {})) {
        const steps = Array.isArray(job["steps"]) ? job["steps"] : [];
        for (const step of steps) {
          const uses = String(field(step, "uses") ?? "");
          const script = String(field(step, "run") ?? "");
          if (uses.startsWith("oven-sh/setup-bun@")) {
            setups += 1;
            expect(
              field(field(step, "with"), "bun-version-file"),
              `${workflow.name} runs setup-bun without the pin file`,
            ).toBe(".bun-version");
          }
          if (script.includes("releases/download/bun-v")) {
            fetches += 1;
            expect(
              script,
              `${workflow.name} fetches a release it writes down itself`,
            ).not.toMatch(/bun-v\d/);
            const release = /releases\/download\/bun-v([^/"']+)\//.exec(script);
            const variable = /^\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?$/.exec(
              release?.[1] ?? "",
            );
            expect(
              variable?.[1],
              `${workflow.name} fetches bun-v${release?.[1]}, which is no ` +
                "shell variable this case can follow",
            ).toBeDefined();
            const name = variable?.[1] ?? "";
            expect(
              new RegExp(`(^|\\n)\\s*${name}=[^\\n]*\\.bun-version`).test(
                script,
              ),
              `${workflow.name} fetches bun-v$${name} and never sets it from ` +
                "the pin file",
            ).toBe(true);
          }
        }
      }
    }
    expect(setups, "no workflow runs setup-bun").toBeGreaterThan(0);
    expect(fetches, "no workflow fetches Bun by hand").toBeGreaterThan(0);
  });

  /**
   * The watcher opens a build issue from a script, and a person opens the
   * same issue from the form. Both have to read as one document, so the
   * script prints the form's title, its label, every field label in order,
   * an option of each dropdown it answers, and every checkbox in order. A
   * field added, renamed or reworded in the form turns this red until the
   * script says the same thing.
   */
  test("the build issue the watcher opens matches the form a person fills", async () => {
    const form = await githubYaml("ISSUE_TEMPLATE/new-keen-build.yml");
    const fields = (field(form, "body") as unknown[]).filter(
      (one) => field(one, "type") !== "markdown",
    );
    expect(fields.length, "the form holds no field").toBeGreaterThan(0);

    const watch = loaded.find((one) => one.name === "build-watch.yml");
    expect(watch, "there is no build-watch.yml").toBeDefined();
    const steps = Object.values(watch?.parsed.jobs ?? {}).flatMap((job) =>
      Array.isArray(job["steps"]) ? (job["steps"] as unknown[]) : [],
    );
    const opener = steps.find(
      (step) => field(step, "name") === "Open the build issue",
    );
    expect(opener, "no step opens the build issue").toBeDefined();
    const script = String(field(opener, "run") ?? "");

    const title = String(field(form, "title"));
    expect(script, "the issue title is not the form's").toContain(
      `--title "${title}`,
    );
    for (const label of field(form, "labels") as string[]) {
      expect(script, `the issue does not carry the ${label} label`).toContain(
        `--label ${label}`,
      );
    }

    const sections = issueSections(printedText(script));
    expect(
      sections.map((section) => section.heading),
      "the issue's headings are not the form's field labels, in order",
    ).toEqual(
      fields.map((one) => String(field(field(one, "attributes"), "label"))),
    );
    for (const [index, section] of sections.entries()) {
      const attributes = field(fields[index], "attributes");
      const type = field(fields[index], "type");
      if (type === "dropdown") {
        expect(
          (field(attributes, "options") as unknown[]).map(String),
          `the issue answers "${section.heading}" with an option the form lacks`,
        ).toContain(section.content);
      }
      if (type === "checkboxes") {
        const labels = (field(attributes, "options") as unknown[]).map(
          (option) => String(field(option, "label")),
        );
        const printed = section.content
          .split("\n")
          .map((line) => line.replace(/^- \[ \] /, ""));
        expect(
          printed,
          `the issue's "${section.heading}" list is not the form's, in order`,
        ).toEqual(labels);
      }
    }
  });

  /** The script reader takes the shapes the opener prints in. */
  test("the printed text of a script joins its printf formats", () => {
    const script = [
      "{",
      "  printf '### A\\n\\n%s\\n\\n' \"$X\"",
      "  printf -- '- [ ] `b` c\\n'",
      "  echo 'ignored'",
      "} > body.md",
    ].join("\n");
    expect(printedText(script)).toBe("### A\n\n%s\n\n- [ ] `b` c\n");
    expect(
      issueSections("### A\n\none\n\n### B\n\n- [ ] x\n- [ ] y\n"),
    ).toEqual([
      { heading: "A", content: "one" },
      { heading: "B", content: "- [ ] x\n- [ ] y" },
    ]);
  });
});

/**
 * Every value in a parsed document with the path of keys that leads to it.
 *
 * @param value - The document, or any part of it.
 * @param path - The keys that led to `value`.
 * @returns One entry per keyed value, at every depth.
 */
function keyedValues(
  value: unknown,
  path: readonly string[] = [],
): [string[], unknown][] {
  if (typeof value !== "object" || value === null) {
    return [];
  }
  const found: [string[], unknown][] = [];
  for (const [key, child] of Object.entries(value)) {
    const childPath = [...path, key];
    if (!Array.isArray(value)) {
      found.push([childPath, child]);
    }
    found.push(...keyedValues(child, childPath));
  }
  return found;
}

/**
 * Whether `text` holds `version` whole, so that one release is not read
 * inside a longer one.
 *
 * @param text - The text to search.
 * @param version - The release, as dot-separated numbers.
 * @returns True when no digit or dotted digit continues it on either side.
 */
function wholeVersionIn(text: string, version: string): boolean {
  const escaped = version.replaceAll(".", "\\.");
  return new RegExp(`(?<![\\d.])${escaped}(?![\\d]|\\.\\d)`).test(text);
}

/**
 * The text a script's `printf` calls print, with their format strings joined
 * in order and `\n` read as a line break. The values the formats take stay
 * as `%s`.
 *
 * @param script - A step's `run` script.
 * @returns What the formats print, arguments aside.
 */
function printedText(script: string): string {
  const formats = [...script.matchAll(/printf(?:\s+--)?\s+'([^']*)'/g)].map(
    (match) => (match[1] as string).replaceAll("\\n", "\n"),
  );
  return formats.join("");
}

/**
 * The `### ` sections of an issue body, as each heading and the text under
 * it with the surrounding blank lines dropped.
 *
 * @param body - The issue body.
 * @returns One entry per heading, in order.
 */
function issueSections(body: string): { heading: string; content: string }[] {
  return body
    .split(/^### /m)
    .slice(1)
    .map((section) => {
      const [heading, ...rest] = section.split("\n");
      return { heading: heading as string, content: rest.join("\n").trim() };
    });
}

/**
 * Every way a probe workflow fails to be disposable.
 *
 * @remarks
 * A probe exists for the few minutes its branch does, and then the branch is
 * deleted. What makes that structural rather than a matter of intent is the
 * trigger: reachable only by pushing a `probe/` branch, so it cannot fire on
 * the default branch and cannot fire on a timer.
 *
 * It uploads nothing either. A probe is exactly where somebody would add an
 * artifact to see the output, and what these fetch out of Steam is a Keen
 * binary that must never leave the runner.
 *
 * @param workflow - The parsed document.
 * @returns One sentence per problem, empty when the probe is disposable.
 */
function disposableProbeProblems(workflow: Workflow): string[] {
  const problems: string[] = [];
  const triggers = Object.keys(workflow.on ?? {});
  if (triggers.length !== 1 || triggers[0] !== "push") {
    problems.push(`it runs on more than a push: ${triggers.join(", ")}`);
  }
  const push = (workflow.on ?? {})["push"] as
    { branches?: string[] } | undefined;
  const branches = push?.branches ?? [];
  if (branches.length === 0) {
    problems.push("it names no branch, so every branch reaches it");
  }
  for (const branch of branches) {
    if (!branch.startsWith("probe/")) {
      problems.push(`it can be pushed to ${branch}`);
    }
  }
  for (const reference of actionReferences(workflow)) {
    if (reference.includes("upload-artifact")) {
      problems.push(`it uploads an artifact: ${reference}`);
    }
  }
  return problems;
}

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
