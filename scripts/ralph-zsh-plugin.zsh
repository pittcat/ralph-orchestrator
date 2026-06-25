#compdef ralph

# Ralph Orchestrator - Zsh Plugin
# Installation: Add to ~/.zshrc: source /path/to/ralph-zsh-plugin.zsh
# Or copy to your plugin directory (e.g., ~/.zsh/plugins/ralph/)
# For auto-completion to work, ensure compinit is loaded before sourcing this plugin.
# Add 'autoload -U compinit; compinit' to ~/.zshrc if not already present.

# Drop old definitions when the plugin is re-sourced so completion fixes take
# effect without requiring a fresh shell.
for _ralph_fn in \
  _ralph \
  _ralph_subcmd_args \
  _ralph_builtin_hats \
  _ralph_hat_source \
  _ralph_run_args \
  _ralph_preflight_args \
  _ralph_hooks_args \
  _ralph_doctor_args \
  _ralph_init_args \
  _ralph_events_args \
  _ralph_clean_args \
  _ralph_emit_args \
  _ralph_plan_args \
  _ralph_code_task_args \
  _ralph_tui_args \
  _ralph_web_args \
  _ralph_completions_args \
  _ralph_tools_subcmd \
  _ralph_wave_subcmd \
  _ralph_wave_emit_args \
  _ralph_loops_subcmd \
  _ralph_hats_subcmd \
  _ralph_memory_subcmd \
  _ralph_memory_args \
  _ralph_task_subcmd \
  _ralph_task_args \
  _ralph_skill_subcmd \
  _ralph_skill_args; do
  unfunction "$_ralph_fn" 2>/dev/null || true
done
unset _ralph_fn

# =============================================================================
# Builtin Hat Collections
# =============================================================================
# Keep this list in sync with `crates/ralph-cli/src/presets.rs` and install
# updates to `~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`.
#
# P3 #33: the list must mirror `PRESETS` entries where `public: true` in
# `presets.rs`. `merge-loop` is `public: false` (it's an internal
# orchestration helper, not a user-facing preset), so it MUST NOT appear
# here — otherwise TAB completion will offer a preset that
# `ralph preset list` does not show, breaking the discoverability
# invariant.
_RALPH_BUILTIN_HAT_VALUES=(
  "builtin:ce-executor-serial"
  # builtin:ce-executor-wave  # deleted 2026-06-23
  "builtin:debug"
  "builtin:autoresearch"
)

_RALPH_BUILTIN_HAT_DESCRIPTIONS=(
  "Isolated-mode plan-driven work execution with serial code review (no wave), auto-fix, shipping, and manager report"
  # Wave-based parallel plan-driven execution with adversarial review, auto-fix, and shipping  # deleted 2026-06-23
  "Bug investigation, root-cause analysis, and adversarial fix verification"
  "Autonomous experiment loop: try ideas, measure, keep what works"
)

# =============================================================================
# Backend List
# =============================================================================
_RALPH_BACKENDS=(
  "auto:Auto-detect backend"
  "claude:Claude CLI backend"
  "kiro:Kiro backend"
  "gemini:Gemini backend"
  "codex:Codex backend"
  "amp:Amp backend"
  "copilot:Co-pilot backend"
  "opencode:Opencode backend"
  "pi:Pi backend"
  "roo:Roo Code backend"
  "traecli:Trae CLI backend"
  "custom:Custom backend"
)

# =============================================================================
# Main Command List
# =============================================================================
_RALPH_COMMANDS=(
  "run:Run the orchestration loop (default if no subcommand given)"
  "preflight:Run preflight checks to validate configuration and environment"
  "hooks:Validate hooks configuration and command wiring"
  "doctor:Run first-run diagnostics and environment checks"
  "tutorial:Interactive walkthrough of hats, hat collections, and workflow"
  "events:View event history for debugging"
  "init:Initialize new ralph.yml configuration file"
  "clean:Clean up Ralph artifacts from .ralph/agent"
  "emit:Emit an event to the current run's events file with proper JSON formatting"
  "plan:Start a Prompt-Driven Development planning session"
  "code-task:Generate code task files from descriptions or plans"
  "task:Generate code task files (legacy alias)"
  "tools:Ralph runtime tools (agent-facing)"
  "wave:Dispatch wave events for parallel hat execution"
  "loops:Manage parallel loops"
  "hats:Manage configured hats"
  "preset:Manage and validate presets"
  "tui:Attach TUI to a running ralph-api server"
  "web:Run web dashboard"
  "mcp:Run Ralph as an MCP server over stdio"
  "completions:Generate shell completions"
  "help:Show help for a command"
)

# =============================================================================
# Tools Subcommands
# =============================================================================
_RALPH_TOOLS_COMMANDS=(
  "memory:Manage persistent memories"
  "task:Manage work items"
  "skill:Load and manage skills"
)

# =============================================================================
# Wave Subcommands
# =============================================================================
_RALPH_WAVE_CMDS=(
  "emit:Emit multiple events as a wave for parallel execution"
)

# =============================================================================
# Loops Subcommands
# =============================================================================
_RALPH_LOOPS_CMDS=(
  "list:List all loops"
  "logs:View loop output/logs"
  "history:Show event history for a loop"
  "retry:Re-run merge for a failed loop"
  "discard:Abandon loop and clean up worktree"
  "stop:Stop a running loop"
  "resume:Resume a suspended loop"
  "prune:Clean up stale loops"
  "attach:Open shell in loop worktree"
  "diff:Show diff of loop changes"
  "merge:Merge a completed loop"
  "process:Process pending merge queue entries"
  "merge-button-state:Get merge button state for a loop"
)

# =============================================================================
# Hats Subcommands
# =============================================================================
_RALPH_HATS_CMDS=(
  "list:List all configured hats"
  "validate:Validate hat topology and report issues"
  "graph:Display hat topology graph"
  "show:Show detailed configuration for a specific hat"
)

# =============================================================================
# Preset Subcommands
# =============================================================================
_RALPH_PRESET_CMDS=(
  "list:List available workflow templates"
  "show:Show details of a specific template"
  "new:Generate a new preset from a template"
  "check:Check preset/workflow contract (config, topology, payload, orphan)"
  "diff:Show differences between a local preset and its template baseline"
  "upgrade:Preview upgrade information for a local preset (dry-run only)"
)

# Builtin template names — must mirror TemplateCatalog::template_names()
# in crates/ralph-cli/src/preset_templates.rs.  When adding a new builtin
# template, add it here too so `ralph preset <TAB>` still works.
_RALPH_PRESET_TEMPLATES=(
  "minimal-linear:Beginner two-hat linear workflow"
  "debug:Diagnose/fix/exit loop for debugging tasks"
  "ce-executor-lite:Lightweight compound-engineering executor"
)

# =============================================================================
# MCP Subcommands
# =============================================================================
_RALPH_MCP_CMDS=(
  "serve:Start MCP server"
)

# =============================================================================
# Memory Subcommands
# =============================================================================
_RALPH_MEMORY_CMDS=(
  "add:Store a new memory"
  "list:List all memories"
  "show:Show a single memory by ID"
  "delete:Delete a memory by ID"
  "search:Find memories by query"
  "prime:Output memories for context injection"
  "init:Initialize memories file"
)

# =============================================================================
# Task Subcommands
# =============================================================================
_RALPH_TASK_CMDS=(
  "add:Create a new task"
  "ensure:Create or reuse a task by stable key"
  "list:List all tasks"
  "ready:Show unblocked tasks"
  "start:Mark a task as in progress"
  "close:Mark a task as complete"
  "fail:Mark a task as failed"
  "reopen:Reopen a closed or failed task"
  "show:Show a single task"
)

# =============================================================================
# Skill Subcommands
# =============================================================================
_RALPH_SKILL_CMDS=(
  "list:List available skills"
  "load:Load skill by name"
)

# =============================================================================
# Hooks Subcommands
# =============================================================================
_RALPH_HOOKS_CMDS=(
  "validate:Validate hooks configuration and command wiring"
)

# =============================================================================
# Main Completion Function
# =============================================================================
_ralph_builtin_hats() {
  compadd \
    -X 'builtin hat collection' \
    -d _RALPH_BUILTIN_HAT_DESCRIPTIONS \
    -a _RALPH_BUILTIN_HAT_VALUES
}

_ralph_hat_source() {
  local ret=1

  _ralph_builtin_hats && ret=0
  _files && ret=0

  return ret
}

_ralph() {
  local -a _ralph_main_opts
  _ralph_main_opts=(
    '-c+[Core config source]:config source:_default'
    '-H+[Hat collection source]:hat source:_ralph_hat_source'
    '-v[Verbose output]'
    '--color[Color output mode]:mode:(auto always never)'
    '-h[Print help]'
    '--help[Print help]'
    '-V[Print version]'
    '--version[Print version]'
  )

  local curcontext="$curcontext" state line
  typeset -A opt_args

  _ralph_cmds=($_RALPH_COMMANDS)
  curcontext="${curcontext}:ralph-"

  _arguments -C $_ralph_main_opts

  # Only return early for options when not in a special state
  if [[ ${words[CURRENT]} == -* ]]; then
    return
  fi

  case ${CURRENT} in
    1)
      _describe 'ralph command' _ralph_cmds
      ;;
    2)
      case ${words[1]} in
        run|preflight|hooks|doctor|tutorial|events|clean|emit|plan|code-task|task)
          _ralph_subcmd_args ${words[1]}
          ;;
        init)
          _ralph_init_args
          ;;
        tools)
          _describe 'tools command' _RALPH_TOOLS_COMMANDS
          ;;
        wave)
          _describe 'wave command' _RALPH_WAVE_CMDS
          ;;
        loops)
          _describe 'loops command' _RALPH_LOOPS_CMDS
          ;;
        hats)
          _describe 'hats command' _RALPH_HATS_CMDS
          ;;
        preset)
          _describe 'preset command' _RALPH_PRESET_CMDS
          ;;
        tui)
          _ralph_tui_args
          ;;
        web)
          _ralph_web_args
          ;;
        mcp)
          _describe 'mcp command' _RALPH_MCP_CMDS
          ;;
        completions)
          _ralph_completions_args
          ;;
      esac
      ;;
    *)
      case ${words[1]} in
        tools)
          _ralph_tools_subcmd ${words[2]} ${words[CURRENT]}
          ;;
        wave)
          _ralph_wave_subcmd ${words[2]}
          ;;
        loops)
          _ralph_loops_subcmd ${words[2]}
          ;;
        hats)
          _ralph_hats_subcmd ${words[2]}
          ;;
        preset)
          _ralph_preset_subcmd ${words[2]} ${words[CURRENT]}
          ;;
      esac
      ;;
  esac
}

# =============================================================================
# Subcommand Argument Helpers
# =============================================================================
(( $+functions[_ralph_subcmd_args] )) ||
_ralph_subcmd_args() {
  local cmd=$1
  case $cmd in
    run)
      _ralph_run_args
      ;;
    preflight)
      _ralph_preflight_args
      ;;
    hooks)
      _ralph_hooks_args
      ;;
    doctor)
      _ralph_doctor_args
      ;;
    events)
      _ralph_events_args
      ;;
    clean)
      _ralph_clean_args
      ;;
    emit)
      _ralph_emit_args
      ;;
    plan)
      _ralph_plan_args
      ;;
    code-task|task)
      _ralph_code_task_args
      ;;
  esac
}

# =============================================================================
# Run Command Arguments
# =============================================================================
(( $+functions[_ralph_run_args] )) ||
_ralph_run_args() {
  local -a run_opts
  run_opts=(
    '-p+[Inline prompt text]:prompt text:_default'
    '-P+[Prompt file path]:prompt file:_files'
    '-b+[Backend to use]:backend:->backend_list'
    '-H+[Hat collection source]:hat source:_ralph_hat_source'
    '--max-iterations+[Override max iterations]:iterations:_default'
    '--completion-promise+[Override completion promise]:promise:_default'
    '--dry-run[Show what would be executed]'
    '--continue[Continue from existing scratchpad]'
    '--loop-id+[Explicit loop ID]:loop id:_default'
    '--no-tui[Disable TUI observation mode]'
    '-a[Force autonomous mode]'
    '--autonomous[Force autonomous mode]'
    '--rpc[Run in RPC mode]'
    '--legacy-tui[Use legacy in-process TUI mode]'
    '--idle-timeout+[Idle timeout in seconds]:seconds:_default'
    '--exclusive[Wait for primary loop slot]'
    '--no-auto-merge[Skip automatic merge]'
    '--worktree[Create isolated worktree for run]'
    '--skip-preflight[Skip preflight checks]'
    '-v[Enable verbose output]'
    '-q[Suppress streaming output]'
    '--record-session+[Record session to JSONL]:file:_files'
    '*:custom args:_default'
  )

  local curcontext="$curcontext" state line
  typeset -A opt_args

  _arguments -C $run_opts

  case $state in
    backend_list)
      _describe 'backend' _RALPH_BACKENDS
      ;;
  esac
}

# =============================================================================
# Preset Subcommand Arguments
# =============================================================================
(( $+functions[_ralph_preset_subcmd] )) ||
_ralph_preset_subcmd() {
  local subcmd=$1
  local word_index=$2

  case $subcmd in
    list)
      local -a list_opts
      list_opts=(
        '--format+[Output format]:format:(human json)'
      )
      _arguments $list_opts
      ;;
    show)
      local -a show_opts
      show_opts=(
        '1:template:_ralph_preset_template'
        '--format+[Output format]:format:(human yaml json)'
      )
      _arguments $show_opts
      ;;
    new)
      local -a new_opts
      new_opts=(
        '1:template:_ralph_preset_template'
        '--name+[Name for the generated preset]:name:_default'
        '--description+[Description for the generated preset]:description:_default'
        '--output+[Output file path]:file:_files'
        '--force[Force overwrite if output file exists]'
        '--check[Run authoring checks after generation]'
        '--format+[Output format]:format:(human json)'
      )
      _arguments $new_opts
      ;;
    check)
      local -a check_opts
      check_opts=(
        '--format+[Output format]:format:(human json)'
        '--strict[Treat warnings as failures]'
      )
      _arguments $check_opts
      ;;
    diff)
      local -a diff_opts
      diff_opts=(
        '--file+[Path to the local preset file]:file:_files'
        '--format+[Output format]:format:(human json)'
      )
      _arguments $diff_opts
      ;;
    upgrade)
      local -a upgrade_opts
      upgrade_opts=(
        '--file+[Path to the local preset file]:file:_files'
        '--format+[Output format]:format:(human json)'
        '--dry-run[Preview upgrade without writing changes]'
        '--force[Apply upgrade even if there are user changes (not implemented in MVP)]'
      )
      _arguments $upgrade_opts
      ;;
  esac
}

(( $+functions[_ralph_preset_template] )) ||
_ralph_preset_template() {
  _describe 'template' _RALPH_PRESET_TEMPLATES
}

# =============================================================================
# Preflight Command Arguments
# =============================================================================
(( $+functions[_ralph_preflight_args] )) ||
_ralph_preflight_args() {
  local -a preflight_opts
  preflight_opts=(
    '--check+[Run only specific check(s)]:check name:_default'
    '--format+[Output format]:format:(human json)'
    '--strict[Treat warnings as failures]'
    '-v[Verbose output]'
  )
  _arguments $preflight_opts
}

# =============================================================================
# Hooks Command Arguments
# =============================================================================
(( $+functions[_ralph_hooks_args] )) ||
_ralph_hooks_args() {
  _describe 'hooks command' _RALPH_HOOKS_CMDS
}

# =============================================================================
# Doctor Command Arguments
# =============================================================================
(( $+functions[_ralph_doctor_args] )) ||
_ralph_doctor_args() {
  local -a doctor_opts
  doctor_opts=(
    '--fix[Attempt to fix detected issues]'
    '-v[Verbose output]'
  )
  _arguments $doctor_opts
}

# =============================================================================
# Init Command Arguments
# =============================================================================
(( $+functions[_ralph_init_args] )) ||
_ralph_init_args() {
  local -a init_opts
  init_opts=(
    '--backend+[Backend to use]:backend:->backend_list'
    '--list-presets[List all available builtin hat collections]'
    '--force[Overwrite existing ralph.yml]'
  )
  _arguments -C $init_opts

  case $state in
    backend_list)
      _describe 'backend' _RALPH_BACKENDS
      ;;
  esac
}

# =============================================================================
# Events Command Arguments
# =============================================================================
(( $+functions[_ralph_events_args] )) ||
_ralph_events_args() {
  local -a events_opts
  events_opts=(
    '--last+[Show only last N events]:number:_default'
    '--topic+[Filter by topic]:topic:_default'
    '--iteration+[Filter by iteration number]:number:_default'
    '--format+[Output format]:format:(table json)'
    '--file+[Path to events file]:file:_files'
    '--clear[Clear the event history. Requires --confirm <loop_id> to authorize.]'
    '--confirm+[Confirmation token: must equal the active loop id (or "current"/"default" when no marker exists)]:loop_id:_default'
  )
  _arguments $events_opts
}

# =============================================================================
# Clean Command Arguments
# =============================================================================
(( $+functions[_ralph_clean_args] )) ||
_ralph_clean_args() {
  local -a clean_opts
  clean_opts=(
    '--dry-run[Preview what would be deleted]'
    '--diagnostics[Clean diagnostic logs]'
  )
  _arguments $clean_opts
}

# =============================================================================
# Emit Command Arguments
# =============================================================================
(( $+functions[_ralph_emit_args] )) ||
_ralph_emit_args() {
  local -a emit_opts
  emit_opts=(
    '1:Topic (e.g. build.done):_default'
    '2:Payload (optional):_default'
    '-j[Parse payload as JSON]'
    '--file+[Path to events file]:file:_files'
    '--policy-check[Validate against event policy]'
    '--unsafe-no-policy-check[Bypass the mandatory policy check (only honored if config permits)]'
    '--hat+[Hat that published this event]:hat:_default'
    '--triggered+[Target hat triggered]:hat:_default'
    '--source+[Source identifier]:source:_default'
  )
  _arguments $emit_opts
}

# =============================================================================
# Plan Command Arguments
# =============================================================================
(( $+functions[_ralph_plan_args] )) ||
_ralph_plan_args() {
  local -a plan_opts
  plan_opts=(
    '-b+[Backend to use]:backend:->backend_list'
    '--teams[Enable Claude Code Agent Teams]'
    '1:Idea (optional):_default'
  )
  _arguments -C $plan_opts

  case $state in
    backend_list)
      _describe 'backend' _RALPH_BACKENDS
      ;;
  esac
}

# =============================================================================
# Code Task Command Arguments
# =============================================================================
(( $+functions[_ralph_code_task_args] )) ||
_ralph_code_task_args() {
  local -a code_task_opts
  code_task_opts=(
    '-b+[Backend to use]:backend:->backend_list'
    '--teams[Enable Claude Code Agent Teams]'
    '1:Input (description text or path to plan):_default'
  )
  _arguments -C $code_task_opts

  case $state in
    backend_list)
      _describe 'backend' _RALPH_BACKENDS
      ;;
  esac
}

# =============================================================================
# TUI Command Arguments
# =============================================================================
(( $+functions[_ralph_tui_args] )) ||
_ralph_tui_args() {
  _arguments \
    '-u+[ralph-api server URL]:url:_default' \
    '--url+[ralph-api server URL]:url:_default'
}

# =============================================================================
# Web Command Arguments
# =============================================================================
(( $+functions[_ralph_web_args] )) ||
_ralph_web_args() {
  local -a web_opts
  web_opts=(
    '--port+[Port to run on]:port:_default'
    '--host+[Host to bind to]:host:_default'
  )
  _arguments $web_opts
}

# =============================================================================
# Completions Command Arguments
# =============================================================================
(( $+functions[_ralph_completions_args] )) ||
_ralph_completions_args() {
  local -a shell_cmds
  shell_cmds=(
    'bash:Bash shell'
    'zsh:Zsh shell'
    'fish:Fish shell'
    'powershell:PowerShell'
  )
  _describe 'shell' shell_cmds
}

# =============================================================================
# Tools Subcommand Dispatcher
# =============================================================================
(( $+functions[_ralph_tools_subcmd] )) ||
_ralph_tools_subcmd() {
  local subcmd=$1

  case $subcmd in
    memory)
      _ralph_memory_subcmd
      ;;
    task)
      _ralph_task_subcmd
      ;;
    skill)
      _ralph_skill_subcmd
      ;;
  esac
}

# =============================================================================
# Wave Subcommand Dispatcher
# =============================================================================
(( $+functions[_ralph_wave_subcmd] )) ||
_ralph_wave_subcmd() {
  local subcmd=$1

  case $subcmd in
    emit)
      _ralph_wave_emit_args
      ;;
  esac
}

# =============================================================================
# Wave Emit Arguments
# =============================================================================
(( $+functions[_ralph_wave_emit_args] )) ||
_ralph_wave_emit_args() {
  _arguments \
    '1:Topic (e.g. review.file):_default' \
    '--payloads+[Payload items for parallel workers]:payload items:_files' \
    '--payloads-stdin[Read payloads from stdin, one per line]' \
    '--output+[Output format]:output:(text json)' \
    '--idempotency-key+[Idempotency key for retry-safe re-emission]:key:_default' \
    '--policy-check[Validate all payloads against event policy before writing]' \
    '--unsafe-no-policy-check[Bypass the mandatory policy check (only honored if config permits)]' \
    '*:payload args:_default'
}

# =============================================================================
# Loops Subcommand Dispatcher
# =============================================================================
(( $+functions[_ralph_loops_subcmd] )) ||
_ralph_loops_subcmd() {
  local subcmd=$1

  case $subcmd in
    list|logs|history|retry|discard|stop|resume|prune|attach|diff|merge|process|merge-button-state)
      _arguments '1:Loop ID:_default'
      ;;
  esac
}

# =============================================================================
# Hats Subcommand Dispatcher
# =============================================================================
(( $+functions[_ralph_hats_subcmd] )) ||
_ralph_hats_subcmd() {
  local subcmd=$1

  case $subcmd in
    show)
      _arguments '1:Hat name:_default'
      ;;
  esac
}

# =============================================================================
# Memory Subcommands
# =============================================================================
(( $+functions[_ralph_memory_subcmd] )) ||
_ralph_memory_subcmd() {
  local -a memory_cmds
  memory_cmds=($_RALPH_MEMORY_CMDS)

  local curcontext="$curcontext" state line
  typeset -A opt_args

  _arguments -C \
    '-t+[Memory type]:type:(pattern decision fix context)' \
    '--tags+[Tags to filter by]:tags:_default' \
    '--format+[Output format]:format:(table json markdown quiet)' \
    '--root+[Working directory]:directory:_files -/' \
    '1:subcommand:->subcmd'

  case $state in
    subcmd)
      _describe 'memory subcommand' memory_cmds
      ;;
  esac
}

# =============================================================================
# Memory Sub-Subcommand Arguments
# =============================================================================
(( $+functions[_ralph_memory_args] )) ||
_ralph_memory_args() {
  local subcmd=$1

  case $subcmd in
    add)
      _arguments -C \
        '-t+[Memory type]:type:(pattern decision fix context)' \
        '--tags+[Tags to filter by]:tags:_default' \
        '*:content:_default'
      ;;
    show|delete)
      _arguments '1:memory id:_default'
      ;;
    search)
      _arguments '1:query:_default'
      ;;
    list)
      _arguments -C \
        '-t+[Memory type]:type:(pattern decision fix context)' \
        '--tags+[Tags to filter by]:tags:_default'
      ;;
    prime)
      _arguments -C \
        '--budget+[Budget for context injection]:budget:_default' \
        '-t+[Memory type]:type:(pattern decision fix context)'
      ;;
  esac
}

# =============================================================================
# Task Subcommands
# =============================================================================
(( $+functions[_ralph_task_subcmd] )) ||
_ralph_task_subcmd() {
  local -a task_cmds
  task_cmds=($_RALPH_TASK_CMDS)

  local curcontext="$curcontext" state line
  typeset -A opt_args

  _arguments -C \
    '-p+[Priority]:priority:(1 2 3 4 5)' \
    '-d+[Description]:description:_default' \
    '--blocked-by+[Blocked by task IDs]:ids:_default' \
    '--key+[Stable task key]:key:_default' \
    '--format+[Output format]:format:(table json quiet)' \
    '--root+[Working directory]:directory:_files -/' \
    '1:subcommand:->subcmd'

  case $state in
    subcmd)
      _describe 'task subcommand' task_cmds
      ;;
  esac
}

# =============================================================================
# Task Sub-Subcommand Arguments
# =============================================================================
(( $+functions[_ralph_task_args] )) ||
_ralph_task_args() {
  local subcmd=$1

  case $subcmd in
    add|ensure)
      _arguments -C \
        '-p+[Priority]:priority:(1 2 3 4 5)' \
        '-d+[Description]:description:_default' \
        '--blocked-by+[Blocked by task IDs]:ids:_default' \
        '--key+[Stable task key]:key:_default' \
        '1:title:_default'
      ;;
    start|close|fail|reopen|show)
      _arguments '1:task id:_default'
      ;;
    list)
      _arguments -C \
        '--status+[Filter by status]:status:(open in_progress closed)'
      ;;
  esac
}

# =============================================================================
# Skill Subcommands
# =============================================================================
(( $+functions[_ralph_skill_subcmd] )) ||
_ralph_skill_subcmd() {
  local -a skill_cmds
  skill_cmds=($_RALPH_SKILL_CMDS)

  local curcontext="$curcontext" state line
  typeset -A opt_args

  _arguments -C \
    '1:subcommand:->subcmd'

  case $state in
    subcmd)
      _describe 'skill subcommand' skill_cmds
      ;;
  esac
}

# =============================================================================
# Skill Sub-Subcommand Arguments
# =============================================================================
(( $+functions[_ralph_skill_args] )) ||
_ralph_skill_args() {
  local subcmd=$1

  case $subcmd in
    load)
      _arguments '1:skill name:_default'
      ;;
  esac
}

# =============================================================================
# Register completion
# =============================================================================
# Ensure compdef is available, then register completion for ralph
(( ${+functions[compdef]} )) && compdef _ralph ralph

# =============================================================================
# Aliases for Common Commands
# =============================================================================
alias r='ralph'
alias rr='ralph run'
alias re='ralph emit'
alias rloops='ralph loops'
alias rhats='ralph hats'
alias rtools='ralph tools'
alias rwave='ralph wave'
alias rskill='ralph tools skill'
alias rmem='ralph tools memory'
alias rtask='ralph tools task'
