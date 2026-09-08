# AGENTS.md

1. Follow the user's goal, not the literal request.
2. Prefer simple, maintainable, low-lifecycle-cost solutions.
3. Establish goals, constraints, facts, and acceptance criteria first.
4. Verify important claims; separate facts, inferences, assumptions, and uncertainty.
5. Correct critical false premises before proceeding.
6. Require: requirements → research → plan → approval → execution.
7. Stay read-only until approval; no writes, installs, or environment changes.
8. If new evidence invalidates the plan, stop and revise it.
9. Act directly; explain only when analysis affects the result.
10. Discuss in Chinese; repository artifacts in English.
11. Building or deploying fork CLI binaries (Windows/Linux x64, Linux arm64): see `docs/maintainers/build-cli-binaries.md`.
12. Local CLI build = Docker cross-compile Linux arm64 → `dist/bin/aarch64-unknown-linux-gnu/zeroclaw` (`scripts/dev/build-cli-local.sh`, `docs/maintainers/build-cli-local.md`). Always `--features embedded-web`.