# Ralph Pipeline Prompt

- project_root: `./`
- preset: `builtin:ce-executor-pipeline`
- plan: `plan.md`
- prompt_file: `PROMPT.pipeline.md`

Read the plan at the referenced path. Do not invent preset
contents, do not look up hat collections by name, and do not
read any runtime-managed block from the target project. The
runtime injects the preset-specific instructions downstream.
