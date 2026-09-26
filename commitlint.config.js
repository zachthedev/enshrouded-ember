import { readFileSync } from 'node:fs';
import { env } from 'node:process';
import { parse } from 'yaml';

// .github/commit-scopes.json lists each scope and what it covers. CONTRIBUTING.md points at it
// rather than restating it, so a new scope is one edit. The path resolves against this file,
// so the list is found however this module is loaded.
const vocabularyPath = new URL('.github/commit-scopes.json', import.meta.url);
const scopes = JSON.parse(readFileSync(vocabularyPath, 'utf8')).map((entry) => entry.scope);

// scope-enum accepts every scope when handed an empty list, so a vocabulary
// that failed to load would read as a passing gate.
if (scopes.length === 0) {
  throw new Error(`${vocabularyPath.href} must list at least one scope.`);
}

// The headers Dependabot starts a commit with: the commit-message prefix and
// prefix-development of each updates entry in .github/dependabot.yml, then a
// colon and a space. A prefix is set per entry, so every entry counts, and a
// prefix anywhere else in the file does not. A repository without the file has
// no Dependabot header. A file that does not parse throws, so the lint fails
// rather than skipping. yaml's duplicate-key check takes time growing with the
// square of the keys, so a file past DEPENDABOT_BYTES throws before the parse
// rather than holding the lint for minutes.
const DEPENDABOT_BYTES = 65_536;
const dependabotPath = new URL('.github/dependabot.yml', import.meta.url);
let dependabotBytes;
try {
  dependabotBytes = readFileSync(dependabotPath);
} catch (error) {
  if (error.code !== 'ENOENT') {
    throw error;
  }
}
if (dependabotBytes !== undefined && dependabotBytes.length > DEPENDABOT_BYTES) {
  throw new Error(
    `${dependabotPath.href} is ${dependabotBytes.length} bytes, and this config reads ${DEPENDABOT_BYTES} at most.`,
  );
}
const dependabotUpdates = parse(dependabotBytes?.toString('utf8') ?? '')?.updates;
const dependabotHeaders = (Array.isArray(dependabotUpdates) ? dependabotUpdates : [])
  .flatMap((entry) => [entry?.['commit-message']?.prefix, entry?.['commit-message']?.['prefix-development']])
  .filter((prefix) => typeof prefix === 'string' && prefix.length > 0)
  .map((prefix) => `${prefix}: `);

export default {
  extends: ['@commitlint/config-conventional'],
  // Dependabot writes body lines past the 72-column limit that hold no URL, such
  // as a grouped update's "Updates `<package>` from <old> to <new>", and that is
  // the update path the cooldown protects. A commit is skipped only on a pull
  // request Dependabot opened, which the shared commits job marks by setting
  // COMMITLINT_DEPENDABOT_PULL_REQUEST to true, and only when its header starts
  // with a Dependabot header above and a line after the header starts with
  // Dependabot's trailer, which anyone can type. A one-commit pull request lands
  // under its commit's header, so a skipped header that lands still carries the
  // type and scope Dependabot is configured with. The rest of that header goes
  // unchecked. A one-line pull request title has no line after its header, so
  // the title lint checks it.
  ignores: [
    (message) => {
      const [header, ...rest] = message.split('\n');
      return (
        env.COMMITLINT_DEPENDABOT_PULL_REQUEST === 'true' &&
        dependabotHeaders.some((prefix) => header.startsWith(prefix)) &&
        rest.some((line) => line.startsWith('Signed-off-by: dependabot[bot] <'))
      );
    },
  ],
  rules: {
    'scope-enum': [2, 'always', scopes],
    // github.com shows a subject whole up to 72 characters and cuts one of 73
    // or more.
    'header-max-length': [2, 'always', 72],
    // The same width for the body, so a message reads the same in a terminal
    // as it does on GitHub. A line holding a URL is exempt by the rule.
    'body-max-line-length': [2, 'always', 72],
  },
};
