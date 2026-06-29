# Troubleshooting Guide

## Common Issues and Solutions

### Installation Issues

#### Agent Not Found

**Problem**: `ralph: command 'claude' not found`

**Solutions**:

1. Verify agent installation:

   ```bash
   which claude
   which gemini
   which q
   ```

2. Install missing agent:

   ```bash
   # Claude
   npm install -g @anthropic-ai/claude-code

   # Gemini
   npm install -g @google/gemini-cli
   ```

3. Add to PATH:

   ```bash
   export PATH=$PATH:/usr/local/bin
   ```

#### Permission Denied

**Problem**: `Permission denied: './ralph'`

**Solution**:

```bash
chmod +x ralph
```

### Configuration Issues

#### Config File Exists

**Problem**: `ralph.yml already exists. Use --force to overwrite.`

**Solutions**:

1. Overwrite the existing file:

   ```bash
   ralph init --backend claude --force
   ```

2. Move or rename the existing config:

   ```bash
   mv ralph.yml ralph.yml.bak
   ```

3. Use a different config file:

   ```bash
   ralph run -c path/to/other.yml
   ```

#### Config Not Found

**Problem**: `Config file not found: ralph.yml`

**Solutions**:

1. Verify the path:

   ```bash
   ls -la ralph.yml
   ```

2. Generate a config:

   ```bash
   ralph init --backend claude
   ```

3. Use defaults by omitting the config flag:

   ```bash
   ralph run
   ```

#### Unknown Backend

**Problem**: `Unknown backend 'foo'`

**Solutions**:

1. Use a supported backend:

   ```bash
   ralph init --backend claude
   ralph init --backend gemini
   ralph init --backend codex
   ```

2. List presets (includes backend hints):

   ```bash
   ralph init --list-presets
   ```

#### Unknown Preset

**Problem**: `Unknown preset 'foo'`

**Solutions**:

1. List presets:

   ```bash
   ralph init --list-presets
   ```

2. Use a known built-in hat collection:

   ```bash
   ralph init --backend claude
   ralph run -c ralph.yml -H builtin:ce-executor-serial
   ```

#### Custom Backend Command

**Problem**: `Custom backend requires a command`

**Solutions**:

1. Add a command to your config:

   ```yaml
   cli:
     backend: "custom"
     command: "my-agent"
     prompt_mode: "stdin" # or "arg"
   ```

2. Generate a template:

   ```bash
   ralph init --backend custom
   ```

#### Ambiguous Routing

**Problem**: `Ambiguous routing: trigger 'build.done' is claimed by both 'builder' and 'reviewer'`

**Solutions**:

1. Ensure only one hat claims each trigger:

   ```yaml
   hats:
     builder:
       triggers: ["build.task"]
       publishes: ["build.done"]
     reviewer:
       triggers: ["review.request"]
       publishes: ["review.done"]
   ```

2. Use delegated events (e.g., `work.start`) instead of reusing core events.

#### Reserved Trigger

**Problem**: `Reserved trigger 'task.start' used by hat 'builder'`

**Solutions**:

1. Replace reserved triggers with custom events:

   ```yaml
   hats:
     builder:
       triggers: ["work.start"]
       publishes: ["work.done"]
   ```

#### Missing Hat Description

**Problem**: `Hat 'builder' is missing required 'description' field`

**Solution**:

```yaml
hats:
  builder:
    description: "Implements code changes for assigned tasks"
```

#### Mutually Exclusive Fields

**Problem**: `Mutually exclusive fields: 'prompt' and 'prompt_file' cannot both be specified`

**Solution**:

- Use either `prompt` **or** `prompt_file` in `event_loop`, not both.

#### RObot Config

> **Removed in the 2026-06-25 refactor (plan 2026-06-25-001).** The `RObot` block
> no longer exists; if your `ralph.yml` still declares it, the field is rejected as
> `unknown field` on the next run. Strip the block.

For runtime-diagnosis recovery (3-strike escalation, completion-correction injection,
drift journals), see `docs/guide/runtime-diagnosis.md` and the surviving
`task.resume` channel. (`human.guidance` was removed by plan
2026-06-28-005; the recovery channel is now `task.resume` plus
`TerminationReason::RecoveryExhausted`.)

#### Multi-Hat Isolation Policy Violation

**Problem**: `ralph preset check`, `ralph preflight`, or `ralph run` fails with the
literal error:

```
preset declares N hats which exceeds the coordinator limit of 3;
set `event_loop.execution_mode: isolated` to run this preset
```

This is a hard cap, not a warning. Adding a 4th hat without flipping
`execution_mode` to `isolated` is the most common cause.

**Fix** (in your preset YAML):

```yaml
event_loop:
  execution_mode: isolated
```

There is no opt-out: no environment variable, test toggle, or per-preset exemption
can disable this rule. The `3`-hat coordinator cap is enforced by
`preset_lint::check_multi_hat_isolation` at every entry point — `ralph preset
check`, `ralph preflight`, and the run startup lint gate — so the loop never
enters a half-started state.

If you are migrating a preset that previously ran under the coordinator mode, you
must also verify that each hat's terminal topics (e.g. `LOOP_COMPLETE`,
`review.complete`, `report.done`) appear in the hat's `publishes` list. U3 closes
the historic exemption that let any hat publish the completion promise; in
isolated mode only the named completion owner may emit the terminal topic.
Unauthorised attempts are rejected with `event.isolation.boundary_violation`.

See the [Multi-Hat Isolation Policy](../guide/configuration.md#multi-hat-isolation-policy-mandatory)
section in `configuration.md` for the full rule, the per-turn budget, and the
round-robin scheduling semantics that go with isolated mode.

### Execution Issues

#### Task Running Too Long

**Problem**: Ralph runs maximum iterations without achieving goals

**Possible Causes**:

1. Unclear or overly complex task description
2. Agent not making progress towards objectives
3. Task scope too large for iteration limits

**Solutions**:

1. Check iteration progress and logs:

   ```bash
   ralph status
   ```

2. Break down complex tasks:

   ```markdown
   # Instead of:

   Build a complete web application

   # Try:

   Create a Flask app with one endpoint that returns "Hello World"
   ```

3. Increase iteration limits or try different agent:

   ```bash
   ralph run --max-iterations 200
   ralph run --agent gemini
   ```

#### Agent Timeout

**Problem**: `Agent execution timed out`

**Solutions**:

1. Increase the adapter inactivity timeout:

   ```yaml
   # In ralph.yml
   adapters:
     claude:
       timeout: 600
   ```

2. Reduce prompt complexity:
   - Break large tasks into smaller ones
   - Remove unnecessary context

3. Check system resources:

   ```bash
   htop
   free -h
   ```

#### Repeated Errors

**Problem**: Same error occurs in multiple iterations

**Solutions**:

1. Check error pattern:

   ```bash
   cat .agent/metrics/state_*.json | jq '.errors'
   ```

2. Clear workspace and retry:

   ```bash
   ralph clean
   ralph run
   ```

3. Manual intervention:
   - Fix the specific issue
   - Add clarification to PROMPT.md
   - Resume execution

#### Loop Detection Issues

**Problem**: `Loop detected: XX% similarity to previous output`

Ralph's loop detection triggers when agent output is ≥90% similar to any of the last 5 outputs.

**Possible Causes**:

1. Agent is stuck on the same subtask
2. Agent producing similar "working on it" messages
3. API errors causing identical retry messages
4. Task requires same action repeatedly (false positive)

**Solutions**:

1. **Check if it's a legitimate loop**:

   ```bash
   # Review recent outputs
   ls -lt .agent/prompts/ | head -10
   diff .agent/prompts/prompt_N.md .agent/prompts/prompt_N-1.md
   ```

2. **Improve prompt to encourage variety**:

   ```markdown
   # Add explicit progress tracking

   ## Current Status

   Document what step you're on and what has changed since last iteration.
   ```

3. **Break down the task**:
   - If agent keeps doing the same thing, the task may need restructuring
   - Split into smaller, more distinct subtasks

4. **Check for underlying issues**:
   - API errors causing retries
   - Permission issues blocking progress
   - Missing dependencies

#### Completion Marker Not Detected

**Problem**: Ralph continues running despite `TASK_COMPLETE` marker

**Possible Causes**:

1. Incorrect marker format
2. Invisible characters or encoding issues
3. Marker buried in code block

**Solutions**:

1. **Use exact format**:

   ```markdown
   # Correct formats:

   - [x] TASK_COMPLETE
         [x] TASK_COMPLETE

   # Incorrect (won't trigger):

   - [ ] TASK_COMPLETE # Not checked
         TASK_COMPLETE # No checkbox
   - [x] TASK_COMPLETE # Capital X
   ```

2. **Check for hidden characters**:

   ```bash
   cat -A PROMPT.md | grep TASK_COMPLETE
   ```

3. **Ensure marker is on its own line**:

   ````markdown
   # Good - on its own line

   - [x] TASK_COMPLETE

   # Bad - inside code block

   ```markdown
   - [x] TASK_COMPLETE # Inside code block - won't work
   ```
   ````

   ```

   ```

4. **Verify encoding**:

   ```bash
   file PROMPT.md
   # Should show: UTF-8 Unicode text
   ```

#### LOOP_COMPLETE Rejected / Post-Completion Retry Loop

**Problem**: After emitting `LOOP_COMPLETE`, the loop continues with `task.resume` messages and never terminates.

**Possible Causes**:

1. **Missing required events** -- The preset defines `required_events` and not all have been emitted yet. The completion gate is all-of: every listed topic must have appeared at least once.
2. **Workflow guard incomplete** -- A `workflow_guards` chain has started instances that have not reached their terminal phase.
3. **Verdict gate failure** -- A `verdict_gate` is configured and the most recent verdict event carries a failing value.
4. **Runtime tasks still open** -- In memories/tasks mode, uncompleted tasks block completion.
5. **Persistent mode** -- `persistent: true` suppresses completion and keeps the loop alive for new work.

**Diagnosis**:

1. **Check the task.resume payload** -- The rejected completion injects a `task.resume` event with a message explaining why. Look for patterns like:
   - `missing required events: ["report.done"]` -- see cause 1
   - `incomplete workflow guard instances` -- see cause 2
   - `verdict gate observed a failing verdict` -- see cause 3
   - `open task(s)` -- see cause 4

2. **Check which required events have been seen**:

   ```bash
   # Review the events file for emitted topics
   cat .ralph/events.jsonl | jq -r '.topic' | sort -u
   ```

3. **Check the topology**:

   ```bash
   ralph hats validate -H presets/my-workflow.yml
   ```

4. **Check the logs for stale-breaker termination**:

   ```bash
   # Look for stale-breaker messages
   grep "Stale-breaker" .ralph/diagnostics/logs/*.log
   ```

**Solutions**:

1. **Emit the missing required events** -- Ensure every hat in the workflow emits its required convergence topic. If the reporter hat is not emitting `report.done`, add the emit to its instructions.
2. **Complete all workflow guard instances** -- Emit the terminal event for each started instance.
3. **Fix the verdict** -- Re-run the review and emit a passing verdict.
4. **Close open tasks** -- Mark tasks as done before emitting completion.
5. **Disable persistent mode** -- Remove `persistent: true` from config if not needed.

**Stale-breaker safety valve**:

If the same completion rejection repeats **3 or more times with no new business events** between rejections, the loop terminates with `TerminationReason::LoopStale` to prevent infinite API-burning loops. The stale-breaker tracks a rejection signature (e.g., `missing_required:report.done`) and counts consecutive rejections with identical signatures and no new topics observed.

To avoid stale-breaker termination, either:
- Emit the missing required event (root cause fix), or
- Use `loop.cancel` to abort the workflow intentionally.

### Git Issues

#### Checkpoint Failed

**Problem**: `Failed to create checkpoint`

**Solutions**:

1. Initialize Git repository:

   ```bash
   git init
   git add .
   git commit -m "Initial commit"
   ```

2. Check Git status:

   ```bash
   git status
   ```

3. Fix Git configuration:

   ```bash
   git config user.email "you@example.com"
   git config user.name "Your Name"
   ```

#### Uncommitted Changes Warning

**Problem**: `Uncommitted changes detected`

**Solutions**:

1. Commit changes:

   ```bash
   git add .
   git commit -m "Save work"
   ```

2. Stash changes:

   ```bash
   git stash
   ralph run
   git stash pop
   ```

3. Disable Git operations:

   ```bash
   ralph run --no-git
   ```

### Context Issues

#### Context Window Exceeded

**Problem**: `Context window limit exceeded`

**Symptoms**:

- Agent forgets earlier instructions
- Incomplete responses
- Errors about missing information

**Solutions**:

1. Reduce file sizes:

   ```bash
   # Split large files
   split -l 500 large_file.py part_
   ```

2. Use more concise prompt:

   ```markdown
   # Remove unnecessary details

   # Focus on current task
   ```

3. Switch to higher-context agent:

   ```bash
   # Claude has 200K context
   ralph run --agent claude
   ```

4. Clear iteration history:

   ```bash
   rm .agent/prompts/prompt_*.md
   ```

### Performance Issues

#### Slow Execution

**Problem**: Iterations taking too long

**Solutions**:

1. Check system resources:

   ```bash
   top
   df -h
   iostat
   ```

2. Reduce parallel operations:
   - Close other applications
   - Limit background processes

3. Use faster agent:

   ```bash
   # Q is typically faster
   ralph run --agent q
   ```

#### High Memory Usage

**Problem**: Ralph consuming excessive memory

**Solutions**:

1. Set resource limits:

   ```python
   # In ralph.json
   {
     "resource_limits": {
       "memory_mb": 2048
     }
   }
   ```

2. Clean old state files:

   ```bash
   find .agent -name "*.json" -mtime +7 -delete
   ```

3. Restart Ralph:

   ```bash
   pkill -f ralph_orchestrator
   ralph run
   ```

### State and Metrics Issues

#### Corrupted State File

**Problem**: `Invalid state file`

**Solutions**:

1. Remove corrupted file:

   ```bash
   rm .agent/metrics/state_latest.json
   ```

2. Restore from backup:

   ```bash
   cp .agent/metrics/state_*.json .agent/metrics/state_latest.json
   ```

3. Reset state:

   ```bash
   ralph clean
   ```

#### Missing Metrics

**Problem**: No metrics being collected

**Solutions**:

1. Check metrics directory:

   ```bash
   ls -la .agent/metrics/
   ```

2. Create directory if missing:

   ```bash
   mkdir -p .agent/metrics
   ```

3. Check permissions:

   ```bash
   chmod 755 .agent/metrics
   ```

## Error Messages

### Common Error Codes

| Error           | Meaning                | Solution               |
| --------------- | ---------------------- | ---------------------- |
| `Exit code 1`   | General failure        | Check logs for details |
| `Exit code 130` | Interrupted (Ctrl+C)   | Normal interruption    |
| `Exit code 137` | Killed (out of memory) | Increase memory limits |
| `Exit code 124` | Timeout                | Increase timeout value |

### Agent-Specific Errors

#### Claude Errors

```
"Rate limit exceeded"
```

**Solution**: Add delay between iterations or upgrade API plan

```
"Invalid API key"
```

**Solution**: Check Claude CLI configuration

#### Gemini Errors

```
"Quota exceeded"
```

**Solution**: Wait for quota reset or upgrade plan

```
"Model not available"
```

**Solution**: Check Gemini CLI version and update

#### Q Chat Errors

```
"Connection refused"
```

**Solution**: Ensure Q service is running

## Debug Mode

### Enable Verbose Logging

```bash
# Maximum verbosity
ralph run --verbose

# With debug environment
DEBUG=1 ralph run

# Save logs
ralph run --verbose 2>&1 | tee debug.log
```

### Inspect Execution

```python
# Add debug points in PROMPT.md
print("DEBUG: Reached checkpoint 1")
```

### Trace Execution

```bash
# Trace system calls
strace -o trace.log ralph run

# Profile Python execution
python -m cProfile ralph_orchestrator.py
```

## Recovery Procedures

### From Failed State

1. **Save current state**:

   ```bash
   cp -r .agent .agent.backup
   ```

2. **Analyze failure**:

   ```bash
   tail -n 100 .agent/logs/ralph.log
   ```

3. **Fix issue**:
   - Update PROMPT.md
   - Fix code errors
   - Clear problematic files

4. **Resume or restart**:

   ```bash
   # Resume from checkpoint
   ralph run

   # Or start fresh
   ralph clean && ralph run
   ```

### From Git Checkpoint

```bash
# List checkpoints
git log --oneline | grep checkpoint

# Reset to checkpoint
git reset --hard <commit-hash>

# Resume execution
ralph run
```

## Getting Help

### Self-Diagnosis

Run the diagnostic script:

```bash
cat > diagnose.sh << 'EOF'
#!/bin/bash
echo "Ralph Orchestrator Diagnostic"
echo "============================"
echo "Agents available:"
which claude && echo "  ✓ Claude" || echo "  ✗ Claude"
which gemini && echo "  ✓ Gemini" || echo "  ✗ Gemini"
which q && echo "  ✓ Q" || echo "  ✗ Q"
echo ""
echo "Git status:"
git status --short
echo ""
echo "Ralph status:"
./ralph status
echo ""
echo "Recent errors:"
grep ERROR .agent/logs/*.log 2>/dev/null | tail -5
EOF
chmod +x diagnose.sh
./diagnose.sh
```

### Community Support

1. **GitHub Issues**: [Report bugs](https://github.com/mikeyobrien/ralph-orchestrator/issues)
2. **Discussions**: [Ask questions](https://github.com/mikeyobrien/ralph-orchestrator/discussions)
3. **Discord**: Join the community chat

### Reporting Bugs

Include in bug reports:

1. Ralph version: `ralph --version`
2. Agent versions
3. Error messages
4. PROMPT.md content
5. Diagnostic output
6. Steps to reproduce

### Loop Stale Detection

**Problem**: Loop terminates with `Failed: stale loop detected` or `LoopStale` in diagnostics.

**Explanation**: Ralph's stale-breaker mechanism automatically terminates a loop when the same completion rejection repeats 3 times without meaningful progress. This prevents infinite API-burning loops when the loop can't reach its completion gate.

**Common causes:**

1. **Missing required events** -- `required_events` lists topics that no completion path emits. Run `ralph hats validate` to check topology.
2. **Open runtime tasks** -- Tasks from `ralph tools task ensure` are still open. Close them with `ralph tools task close <id>` or complete the work.
3. **Incomplete workflow guards** -- A guarded chain (e.g., experiment phases) is stuck at an intermediate phase. Advance the chain by emitting the next expected event.
4. **Verdict gate rejection** -- The configured `verdict_gate` observed a failing verdict (e.g., `review.complete` with `pass_or_fail: fail`).

**Diagnostics:**

Check diagnostics for the rejection reason:

```bash
jq 'select(.reason == "LoopStale")' .ralph/diagnostics/*/orchestration.jsonl
```

Look for `completion_rejection_signature` in the output to see what was blocking completion:

- `missing_required:<topics>` -- Required events not seen
- `open_tasks:<count>:<hash>` -- Open tasks blocking completion
- `workflow_guard:<message>` -- Incomplete workflow chain
- `verdict_fail:<topic>` -- Verdict gate rejection

**Fixes:**

```bash
# Check topology
ralph hats validate -H builtin:ce-executor-serial
ralph hats validate -H builtin:ce-executor-serial

# Check open tasks
ralph tools task list

# Check preflight
ralph preflight -c ralph.yml -H builtin:ce-executor-serial
ralph preflight -c ralph.yml -H builtin:ce-executor-serial
```

See [Loop Detection](../advanced/loop-detection.md) for the technical details of backlog detection and [Presets](../guide/presets.md) for completion gate configuration.

## Prevention Tips

### Best Practices

1. **Start simple**: Test with basic tasks first
2. **Regular checkpoints**: Use default 5-iteration interval
3. **Monitor progress**: Check status frequently
4. **Version control**: Commit before running Ralph
5. **Resource limits**: Set appropriate limits
6. **Clear requirements**: Write specific, testable criteria

### Pre-flight Checklist

Before running Ralph:

- [ ] PROMPT.md is clear and specific
- [ ] Git repository is clean
- [ ] Agents are installed and working
- [ ] Sufficient disk space available
- [ ] No sensitive data in prompt
- [ ] Backup important files
