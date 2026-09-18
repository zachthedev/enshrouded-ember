export default {
  extends: ['@commitlint/config-conventional'],
  // Dependabot writes release notes and compare links into the body, well past
  // the 72-column limit, and that is the update path the cooldown protects.
  ignores: [(message) => message.includes('Signed-off-by: dependabot[bot]')],
  rules: {
    'scope-enum': [
      2,
      'always',
      ['loader', 'sdk', 'holistic', 'enshrouded', 'sigs', 'platform', 'testkit', 'xtask', 'deps', 'ci', 'release'],
    ],
    'body-max-line-length': [2, 'always', 72],
  },
};
