# Implementation Review Prompt

This prompt is the runtime entry for the `implementation-review` workflow.

- Provide the real repository-relative implementation plan with `--plan PLAN_PATH`.
- Read that plan before reviewing any code, and use it as the source of review scope and intent.
- Keep the review read-only: do not modify tracked source files, the plan, or unrelated project files.
- Write only the review artifacts required by the active workflow under `.ralph/review/<plan>/`.
- If the plan is missing, unreadable, ambiguous, or the review scope cannot be frozen safely, stop without project changes and report the blocking artifact.
- Follow the injected Ralph tools guidance for event prechecks, payload fields, and terminal completion.
