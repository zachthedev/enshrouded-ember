export default {
  extends: ["@commitlint/config-conventional"],
  // Dependabot writes release notes and compare links into the body, well past
  // the 72-column limit, and that is the update path the cooldown protects.
  ignores: [(message) => message.includes("Signed-off-by: dependabot[bot]")],
  rules: {
    "scope-enum": [
      2,
      "always",
      // The workspace crate names past the `ember-` prefix, plus the
      // cross-cutting names no crate owns. `cargo xtask scopes` prints the
      // same list.
      // prettier-ignore
      [
        'loader', 'sdk', 'holistic', 'kfc', 'enshrouded', 'sigs',
        'platform', 'testkit', 'xtask', 'deps', 'ci', 'release',
      ],
    ],
    "body-max-line-length": [2, "always", 72],
  },
};
