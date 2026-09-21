import { describe, expect, test } from 'bun:test';
import { readdir } from 'node:fs/promises';
import { DESTINATION, NEEDS_ARCHIVE } from './archive.ts';
import { ARCHIVE_FILES } from './records.ts';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';

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
const workflowDir = fileURLToPath(new URL('../.github/workflows/', import.meta.url));

/** Every workflow in the repository. */
async function workflows(): Promise<LoadedWorkflow[]> {
  const names = (await readdir(workflowDir)).filter((name) => /\.ya?ml$/.test(name));
  const loaded: LoadedWorkflow[] = [];
  for (const name of names) {
    const text = await Bun.file(join(workflowDir, name)).text();
    loaded.push({ name, text, parsed: Bun.YAML.parse(text) as Workflow });
  }
  return loaded;
}

/** The secrets that reach the archive, split by what they can do to it. */
const READ_SECRETS = ['R2_ARCHIVE_READ_ACCESS_KEY_ID', 'R2_ARCHIVE_READ_SECRET_ACCESS_KEY'];
const WRITE_SECRETS = ['R2_ARCHIVE_WRITE_ACCESS_KEY_ID', 'R2_ARCHIVE_WRITE_SECRET_ACCESS_KEY'];

/**
 * The deploy key a job checks out with when it has to push to main.
 *
 * @remarks
 * Main requires a pull request, and a deploy key is the only actor that
 * bypasses that rule. A job's own `GITHUB_TOKEN` has no bypass, so a push over
 * HTTPS is refused.
 */
const PUSH_KEY_SECRET = 'EMBER_CI_SSH_KEY';

/**
 * The deployment environments known to carry no required reviewer.
 *
 * @remarks
 * A reviewer list is a GitHub setting rather than a file, so nothing in this
 * repository can read one. GitHub is the authority and this list is a claim
 * about it, which is the weakness of every case that reads it: a reviewer
 * added to one of these names turns an unattended run into a wait and leaves
 * the gate green.
 *
 * What the list does close is the direction that changes under a diff. An
 * environment absent from it is read as gating on a person, so a job wired to
 * a new environment is refused until somebody states which kind it is.
 *
 * Reviewers aside, every environment carries a deployment branch policy naming
 * `main`. A workflow on any other branch is refused the secrets the
 * environment holds, which a repository-level secret cannot do.
 */
const UNGATED_ENVIRONMENTS = ['archive-read', 'digest-push'];

/**
 * How far apart two scheduled workflows come due, in minutes.
 *
 * @remarks
 * Two hourly crons can be at most thirty apart, so this is a floor rather than
 * a target. What it has to clear is the longest a job can run, because a job
 * still running when the other comes due is the case the spacing exists to
 * avoid. The case below asserts that, so raising a job's timeout past this
 * number turns the suite red rather than quietly shortening the margin.
 */
const MINUTES_APART = 20;

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
const FORK_INFLUENCED = ['pull_request', 'pull_request_target', 'workflow_run', 'issue_comment', 'discussion_comment'];

/**
 * The owners every action in this repository may come from.
 *
 * @remarks
 * A commit pin binds the bytes at `owner/repo@sha` and says nothing about
 * which `owner/repo` was written down. Repointing a pinned step to
 * `evilcorp/setup-bun` at some 40-hex commit of their own keeps the pin and
 * changes the code that runs.
 */
const ACTION_OWNERS = ['actions', 'jdx', 'oven-sh', 'Swatinem'];

const loaded = await workflows();

describe('the workflows', () => {
  test('there is at least one, so an empty directory cannot pass every case', () => {
    expect(loaded.length).toBeGreaterThan(0);
  });

  /**
   * GitHub passes no secrets to a workflow triggered from a forked repository,
   * so an archive step on a pull request has no credential at all. Both
   * repositories are public, so that is the ordinary case rather than the
   * exception.
   */
  test.each(loaded.map((workflow) => [workflow.name] as const))(
    '%s reaches no archive credential if a pull request can trigger it',
    (name) => {
      const workflow = loaded.find((one) => one.name === name) as LoadedWorkflow;
      const triggers = Object.keys(workflow.parsed.on ?? {});
      if (!triggers.some((trigger) => FORK_INFLUENCED.includes(trigger))) {
        return;
      }
      for (const secret of [...READ_SECRETS, ...WRITE_SECRETS, PUSH_KEY_SECRET]) {
        expect(workflow.text, `${name} runs on a fork-influenced trigger and names ${secret}`).not.toContain(secret);
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
  test('the deploy key and a push go together, in both directions', () => {
    let pushing = 0;
    for (const workflow of loaded) {
      for (const [jobName, job] of Object.entries(workflow.parsed.jobs ?? {})) {
        const text = JSON.stringify(job);
        const steps = Array.isArray(job['steps']) ? job['steps'] : [];
        const pushes = steps.some((step) => {
          const script = (step as Record<string, unknown>)['run'];
          return typeof script === 'string' && /\bgit push\b/.test(script);
        });
        const carriesKey = text.includes(PUSH_KEY_SECRET);
        if (pushes) {
          pushing += 1;
        }
        expect(
          carriesKey,
          pushes
            ? `${workflow.name} job ${jobName} pushes without the deploy key, ` + 'so main will refuse it'
            : `${workflow.name} job ${jobName} carries the deploy key and ` + 'pushes nothing',
        ).toBe(pushes);
      }
    }
    // Held to an exact number in both directions. Zero means the rule above
    // read nothing, and any other change means a pusher was added or lost
    // without anybody weighing what it does to main.
    expect(pushing, 'the jobs that push are not the ones this rule was written against').toBe(3);
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
  test('no job holds both an archive credential and the deploy key', () => {
    let holdingArchive = 0;
    let holdingKey = 0;
    for (const workflow of loaded) {
      for (const [jobName, job] of Object.entries(workflow.parsed.jobs ?? {})) {
        const text = JSON.stringify(job);
        const archive = [...READ_SECRETS, ...WRITE_SECRETS].some((secret) => text.includes(secret));
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
            'the deploy key, so one compromise reaches the bucket and main',
        ).toBe(false);
      }
    }
    expect(holdingArchive, 'no job holds an archive credential, so this case checked nothing').toBeGreaterThan(0);
    expect(holdingKey, 'no job holds the deploy key, so this case checked nothing').toBeGreaterThan(0);
  });

  /**
   * R2 has no object versioning and no Object Lock, so an overwrite is final.
   * The environment is the only control GitHub offers that a pull request
   * cannot reach, even by landing a workflow change.
   */
  test('every job holding a write secret declares the archive-write environment', () => {
    for (const workflow of loaded) {
      for (const [jobName, job] of Object.entries(workflow.parsed.jobs ?? {})) {
        const text = JSON.stringify(job);
        const holdsWrite = WRITE_SECRETS.some((secret) => text.includes(secret));
        if (!holdsWrite) {
          continue;
        }
        expect(job['environment'], `${workflow.name} job ${jobName} holds a write secret`).toBe('archive-write');
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
  test('a write secret appears nowhere but inside a gated job', () => {
    for (const workflow of loaded) {
      const gated = Object.values(workflow.parsed.jobs ?? {})
        .filter((job) => job['environment'] === 'archive-write')
        .map((job) => JSON.stringify(job))
        .join('\n');
      for (const secret of WRITE_SECRETS) {
        const inFile = workflow.text.split(secret).length - 1;
        const inGatedJobs = gated.split(secret).length - 1;
        expect(
          inGatedJobs,
          `${workflow.name} names ${secret} ${inFile} times and only ` +
            `${inGatedJobs} of those are inside a job declaring ` +
            'environment: archive-write',
        ).toBe(inFile);
      }
    }
  });

  /**
   * The archive job waits on a maintainer's approval every time it starts, so
   * what starts it has to answer without waiting on one. One job hands on
   * `needs_archive`, and it asks the bucket with the read token alone: no
   * write secret and no deploy key, so a scheduled run never sits in
   * `waiting`. Every job holding a write secret starts on that answer, and a
   * second job answering would mean something other than the bucket was
   * deciding.
   *
   * The deciding job declares an environment, for the deployment branch policy
   * that refuses its token to a workflow on any other branch. What this case
   * reads is whether that environment gates on a person, because the wait is
   * the thing an hourly unattended check cannot afford.
   */
  test("the archive job starts on the bucket's answer, which waits on no approval", () => {
    let checked = 0;
    for (const workflow of loaded) {
      const jobs = workflow.parsed.jobs ?? {};
      const deciders = Object.entries(jobs).filter(([, job]) =>
        JSON.stringify(job['outputs'] ?? {}).includes(NEEDS_ARCHIVE),
      );
      for (const [jobName, job] of Object.entries(jobs)) {
        const text = JSON.stringify(job);
        if (!WRITE_SECRETS.some((secret) => text.includes(secret))) {
          continue;
        }
        checked += 1;
        expect(
          deciders.map(([name]) => name),
          `${workflow.name} job ${jobName} holds a write secret, and exactly ` + 'one job has to answer needs_archive',
        ).toHaveLength(1);
        const [deciderName, decider] = deciders[0] as [string, Record<string, unknown>];
        expect([job['needs']].flat()).toContain(deciderName);
        expect(String(job['if'] ?? ''), `${workflow.name} job ${jobName} does not start on ${deciderName}`).toContain(
          `needs.${deciderName}.outputs.${NEEDS_ARCHIVE}`,
        );

        const deciderText = JSON.stringify(decider);
        expect(
          waitsForApproval(decider),
          `${deciderName} declares environment ` +
            `${String(decider['environment'])}, which is not on the ` +
            'reviewer-free list, so every scheduled run waits on an approval',
        ).toBe(false);
        for (const secret of READ_SECRETS) {
          expect(deciderText, `${deciderName} cannot ask the bucket`).toContain(secret);
        }
        for (const secret of [...WRITE_SECRETS, PUSH_KEY_SECRET]) {
          expect(deciderText, `${deciderName} answers on every scheduled run and holds ${secret}`).not.toContain(
            secret,
          );
        }
      }
    }
    expect(checked, 'no job holds a write secret, so this case checked nothing').toBeGreaterThan(0);
  });

  /**
   * `UNGATED_ENVIRONMENTS` is where the case above can be silenced. Adding the
   * write environment to it reads as one more name on a list and turns that
   * case green with the approval gate gone, so this refuses it at the list.
   *
   * The list is read in the other direction too. A name left on it after its
   * last job is gone is an allowance nothing uses, and it is the one a job
   * wired later passes through unread. `ACTION_OWNERS` carries the same guard,
   * for the same reason.
   */
  test('the reviewer-free list is used in full and gates no write secret', () => {
    const declared = new Set<string>();
    let checked = 0;
    for (const workflow of loaded) {
      for (const [jobName, job] of Object.entries(workflow.parsed.jobs ?? {})) {
        const environment = job['environment'];
        if (typeof environment === 'string') {
          declared.add(environment);
        }
        const text = JSON.stringify(job);
        if (!WRITE_SECRETS.some((secret) => text.includes(secret))) {
          continue;
        }
        checked += 1;
        expect(
          waitsForApproval(job),
          `${workflow.name} job ${jobName} holds a write secret and starts ` +
            `without an approval, under environment ${String(environment)}`,
        ).toBe(true);
      }
    }
    expect(
      UNGATED_ENVIRONMENTS.filter((name) => !declared.has(name)),
      'an environment on the reviewer-free list is declared by no job',
    ).toEqual([]);
    expect(checked, 'no job holds a write secret, so this case checked nothing').toBeGreaterThan(0);
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
  test('a checkout keeps the job token only where a push needs it', () => {
    let checked = 0;
    for (const workflow of loaded) {
      const secrets = [...READ_SECRETS, ...WRITE_SECRETS, PUSH_KEY_SECRET];
      if (!secrets.some((secret) => workflow.text.includes(secret))) {
        continue;
      }
      for (const [jobName, job] of Object.entries(workflow.parsed.jobs ?? {})) {
        const steps = Array.isArray(job['steps']) ? job['steps'] : [];
        const pushes = steps.some((step) => {
          const script = (step as Record<string, unknown>)['run'];
          return typeof script === 'string' && /\bgit push\b/.test(script);
        });
        for (const step of steps) {
          const uses = (step as Record<string, unknown>)['uses'];
          if (typeof uses !== 'string' || !uses.startsWith('actions/checkout')) {
            continue;
          }
          checked += 1;
          const options = ((step as Record<string, unknown>)['with'] ?? {}) as Record<string, unknown>;
          expect(
            options['persist-credentials'],
            `${workflow.name} job ${jobName} checks out ` +
              (pushes ? 'for a push, so the token has to stay' : 'and leaves the job token in the workspace'),
          ).toBe(pushes ? undefined : false);
        }
      }
    }
    expect(checked, 'no job checks the repository out, so this case checked nothing').toBeGreaterThan(0);
  });

  /**
   * A job that waits for a maintainer's approval holds whatever concurrency
   * group its run belongs to for the whole wait. At the workflow level that
   * group covers the watcher too, so nothing reads Steam until somebody
   * clicks, and a build that moves in the meantime is past its manifest by the
   * time the fetch runs.
   *
   * The group therefore sits on the job that waits rather than on the
   * workflow. A job whose environment is on `UNGATED_ENVIRONMENTS` starts
   * immediately, so it holds no group for a wait and needs none on this
   * account. Reading the bare presence of an environment instead would demand
   * a group of the hourly bucket check, which serializes a job that races
   * nothing.
   *
   * What a pushing job needs is the case below, which covers every workflow
   * rather than only one with an approval gate.
   */
  test('a workflow with an approval gate keeps concurrency off the workflow', () => {
    let checked = 0;
    for (const workflow of loaded) {
      const jobs = Object.entries(workflow.parsed.jobs ?? {});
      if (!jobs.some(([, job]) => waitsForApproval(job))) {
        continue;
      }
      checked += 1;
      expect(
        workflow.parsed.concurrency,
        `${workflow.name} holds a workflow-level concurrency group while one ` + 'of its jobs waits for an approval',
      ).toBeUndefined();
      for (const [jobName, job] of jobs) {
        if (!waitsForApproval(job)) {
          continue;
        }
        expect(
          (job['concurrency'] as { group?: string } | undefined)?.group,
          `${workflow.name} job ${jobName} waits for an approval with no ` + 'concurrency group of its own',
        ).toEqual(expect.any(String));
      }
    }
    expect(checked, 'no workflow has an approval gate, so this case checked nothing').toBeGreaterThan(0);
  });

  /**
   * Every job that pushes holds a concurrency group, and the jobs committing
   * one file hold the same group.
   *
   * Two runs appending to the end of one record and pushing it is the race.
   * The loser's `git pull --rebase` hits a content conflict rather than
   * replaying, and a `run:` block is `bash -e`, so the step dies before the
   * push. A group per job serializes each against itself and neither against
   * the other, which is no help when the two sit in different workflows.
   *
   * A group name is repository-scoped, so one name is the whole mechanism.
   * That is GitHub's documented behavior rather than something measured here.
   *
   * Read from `git add`, because the path a job stages is what says which
   * record it writes. Jobs committing different files rebase past each other
   * cleanly and need no shared name.
   */
  test('jobs that commit one record hold one concurrency group', () => {
    const byPath = new Map<string, Map<string, string>>();
    let checked = 0;
    for (const workflow of loaded) {
      for (const [jobName, job] of Object.entries(workflow.parsed.jobs ?? {})) {
        const scripts = (Array.isArray(job['steps']) ? job['steps'] : [])
          .map((step) => (step as Record<string, unknown>)['run'])
          .filter((script): script is string => typeof script === 'string');
        if (!scripts.some((script) => /\bgit push\b/.test(script))) {
          continue;
        }
        checked += 1;
        const group = (job['concurrency'] as { group?: string } | undefined)?.group;
        expect(
          group,
          `${workflow.name} job ${jobName} pushes with no concurrency group, ` +
            'so nothing holds it apart from another run',
        ).toEqual(expect.any(String));
        for (const script of scripts) {
          for (const staged of script.matchAll(/\bgit add\s+(\S+)/g)) {
            const path = staged[1] as string;
            const holders = byPath.get(path) ?? new Map<string, string>();
            holders.set(String(group), `${workflow.name} job ${jobName}`);
            byPath.set(path, holders);
          }
        }
      }
    }
    for (const [path, holders] of byPath) {
      expect(
        [...holders.keys()],
        `the jobs committing ${path} hold different concurrency groups, so ` +
          `neither is held apart from the other: ` +
          [...holders].map(([group, job]) => `${job} under ${group}`).join(', '),
      ).toHaveLength(1);
    }
    expect(checked, 'no job pushes, so this case checked nothing').toBeGreaterThan(0);
  });

  /**
   * A job gated on another job's outputs is skipped when that job goes red,
   * because GitHub applies `success()` where no status function is written.
   * The watch job can fail after it has written the manifest id, and a build
   * detected and then dropped is one the bucket never gets: the archiving
   * window closes when Keen's next build rolls the manifest.
   */
  test("every job gated on another job's outputs says !cancelled()", () => {
    let checked = 0;
    for (const workflow of loaded) {
      for (const [jobName, job] of Object.entries(workflow.parsed.jobs ?? {})) {
        const gate = String(job['if'] ?? '');
        if (!/needs\.[A-Za-z0-9_-]+\.(outputs|result)/.test(gate)) {
          continue;
        }
        checked += 1;
        expect(
          gate,
          `${workflow.name} job ${jobName} is gated on another job and takes ` +
            'the implicit success(), so an unrelated failure skips it',
        ).toContain('!cancelled()');
      }
    }
    expect(checked, 'no job is gated on another job, so this case checked nothing').toBeGreaterThan(0);
  });

  /**
   * Each Steam application is read by one workflow, and that workflow reads no
   * other.
   *
   * GitHub titles the notification for a failed run after the workflow and
   * names nothing inside it, so a workflow covering two applications sends one
   * sentence for two problems with unrelated causes. Keeping them apart is
   * what makes the title say which one.
   *
   * Read from the SteamCMD command that names an application rather than from
   * the environment. An id written straight into a `docker run` line carries
   * no environment key at all, and reading keys alone passes exactly the edit
   * this rule exists to refuse: a second read put back into the server
   * pipeline with the number inline.
   *
   * So the argument has to be an environment reference, which is also what
   * keeps one workflow's id in one place.
   */
  test('each Steam application is read by one workflow, which reads no other', () => {
    const seen = new Map<string, string>();
    let checked = 0;
    for (const workflow of loaded) {
      const env = (workflow.parsed.env ?? {}) as Record<string, unknown>;
      const read = new Map<string, string>();
      for (const { job, step } of jobSteps(workflow.parsed)) {
        const script = String(step['run'] ?? '');
        for (const call of script.matchAll(/\+app_(?:info_print|update)\s+(\S+)/g)) {
          const argument = call[1] as string;
          const reference = /^"?\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?"?$/.exec(argument);
          expect(
            reference?.[1],
            `${workflow.name} job ${job} names application ${argument} in the ` +
              'command itself, so no environment key holds it and nothing can ' +
              'tell which application this workflow reads',
          ).toBeDefined();
          const value = env[reference?.[1] ?? ''];
          expect(
            value,
            `${workflow.name} job ${job} reads $${String(reference?.[1])}, ` + 'which its environment does not set',
          ).toBeDefined();
          read.set(String(value), `${job} reads $${String(reference?.[1])}`);
        }
      }
      if (read.size === 0) {
        continue;
      }
      checked += 1;
      expect(
        [...read.keys()],
        `${workflow.name} reads more than one Steam application, so one run ` +
          `covers two with unrelated causes of failure: ` +
          [...read].map(([id, where]) => `${id} where ${where}`).join(', '),
      ).toHaveLength(1);
      const id = [...read.keys()][0] as string;
      const already = seen.get(id);
      expect(already, `${workflow.name} reads application ${id}, and so does ${String(already)}`).toBeUndefined();
      seen.set(id, workflow.name);
    }
    expect(checked, 'no workflow reads a Steam application, so this case checked nothing').toBeGreaterThan(0);
  });

  /**
   * Two scheduled workflows come due far enough apart to stay out of each
   * other's way.
   *
   * The shared concurrency group is what actually serializes the two record
   * writers, so this is the second line: two runs spaced further apart than a
   * job can occupy never contend for the group, and a run held for a group can
   * be cancelled by the next one arriving.
   *
   * `MINUTES_APART` is the spacing, and it is the longest budget either
   * record-writing job declares. Closer than its own timeout and a job can
   * still be running when the other comes due.
   *
   * A minute that is not one number comes due more than once an hour and can
   * land on any other, so it is refused rather than measured.
   */
  test('scheduled workflows come due a stated distance apart', () => {
    // The spacing has to clear the longest a job can run, or the two can
    // overlap while both crons are punctual.
    for (const workflow of loaded) {
      for (const [jobName, job] of Object.entries(workflow.parsed.jobs ?? {})) {
        const budget = job['timeout-minutes'];
        if (typeof budget !== 'number') {
          continue;
        }
        const pushes = (Array.isArray(job['steps']) ? job['steps'] : []).some((step) => {
          const script = (step as Record<string, unknown>)['run'];
          return typeof script === 'string' && /\bgit push\b/.test(script);
        });
        if (!pushes) {
          continue;
        }
        expect(
          budget,
          `${workflow.name} job ${jobName} may run for ${budget} minutes, ` +
            'which is longer than the spacing between two schedules',
        ).toBeLessThanOrEqual(MINUTES_APART);
      }
    }

    const minutes = new Map<number, string>();
    let checked = 0;
    for (const workflow of loaded) {
      const schedule = (workflow.parsed.on ?? {})['schedule'];
      if (!Array.isArray(schedule)) {
        continue;
      }
      for (const entry of schedule) {
        const cron = String(field(entry, 'cron') ?? '');
        const minute = cron.split(/\s+/)[0] ?? '';
        checked += 1;
        expect(minute, `${workflow.name} schedules on "${cron}", whose minute is not one number`).toMatch(/^\d{1,2}$/);
        const due = Number(minute);
        for (const [taken, other] of minutes) {
          // Around the hour rather than along it, so minute 5 and minute 58
          // read as seven apart.
          const gap = Math.abs(due - taken);
          expect(
            Math.min(gap, 60 - gap),
            `${workflow.name} comes due at minute ${due} and ${other} at ` +
              `minute ${taken}, which is closer than a job can run`,
          ).toBeGreaterThanOrEqual(MINUTES_APART);
        }
        minutes.set(due, workflow.name);
      }
    }
    expect(checked, 'no workflow runs on a schedule, so this case checked nothing').toBeGreaterThan(0);
  });

  /**
   * A pinned digest is spelled one way everywhere it appears.
   *
   * The container image and the Bun archive are both bumped by hand and both
   * appear in more than one workflow. A bump reaching one file and not the
   * other leaves a workflow running an image, or unpacking an archive, the
   * repository no longer claims.
   *
   * Matched on the shape of the value rather than the name above it. A key
   * comparison passes a drifted digest under a renamed key, and reading
   * workflow-level `env` alone passes one moved into a job's own block. The
   * text of the file has neither escape.
   *
   * This repository pins one image and one archive, so more than one spelling
   * of either is drift by construction. A second image added on purpose turns
   * this red, which is the point at which somebody decides rather than drifts.
   */
  test('a pinned digest is spelled one way in every workflow', () => {
    const found = new Map<string, Map<string, string>>([
      ['container image', new Map()],
      ['archive digest', new Map()],
    ]);
    for (const workflow of loaded) {
      for (const image of workflow.text.matchAll(/[A-Za-z0-9._/-]+@sha256:[0-9a-f]{64}/g)) {
        (found.get('container image') as Map<string, string>).set(image[0], workflow.name);
      }
      // The digest inside a container reference is already covered above, and
      // a commit pin is forty characters rather than sixty-four.
      for (const digest of workflow.text.matchAll(/(?<![0-9a-f:@])[0-9a-f]{64}(?![0-9a-f])/g)) {
        (found.get('archive digest') as Map<string, string>).set(digest[0], workflow.name);
      }
    }
    for (const [kind, spellings] of found) {
      expect(spellings.size, `the workflows pin no ${kind}, so this case checked nothing`).toBeGreaterThan(0);
      expect(
        [...spellings],
        `the workflows pin more than one ${kind}: ` + [...spellings].map(([v, w]) => `${v} in ${w}`).join(', '),
      ).toHaveLength(1);
    }
  });

  /**
   * A variable a script reads is set in the workflow that runs it.
   *
   * The digest check and the container run both take their value from the
   * environment, and an undefined one leaves `sha256sum` reading a malformed
   * line or `docker run` reaching for an empty image. The rule above holds
   * every spelling of a digest equal, which says nothing when a workflow stops
   * naming one at all, so this is what notices a value going missing from one
   * file rather than from all of them.
   */
  test('a script that checks a digest or runs a container names a variable its workflow sets', () => {
    let checked = 0;
    for (const workflow of loaded) {
      const env = (workflow.parsed.env ?? {}) as Record<string, unknown>;
      for (const { job, step } of jobSteps(workflow.parsed)) {
        const script = String(step['run'] ?? '');
        const wanted = [
          ...[...script.matchAll(/echo "\$([A-Z0-9_]+) .*sha256sum --check/g)].map((m) => m[1] as string),
          ...[...script.matchAll(/\bdocker\s+run\b[^\n]*\$\{?([A-Z0-9_]*_IMAGE)\}?/g)].map((m) => m[1] as string),
        ];
        for (const name of wanted) {
          checked += 1;
          expect(
            env[name],
            `${workflow.name} job ${job} reads $${name}, which its environment does not set`,
          ).toBeDefined();
        }
      }
    }
    expect(checked, 'no script checks a digest or runs a container, so this case checked nothing').toBeGreaterThan(0);
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
        `${workflow.name} passes --unattested, so a row it writes rests on ` + 'whoever typed the gid',
      ).not.toContain('--unattested');
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
  test('no workflow restates the archive destination', () => {
    for (const workflow of loaded) {
      for (const [field, value] of Object.entries(DESTINATION)) {
        expect(
          workflow.text,
          `${workflow.name} restates the ${field} from archive.json, which is ` + 'the one place it is written down',
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
  test.each(loaded.filter((one) => one.name.startsWith('probe-')))(
    '$name can only run on a probe branch, and uploads nothing',
    (workflow: LoadedWorkflow) => {
      expect(disposableProbeProblems(workflow.parsed)).toEqual([]);
    },
  );

  test('the disposable-probe rule catches what it is for', () => {
    const onProbeBranch = {
      on: { push: { branches: ['probe/**'] } },
      jobs: { probe: { steps: [{ run: 'echo hello' }] } },
    };
    expect(disposableProbeProblems(onProbeBranch)).toEqual([]);

    expect(
      disposableProbeProblems({
        ...onProbeBranch,
        on: {
          push: { branches: ['probe/**'] },
          schedule: [{ cron: '0 * * * *' }],
        },
      }),
    ).toContain('it runs on more than a push: push, schedule');

    expect(
      disposableProbeProblems({
        ...onProbeBranch,
        on: { push: { branches: ['main'] } },
      }),
    ).toContain('it can be pushed to main');

    expect(disposableProbeProblems({ ...onProbeBranch, on: { push: {} } })).toContain(
      'it names no branch, so every branch reaches it',
    );

    expect(
      disposableProbeProblems({
        ...onProbeBranch,
        jobs: {
          probe: { steps: [{ uses: 'actions/upload-artifact@aaaa' }] },
        },
      }),
    ).toContain('it uploads an artifact: actions/upload-artifact@aaaa');
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
  test('the SteamCMD file filter names exactly the archived files', () => {
    let checked = 0;
    for (const workflow of loaded) {
      const filter = (workflow.parsed.env ?? {})['STEAMCMD_FILE_FILTER'];
      if (filter === undefined) {
        continue;
      }
      checked += 1;
      expect(String(filter).split(';').sort()).toEqual([...ARCHIVE_FILES].sort());
    }
    expect(checked, 'no workflow sets a file filter').toBeGreaterThan(0);
  });

  /**
   * Depot 2278521 declares `oslist windows`, and a Linux SteamCMD selects
   * depots by client platform. Without the flag the fetch dies with "Missing
   * configuration" and writes nothing, measured on ubuntu-latest.
   */
  test('every app_update forces the platform the depot declares', () => {
    let checked = 0;
    for (const workflow of loaded) {
      for (const job of Object.values(workflow.parsed.jobs ?? {})) {
        const steps = Array.isArray(job['steps']) ? job['steps'] : [];
        for (const step of steps) {
          const script = (step as Record<string, unknown>)['run'];
          if (typeof script !== 'string' || !script.includes('+app_update')) {
            continue;
          }
          checked += 1;
          expect(script, `${workflow.name} runs app_update without forcing the platform`).toContain(
            '+@sSteamCmdForcePlatformType',
          );
        }
      }
    }
    expect(checked, 'no workflow fetches a build').toBeGreaterThan(0);
  });

  /**
   * A tag is resolved when the job runs, so whoever owns it chooses the image
   * on the day rather than on the day the line was written.
   */
  test('every container image is pinned by digest', () => {
    let checked = 0;
    for (const workflow of loaded) {
      // Every image is named by a variable whose name ends in _IMAGE, and the
      // pin lives on that variable. Picking the image out of a `docker run`
      // line by pattern does not survive contact with the line: a volume mount
      // or a --user flag puts other words, and other variables, ahead of it.
      for (const [name, value] of Object.entries(workflow.parsed.env ?? {})) {
        if (!name.endsWith('_IMAGE')) {
          continue;
        }
        checked += 1;
        expect(String(value), `${workflow.name} sets ${name} to an image that is not pinned by digest`).toMatch(
          /@sha256:[0-9a-f]{64}$/,
        );
      }

      for (const line of workflow.text.split('\n')) {
        // A comment naming the command is prose about it, not a run of it.
        if (/^\s*#/.test(line) || !/\bdocker\s+run\b/.test(line)) {
          continue;
        }
        expect(
          line,
          `${workflow.name} runs a container without naming a pinned *_IMAGE ` + `variable: ${line.trim()}`,
        ).toMatch(/\$\{?[A-Z0-9_]*_IMAGE\}?/);
      }
    }
    expect(checked, 'no workflow names a container image').toBeGreaterThan(0);
  });

  /**
   * Every artifact this pipeline downloads at run time is executed. The digest
   * check is the only thing binding the bytes, and deleting the line is a
   * one-character-looking change.
   */
  test('every downloaded artifact is checked against a digest before it is used', () => {
    let checked = 0;
    for (const workflow of loaded) {
      for (const job of Object.values(workflow.parsed.jobs ?? {})) {
        const steps = Array.isArray(job['steps']) ? job['steps'] : [];
        for (const step of steps) {
          const script = (step as Record<string, unknown>)['run'];
          if (typeof script !== 'string' || !script.includes('curl ')) {
            continue;
          }
          checked += 1;
          expect(script, `${workflow.name} downloads a file and does not check it:\n${script}`).toContain(
            'sha256sum --check --strict',
          );
        }
      }
    }
    expect(checked, 'no workflow downloads anything').toBeGreaterThan(0);
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
  test('every action comes from a known owner, and every known owner is used', () => {
    const used = new Set<string>();
    for (const workflow of loaded) {
      for (const reference of actionReferences(workflow.parsed)) {
        if (reference.startsWith('./')) {
          continue;
        }
        const owner = reference.split('/')[0] as string;
        used.add(owner);
        expect(ACTION_OWNERS, `${workflow.name} uses ${reference}, whose owner is not on the list`).toContain(owner);
      }
    }
    expect(
      ACTION_OWNERS.filter((owner) => !used.has(owner)),
      'an owner on the list has no action left in any workflow',
    ).toEqual([]);
  });

  /**
   * Bun runs the root `prepare` on install, which points `core.hooksPath` at
   * `.githooks`. That pre-push hook runs the whole gate, and no runner here
   * installs the tool belt, so the gate exits non-zero and the push is
   * refused. A workflow that installs and pushes has to keep the two apart.
   */
  test('a workflow that installs and pushes does not run install scripts', () => {
    for (const workflow of loaded) {
      const scripts = Object.values(workflow.parsed.jobs ?? {})
        .flatMap((job) => (Array.isArray(job['steps']) ? job['steps'] : []))
        .map((step) => (step as Record<string, unknown>)['run'])
        .filter((script): script is string => typeof script === 'string');
      const pushes = scripts.some((script) => /\bgit push\b/.test(script));
      const installs = scripts.filter((script) => /\bbun(x)? install\b/.test(script));
      if (!pushes || installs.length === 0) {
        continue;
      }
      for (const install of installs) {
        expect(
          install,
          `${workflow.name} pushes and installs, so the install has to skip ` +
            'the root prepare script that installs the pre-push hook',
        ).toContain('--ignore-scripts');
      }
    }
  });

  /**
   * The reader has to see an action whatever style it is written in. A
   * line-anchored text match cannot, which is why the reader parses the
   * document.
   */
  test('the reader finds an action written in flow style', () => {
    const parsed = Bun.YAML.parse(
      [
        'on: push',
        'jobs:',
        '  block:',
        '    steps:',
        '      - uses: actions/checkout@aaaa',
        '  flow:',
        '    steps:',
        '      - { uses: actions/setup-node@bbbb }',
        '  reusable:',
        '    uses: ./.github/workflows/other.yml@cccc',
      ].join('\n'),
    ) as Workflow;
    expect(actionReferences(parsed).sort()).toEqual([
      './.github/workflows/other.yml@cccc',
      'actions/checkout@aaaa',
      'actions/setup-node@bbbb',
    ]);
  });
});

/** A file under `.github`, read as text. */
async function githubText(relative: string): Promise<string> {
  return Bun.file(fileURLToPath(new URL(`../.github/${relative}`, import.meta.url))).text();
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
  if (typeof value !== 'object' || value === null) {
    return undefined;
  }
  return (value as Record<string, unknown>)[key];
}

/**
 * The inline zizmor ignore comments this repository has decided on, as
 * `file:audit`. Every one answers `artipacked` on a checkout whose job pushes
 * over SSH, which needs the key to stay in the checkout.
 */
const ALLOWED_ZIZMOR_IGNORES = [
  'build-watch.yml:artipacked',
  'build-watch.yml:artipacked',
  'client-build-watch.yml:artipacked',
];

describe('the GitHub configuration', () => {
  /**
   * Dependabot's cooldown is the wait between a version being published and a
   * pull request proposing it, and an ecosystem with no cooldown block waits
   * for nothing. zizmor's `dependabot-cooldown` refuses a cooldown shorter than
   * `.github/zizmor.yml` sets, and passes a block removed entirely, so this
   * reads every entry.
   */
  test('every dependabot ecosystem carries a cooldown zizmor can hold', async () => {
    const zizmor = await githubYaml('zizmor.yml');
    const threshold = field(field(field(field(zizmor, 'rules'), 'dependabot-cooldown'), 'config'), 'days');
    expect(typeof threshold, '.github/zizmor.yml sets no dependabot-cooldown threshold in days').toBe('number');

    const updates = field(await githubYaml('dependabot.yml'), 'updates');
    expect(Array.isArray(updates), 'dependabot.yml lists no updates').toBe(true);
    const short: string[] = [];
    for (const update of updates as unknown[]) {
      const ecosystem = String(field(update, 'package-ecosystem'));
      const days = field(field(update, 'cooldown'), 'default-days');
      if (typeof days !== 'number') {
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
  test('zizmor answers only the findings this repository allows', async () => {
    const github = fileURLToPath(new URL('../.github/', import.meta.url));
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
        for (const audit of (match[1] as string).split(',')) {
          if (audit.trim() !== '') {
            found.push(`${file}:${audit.trim()}`);
          }
        }
      }
    }
    expect(found.sort()).toEqual([...ALLOWED_ZIZMOR_IGNORES].sort());

    const rules = field(await githubYaml('zizmor.yml'), 'rules');
    const answering = Object.entries((rules ?? {}) as Record<string, unknown>).filter(
      ([, rule]) => field(rule, 'ignore') !== undefined || field(rule, 'disable') !== undefined,
    );
    expect(
      answering.map(([name]) => name),
      '.github/zizmor.yml ignores or disables an audit outright',
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
  test('every Bun release a workflow uses is read from .bun-version', async () => {
    const pin = (await Bun.file(fileURLToPath(new URL('../.bun-version', import.meta.url))).text()).trim();
    expect(pin, '.bun-version holds what is not one exact release').toMatch(/^\d+\.\d+\.\d+$/);

    let setups = 0;
    let fetches = 0;
    for (const workflow of loaded) {
      for (const [path, value] of keyedValues(workflow.parsed)) {
        const key = path[path.length - 1] as string;
        if (!/bun/i.test(key) || !/version/i.test(key)) {
          continue;
        }
        expect(
          key === 'bun-version-file' && value === '.bun-version',
          `${workflow.name} sets ${path.join('.')} to ${String(value)} itself`,
        ).toBe(true);
      }
      expect(wholeVersionIn(workflow.text, pin), `${workflow.name} writes the release .bun-version pins`).toBe(false);

      for (const job of Object.values(workflow.parsed.jobs ?? {})) {
        const steps = Array.isArray(job['steps']) ? job['steps'] : [];
        for (const step of steps) {
          const uses = String(field(step, 'uses') ?? '');
          const script = String(field(step, 'run') ?? '');
          if (uses.startsWith('oven-sh/setup-bun@')) {
            setups += 1;
            expect(
              field(field(step, 'with'), 'bun-version-file'),
              `${workflow.name} runs setup-bun without the pin file`,
            ).toBe('.bun-version');
          }
          if (script.includes('releases/download/bun-v')) {
            fetches += 1;
            expect(script, `${workflow.name} fetches a release it writes down itself`).not.toMatch(/bun-v\d/);
            const release = /releases\/download\/bun-v([^/"']+)\//.exec(script);
            const variable = /^\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?$/.exec(release?.[1] ?? '');
            expect(
              variable?.[1],
              `${workflow.name} fetches bun-v${release?.[1]}, which is no ` + 'shell variable this case can follow',
            ).toBeDefined();
            const name = variable?.[1] ?? '';
            expect(
              new RegExp(`(^|\\n)\\s*${name}=[^\\n]*\\.bun-version`).test(script),
              `${workflow.name} fetches bun-v$${name} and never sets it from ` + 'the pin file',
            ).toBe(true);
          }
        }
      }
    }
    expect(setups, 'no workflow runs setup-bun').toBeGreaterThan(0);
    expect(fetches, 'no workflow fetches Bun by hand').toBeGreaterThan(0);
  });

  /**
   * The watcher opens a build issue from a script, and a person opens the
   * same issue from the form. Both have to read as one document, so the
   * script prints the form's title, its label, every field label in order,
   * an option of each dropdown it answers, and every checkbox in order. A
   * field added, renamed or reworded in the form turns this red until the
   * script says the same thing.
   */
  test('the build issue the watcher opens matches the form a person fills', async () => {
    const form = await githubYaml('ISSUE_TEMPLATE/new-keen-build.yml');
    const fields = (field(form, 'body') as unknown[]).filter((one) => field(one, 'type') !== 'markdown');
    expect(fields.length, 'the form holds no field').toBeGreaterThan(0);

    const watch = loaded.find((one) => one.name === 'build-watch.yml');
    expect(watch, 'there is no build-watch.yml').toBeDefined();
    const steps = Object.values(watch?.parsed.jobs ?? {}).flatMap((job) =>
      Array.isArray(job['steps']) ? (job['steps'] as unknown[]) : [],
    );
    const opener = steps.find((step) => field(step, 'name') === 'Open the build issue');
    expect(opener, 'no step opens the build issue').toBeDefined();
    const script = String(field(opener, 'run') ?? '');

    const title = String(field(form, 'title'));
    expect(script, "the issue title is not the form's").toContain(`--title "${title}`);
    for (const label of field(form, 'labels') as string[]) {
      expect(script, `the issue does not carry the ${label} label`).toContain(`--label ${label}`);
    }

    const sections = issueSections(printedText(script));
    expect(
      sections.map((section) => section.heading),
      "the issue's headings are not the form's field labels, in order",
    ).toEqual(fields.map((one) => String(field(field(one, 'attributes'), 'label'))));
    for (const [index, section] of sections.entries()) {
      const attributes = field(fields[index], 'attributes');
      const type = field(fields[index], 'type');
      if (type === 'dropdown') {
        expect(
          (field(attributes, 'options') as unknown[]).map(String),
          `the issue answers "${section.heading}" with an option the form lacks`,
        ).toContain(section.content);
      }
      if (type === 'checkboxes') {
        const labels = (field(attributes, 'options') as unknown[]).map((option) => String(field(option, 'label')));
        const printed = section.content.split('\n').map((line) => line.replace(/^- \[ \] /, ''));
        expect(printed, `the issue's "${section.heading}" list is not the form's, in order`).toEqual(labels);
      }
    }
  });

  /** The script reader takes the shapes the opener prints in. */
  test('the printed text of a script joins its printf formats', () => {
    const script = [
      '{',
      '  printf \'### A\\n\\n%s\\n\\n\' "$X"',
      "  printf -- '- [ ] `b` c\\n'",
      "  echo 'ignored'",
      '} > body.md',
    ].join('\n');
    expect(printedText(script)).toBe('### A\n\n%s\n\n- [ ] `b` c\n');
    expect(issueSections('### A\n\none\n\n### B\n\n- [ ] x\n- [ ] y\n')).toEqual([
      { heading: 'A', content: 'one' },
      { heading: 'B', content: '- [ ] x\n- [ ] y' },
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
function keyedValues(value: unknown, path: readonly string[] = []): [string[], unknown][] {
  if (typeof value !== 'object' || value === null) {
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
  const escaped = version.replaceAll('.', '\\.');
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
  const formats = [...script.matchAll(/printf(?:\s+--)?\s+'([^']*)'/g)].map((match) =>
    (match[1] as string).replaceAll('\\n', '\n'),
  );
  return formats.join('');
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
      const [heading, ...rest] = section.split('\n');
      return { heading: heading as string, content: rest.join('\n').trim() };
    });
}

/**
 * Whether a job waits on a person before it starts.
 *
 * @param job - The job, as the parsed document holds it.
 * @returns True when the job declares an environment that is not on
 * `UNGATED_ENVIRONMENTS`.
 */
function waitsForApproval(job: Record<string, unknown>): boolean {
  const environment = job['environment'];
  if (environment === undefined) {
    return false;
  }
  return !UNGATED_ENVIRONMENTS.includes(String(environment));
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
  if (triggers.length !== 1 || triggers[0] !== 'push') {
    problems.push(`it runs on more than a push: ${triggers.join(', ')}`);
  }
  const push = (workflow.on ?? {})['push'] as { branches?: string[] } | undefined;
  const branches = push?.branches ?? [];
  if (branches.length === 0) {
    problems.push('it names no branch, so every branch reaches it');
  }
  for (const branch of branches) {
    if (!branch.startsWith('probe/')) {
      problems.push(`it can be pushed to ${branch}`);
    }
  }
  for (const reference of actionReferences(workflow)) {
    if (reference.includes('upload-artifact')) {
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
    const jobUses = job['uses'];
    if (typeof jobUses === 'string') {
      found.push(jobUses);
    }
    const steps = job['steps'];
    if (!Array.isArray(steps)) {
      continue;
    }
    for (const step of steps) {
      const uses = (step as Record<string, unknown>)['uses'];
      if (typeof uses === 'string') {
        found.push(uses);
      }
    }
  }
  return found;
}

/**
 * Every step a workflow runs, with the job it belongs to.
 *
 * @param workflow - The parsed document.
 * @returns One entry per step, at every job.
 */
function jobSteps(workflow: Workflow): { job: string; step: Record<string, unknown> }[] {
  const found: { job: string; step: Record<string, unknown> }[] = [];
  for (const [job, body] of Object.entries(workflow.jobs ?? {})) {
    const steps = Array.isArray(body['steps']) ? body['steps'] : [];
    for (const step of steps) {
      found.push({ job, step: step as Record<string, unknown> });
    }
  }
  return found;
}

/** The file pinning a version for every tool mise installs. */
const PINS = 'mise.toml';

/** One tool the pin file names, and the version it holds. */
interface PinnedTool {
  /** The key the entry is spelled under, bare name or backend coordinate. */
  readonly name: string;
  /** The version the entry holds. */
  readonly version: string;
}

/**
 * Every tool `PINS` pins.
 *
 * @remarks
 * An entry's value is the version itself, or a table carrying it under
 * `version`, which is the shape an entry with backend options takes. The key
 * is whatever `PINS` spells, so a backend coordinate stays whole.
 *
 * The lockfile beside it is read by the xtask suite, which holds every entry
 * to a checksum per platform. One rule per file is what keeps a failure naming
 * the thing that broke.
 *
 * @returns One entry per tool, in the order the file holds them.
 */
async function pinnedTools(): Promise<PinnedTool[]> {
  const parsed = Bun.TOML.parse(await Bun.file(PINS).text()) as Record<string, unknown>;
  const tools = parsed['tools'];
  if (typeof tools !== 'object' || tools === null) {
    throw new Error(`${PINS} holds no tools table`);
  }
  return Object.entries(tools as Record<string, unknown>).map(([name, entry]) => {
    const version = typeof entry === 'string' ? entry : (entry as Record<string, unknown>)['version'];
    if (typeof version !== 'string') {
      throw new Error(`${PINS} pins no version for ${name}`);
    }
    return { name, version };
  });
}

/**
 * Every way a workflow installs something other than what the pin file holds.
 *
 * @remarks
 * Five shapes. A version written anywhere in the workflow is refused outright,
 * comment included, because the next bump leaves that copy behind. A
 * `mise_toml` or `tool_versions` input makes the action write its own pin file
 * over the committed one, so the lockfile rule would check a file no install
 * reads. A `sha256` input returns before the action compares its mise download
 * against the minisign-signed `SHASUMS256.txt`, trading a signature for a hash
 * somebody typed. An install turned off leaves the gate resolving tools that
 * are not there. A mise release that is not exact resolves at run time, and a
 * release resolved at run time is under no cooldown.
 *
 * @param workflow - The workflow, parsed and with the text it came from.
 * @param tools - Every tool the pin file names.
 * @returns One sentence per problem, empty when the workflow is bound to its
 * pins.
 */
function pinProblems(workflow: LoadedWorkflow, tools: readonly PinnedTool[]): string[] {
  const problems: string[] = [];
  for (const tool of tools) {
    if (wholeVersionIn(workflow.text, tool.version)) {
      problems.push(`it writes ${tool.version}, which ${PINS} pins`);
    }
  }

  for (const { step } of jobSteps(workflow.parsed)) {
    if (!String(step['uses'] ?? '').startsWith('jdx/mise-action@')) {
      continue;
    }
    const inputs = (step['with'] ?? {}) as Record<string, unknown>;
    for (const written of ['mise_toml', 'tool_versions']) {
      if (inputs[written] !== undefined) {
        problems.push(`mise-action takes ${written}, which writes over ${PINS}`);
      }
    }
    if (inputs['sha256'] !== undefined) {
      problems.push('mise-action takes a sha256, which skips the signed checksum file');
    }
    if (inputs['install'] === false) {
      problems.push('mise-action installs nothing, so no tool is there to run');
    }
    const release = String(inputs['version'] ?? '');
    if (!/^\d+\.\d+\.\d+$/.test(release)) {
      problems.push(`mise-action takes ${release || 'no version'}, which is not one exact release`);
    }
  }
  return problems;
}

const tools = await pinnedTools();

describe('the pinned tools', () => {
  test('every tool names one exact release', () => {
    expect(tools.length, `${PINS} pins nothing`).toBeGreaterThan(0);
    for (const tool of tools) {
      expect(tool.version, `${PINS} pins ${tool.name} at a range`).toMatch(/^\d+\.\d+\.\d+$/);
    }
  });

  /**
   * The workflows take every version from the file that pins it. A version
   * written into a workflow is a second copy, and the next bump leaves it
   * behind while continuous integration keeps installing the old one.
   */
  test.each(loaded.map((workflow) => [workflow.name] as const))(
    '%s takes every version from the file that pins it',
    (name) => {
      const workflow = loaded.find((one) => one.name === name) as LoadedWorkflow;
      expect(pinProblems(workflow, tools)).toEqual([]);
    },
  );

  /**
   * Some workflow step installs through mise. A pin file nothing installs from
   * pins versions that reach no runner, and every case above still passes.
   */
  test('a workflow step installs through mise', () => {
    const installers = loaded
      .filter((workflow) =>
        jobSteps(workflow.parsed).some(({ step }) => String(step['uses'] ?? '').startsWith('jdx/mise-action@')),
      )
      .map((workflow) => workflow.name);
    expect(installers, 'no workflow installs through mise').not.toEqual([]);
  });
});

/**
 * The gate job's shape, reduced to the lines the pin rule reads.
 *
 * @remarks
 * Each case below is this document with one thing changed, and every change is
 * one a weaker rule passes over: a version written into a comment, a pin file
 * handed to the action, a signed checksum traded for a typed hash, an install
 * turned off, a release left to resolve at run time. A rule nothing can break
 * is a rule that proves nothing, so each case names the sentence it expects.
 */
const SOUND_GATE = [
  'name: ci',
  'on: [push]',
  'jobs:',
  '  gate:',
  '    strategy:',
  '      matrix:',
  '        os: [windows-latest, ubuntu-latest]',
  '    runs-on: ${{ matrix.os }}',
  '    steps:',
  '      - uses: jdx/mise-action@aaaa',
  '        with:',
  '          version: 2026.9.5',
  '      - run: bun install --frozen-lockfile',
  '        shell: bash',
].join('\n');

/**
 * A document the pin rule can read, built from text written here.
 *
 * @param text - The workflow, as YAML.
 * @returns The document with the text it came from.
 */
function doctored(text: string): LoadedWorkflow {
  return { name: 'ci.yml', text, parsed: Bun.YAML.parse(text) as Workflow };
}

describe('the pin rule against a doctored document', () => {
  const onePin: PinnedTool[] = [{ name: 'cargo-deny', version: '0.20.2' }];

  test('the sound document is accepted', () => {
    expect(pinProblems(doctored(SOUND_GATE), onePin)).toEqual([]);
  });

  test.each([
    [
      'a pinned version written into a comment',
      `${SOUND_GATE}\n      # cargo-deny 0.20.2 is what this installs`,
      'it writes 0.20.2, which mise.toml pins',
    ],
    [
      'a pin file handed to the action',
      SOUND_GATE.replace('          version: 2026.9.5', '          version: 2026.9.5\n          mise_toml: "[tools]"'),
      'mise-action takes mise_toml, which writes over mise.toml',
    ],
    [
      'a tool-versions file handed to the action',
      SOUND_GATE.replace(
        '          version: 2026.9.5',
        '          version: 2026.9.5\n          tool_versions: cargo-deny 0.20.1',
      ),
      'mise-action takes tool_versions, which writes over mise.toml',
    ],
    [
      'a typed hash in place of the signed checksum file',
      SOUND_GATE.replace('          version: 2026.9.5', '          version: 2026.9.5\n          sha256: abc123'),
      'mise-action takes a sha256, which skips the signed checksum file',
    ],
    [
      'the install turned off',
      SOUND_GATE.replace('          version: 2026.9.5', '          version: 2026.9.5\n          install: false'),
      'mise-action installs nothing, so no tool is there to run',
    ],
    [
      'a mise release left to resolve at run time',
      SOUND_GATE.replace('          version: 2026.9.5', '          version: 2026'),
      'mise-action takes 2026, which is not one exact release',
    ],
    [
      'no mise release at all',
      SOUND_GATE.replace('        with:\n          version: 2026.9.5\n', ''),
      'mise-action takes no version, which is not one exact release',
    ],
  ])('%s', (_what, text, expected) => {
    expect(pinProblems(doctored(text), onePin)).toContain(expected);
  });
});
