import { readFileSync } from 'node:fs';

// The scope vocabulary is .github/commit-scopes.json, an array of
// { scope, covers } objects. `cargo xtask scopes` prints the same file, so the
// hook and the command read one list.
const entries = JSON.parse(readFileSync(new URL('.github/commit-scopes.json', import.meta.url), 'utf8'));
const scopes = entries.map((entry) => entry.scope);
if (scopes.length === 0) {
  throw new Error('.github/commit-scopes.json names no scope');
}

export default {
  extends: ['@commitlint/config-conventional'],
  // Dependabot writes release notes and compare links into the body, well past
  // the 72-column limit, and that is the update path the cooldown protects.
  ignores: [(message) => message.includes('Signed-off-by: dependabot[bot]')],
  rules: {
    'scope-enum': [2, 'always', scopes],
    'header-max-length': [2, 'always', 72],
    'body-max-line-length': [2, 'always', 72],
  },
};
