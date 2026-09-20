import { readFileSync } from 'node:fs';

// The scope vocabulary is .github/commit-scopes.json. `cargo xtask scopes`
// prints the same file, so the hook and the command read one list.
const scopes = JSON.parse(readFileSync(new URL('.github/commit-scopes.json', import.meta.url), 'utf8'));

export default {
  extends: ['@commitlint/config-conventional'],
  // Dependabot writes release notes and compare links into the body, well past
  // the 72-column limit, and that is the update path the cooldown protects.
  ignores: [(message) => message.includes('Signed-off-by: dependabot[bot]')],
  rules: {
    'scope-enum': [2, 'always', scopes],
    'body-max-line-length': [2, 'always', 72],
  },
};
