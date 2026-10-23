// Conventional Commits, tuned to this repository's history:
// `subject-case` stays open (existing messages mix "Fix token pasting"
// and lowercase subjects), everything else follows the standard rules.
export default {
  extends: ["@commitlint/config-conventional"],
  rules: {
    "header-max-length": [2, "always", 100],
    "subject-case": [0, "always", []],
    "type-enum": [
      2,
      "always",
      ["feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore", "revert"],
    ],
  },
};
