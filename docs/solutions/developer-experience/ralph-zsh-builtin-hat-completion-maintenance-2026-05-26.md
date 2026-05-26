---
title: Ralph zsh builtin hat completion maintenance
date: 2026-05-26
category: developer-experience
module: ralph-cli
problem_type: developer_experience
component: tooling
severity: medium
applies_when:
  - Updating builtin Ralph hat collections or presets
  - Maintaining zsh completion for values that contain colons
  - Installing repository completion changes into the current user's oh-my-zsh plugin
tags: [zsh-completion, ralph-cli, builtin-hats, oh-my-zsh, developer-experience]
---

# Ralph zsh builtin hat completion maintenance

## Context

`ralph run -H builtin:<TAB>` stopped producing useful completions for builtin hat collections. Earlier investigation confirmed that `compdef _ralph ralph` was registered and the completion function was being loaded, but the actual `builtin:*` candidates still did not appear correctly in an interactive shell.

The final fix also uncovered a maintenance rule: builtin hat collection changes in the Rust CLI and mirrored preset files must be reflected in `scripts/ralph-zsh-plugin.zsh`, then installed into the user's active oh-my-zsh plugin copy. Updating the repository script alone is not enough when the current shell sources `~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`.

## Guidance

Keep builtin hat completion values in `scripts/ralph-zsh-plugin.zsh` as value and description arrays, and complete them with `compadd`, not `_describe`.

The important shape is:

```zsh
_RALPH_BUILTIN_HAT_VALUES=(
  "builtin:code-assist"
  "builtin:debug"
  "builtin:research"
  "builtin:review"
  "builtin:pdd-to-code-assist"
  "builtin:autoresearch"
  "builtin:hatless-baseline"
  "builtin:merge-loop"
)

_RALPH_BUILTIN_HAT_DESCRIPTIONS=(
  "Default implementation workflow with TDD and adversarial validation"
  "Bug investigation, root-cause analysis, and adversarial fix verification"
  "Read-only codebase and architecture exploration with evidence-first synthesis"
  "Adversarial code review without making modifications"
  "Advanced end-to-end idea-to-code workflow"
  "Autonomous experiment loop: try ideas, measure, keep what works"
  "Minimal bare hat collection for baseline comparison"
  "Internal preset for loop merge operations"
)

_ralph_builtin_hats() {
  compadd \
    -X 'builtin hat collection' \
    -d _RALPH_BUILTIN_HAT_DESCRIPTIONS \
    -a _RALPH_BUILTIN_HAT_VALUES
}
```

Do not encode builtin hat candidates as `_describe` entries like:

```zsh
_RALPH_BUILTIN_HATS=(
  "builtin:code-assist:Default implementation workflow"
)

_describe 'hat source' _RALPH_BUILTIN_HATS
```

zsh uses `:` as the separator between the completion value and its description in `_describe` entry strings. For values that themselves contain `:`, such as `builtin:code-assist`, `_describe` can split the value incorrectly and leave `builtin:` prefix completion broken.

Use a dedicated hat-source completer for `-H`:

```zsh
_ralph_hat_source() {
  local ret=1

  _ralph_builtin_hats && ret=0
  _files && ret=0

  return ret
}
```

Wire `-H` to `_ralph_hat_source` in both global and `run` argument completion:

```zsh
'-H+[Hat collection source]:hat source:_ralph_hat_source'
```

Also clear old function definitions at the top of the plugin before redefining them. This makes `source ~/.zshrc` pick up completion fixes without requiring a fresh shell:

```zsh
for _ralph_fn in \
  _ralph \
  _ralph_subcmd_args \
  _ralph_builtin_hats \
  _ralph_hat_source \
  _ralph_run_args; do
  unfunction "$_ralph_fn" 2>/dev/null || true
done
unset _ralph_fn
```

The real script clears every helper function it defines, not only the abbreviated list above.

When builtin hat collections or preset metadata change, update these places together:

- `crates/ralph-cli/src/presets.rs` and any mirrored preset YAML involved in the CLI change
- `scripts/ralph-zsh-plugin.zsh` builtin hat value and description arrays
- `AGENTS.md` maintenance guidance if the workflow changes
- `~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`, by installing the repository script:

```bash
cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh
```

## Why This Matters

`builtin:*` is a user-facing CLI contract. If the Rust CLI accepts a builtin hat collection but zsh completion does not expose it, users get stale or missing guidance at the exact point where completion should reduce friction.

The failure mode is subtle because the completion function can be registered correctly while candidates are still malformed. Checking only `_comps[ralph]` or `whence -f _ralph` is insufficient; the candidate generation path for `-H builtin:` must also be validated.

Installing the script matters because this user's shell sources:

```zsh
source "$HOME/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh"
```

That file is a copied plugin artifact, not the repository file. Any repository-only fix remains invisible until copied into the oh-my-zsh plugin directory or the shell is configured to source the repository script directly.

## When to Apply

- A builtin hat collection is added, removed, renamed, hidden, or made public.
- A preset description changes and should be reflected in completion descriptions.
- `ralph run -H builtin:<TAB>` shows missing, stale, truncated, or duplicate candidates.
- zsh completion code is changed and needs to support values containing colons.

## Examples

After editing the script, run zsh-level validation:

```bash
zsh -n scripts/ralph-zsh-plugin.zsh
zsh -fc 'autoload -Uz compinit; compinit -D -u; source scripts/ralph-zsh-plugin.zsh; echo ${_comps[ralph]:-NOT_REGISTERED}; print -rl -- $_RALPH_BUILTIN_HAT_VALUES'
```

Then install it for the current user:

```bash
cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh
diff -q scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh
```

For an interactive smoke test, source the user's zsh config and complete:

```zsh
source ~/.zshrc
ralph run -H buil<TAB>
```

Expected behavior: zsh offers full `builtin:*` candidates such as `builtin:code-assist`, without `_description: not an identifier` errors and without truncating the candidate to only `builtin`.

## Related

- `scripts/ralph-zsh-plugin.zsh`
- `crates/ralph-cli/src/presets.rs`
- `AGENTS.md`
- `docs/solutions/ralph-zsh-completion-issue.md` records the earlier incomplete investigation that led to the final fix.
