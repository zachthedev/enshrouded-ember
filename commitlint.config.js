export default {
  extends: ['@commitlint/config-conventional'],
  rules: {
    'scope-enum': [
      2,
      'always',
      ['loader', 'sdk', 'holistic', 'enshrouded', 'sigs', 'platform', 'testkit', 'xtask', 'deps', 'ci', 'release'],
    ],
    'body-max-line-length': [2, 'always', 72],
  },
};
