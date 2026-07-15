# Cost Management Guide

Effective cost management is crucial when running AI orchestration at scale. This guide helps you optimize spending while maintaining task quality with the Rust `ralph` CLI.

## Understanding Costs

### Backend Pricing Guidance

Actual pricing depends on your API provider and model. The table below gives rough guidance for comparing backends in typical Ralph tasks:

| Backend | Relative Input Cost | Relative Output Cost | Typical Range / Task |
|---------|---------------------|----------------------|----------------------|
| **Claude** | High | High | $5 - $50 |
| **Gemini** | Low-Medium | Low-Medium | $1 - $15 |
| **Codex** | Medium-High | Medium-High | $3 - $30 |
| **OpenCode** | Low | Low | $1 - $10 |
| **Pi** | Low | Low | $1 - $10 |
| **Trae CLI** | Varies | Varies | Varies |
| **Custom** | Varies | Varies | Varies |

> These are illustrative ranges. Always check your provider's current pricing and bill against your actual usage.

### Cost Calculation

For a single API call:

```
total_cost = (input_tokens / 1_000_000 * input_price_per_1m) +
             (output_tokens / 1_000_000 * output_price_per_1m)
```

**Example:**
- Task uses 100K input tokens and 50K output tokens
- With a $3 / 1M input and $15 / 1M output backend: (0.1 × $3) + (0.05 × $15) = $1.05

Ralph does not bill you directly; it routes calls to the backend configured in your preset or adapter config. Cost control is enforced at the orchestrator level.

## Cost Control Mechanisms

### 1. Hard Cost Limits

Set a maximum spending cap in `ralph.yml`:

```yaml
event_loop:
  max_cost_usd: 10.0
```

When cumulative estimated cost exceeds this value, Ralph stops the loop. There is no default limit; if omitted, cost is unbounded.

### 2. Iteration and Runtime Limits

Cap total work by iteration count or elapsed time.

CLI:

```bash
# Strict iteration budget
ralph run --max-iterations 20
```

Config:

```yaml
event_loop:
  max_iterations: 20
  max_runtime_seconds: 3600
```

`max_iterations` defaults to `100`. `max_runtime_seconds` defaults to `14400` (4 hours).

### 3. Checkpoint Interval

Control how frequently Ralph checkpoints state:

```yaml
event_loop:
  checkpoint_interval: 5
```

The default is `5`. There is no `--checkpoint-interval` CLI flag.

### 4. Context and Token Management

Ralph does not provide global context-window or context-threshold settings. Instead, manage context through:

- **Prompt design**: keep prompts focused and reference external files rather than inlining large specs.
- **Guardrails**: use hat instructions and preset rules to prevent unbounded work.
- **Memory budget**: limit how much memory context is retained:

```yaml
memories:
  budget: 4000
```

- **Per-hat instructions**: each hat's `instructions:` define what it may read, emit, and retry, preventing runaway loops.

## Optimization Strategies

### 1. Tiered Backend Strategy

Use different backends for different task phases:

```bash
# Phase 1: Research with a cheap backend
ralph run -H opencode --max-iterations 5 --prompt research.md

# Phase 2: Implementation with a stronger backend
ralph run -H claude --max-iterations 20 --prompt implement.md

# Phase 3: Testing with a cheap backend
ralph run -H gemini --max-iterations 10 --prompt test.md
```

Alternatively, configure backends in a preset's `ralph.yml` and switch presets between phases.

### 2. Prompt Optimization

Reduce token usage through efficient prompts:

#### Before (Expensive)

```markdown
Please create a comprehensive web application with the following features:
- User authentication system with registration, login, password reset
- Dashboard with charts and graphs
- API with full CRUD operations
- Complete test suite
- Detailed documentation
[... 5000 tokens of requirements ...]
```

#### After (Optimized)

```markdown
Build user auth API:
- Register/login endpoints
- JWT tokens
- PostgreSQL storage
- Basic tests
See spec.md for details.
```

### 3. Iteration Optimization

Fewer, focused iterations save money:

```bash
# Many unbounded iterations (expensive)
ralph run --max-iterations 100   # ❌

# Tight, focused budget (economical)
ralph run --max-iterations 20    # ✅
```

Set both limits in `ralph.yml`:

```yaml
event_loop:
  max_iterations: 20
  max_runtime_seconds: 1800
```

## Cost Monitoring

### Runtime Diagnosis

Enable diagnostics to emit telemetry JSONL under `.ralph/diagnostics/`:

```bash
RALPH_DIAGNOSTICS=1 ralph run --prompt task.md
```

Or enable it persistently in `ralph.yml`:

```yaml
telemetry:
  runtime_diagnosis: true
```

Then inspect the loop:

```bash
ralph diagnose
```

This surfaces iteration counts, per-backend call patterns, and runtime events that help you understand where cost accumulates.

### Inspecting Telemetry

Diagnostics are written as JSONL files in `.ralph/diagnostics/`. Query them with standard tools:

```bash
# Total iterations recorded
jq -r 'select(.event_type == "iteration_start") | .iteration' .ralph/diagnostics/events.jsonl | wc -l

# Last few events
jq . .ralph/diagnostics/events.jsonl | tail -n 40
```

### Cost Dashboards

You can build a dashboard from `.ralph/diagnostics/*.jsonl` or backend invoices. Ralph does not ship a built-in dashboard, but the telemetry schema is stable enough to pipe into your own tooling.

Example: export a simple CSV of events:

```bash
jq -r '[.timestamp, .event_type, .hat // "-"] | @csv' .ralph/diagnostics/events.jsonl > events.csv
```

## Budget Planning

### Task Cost Estimation

| Task Type | Complexity | Recommended Budget | Backend Suggestion |
|-----------|------------|-------------------:|-------------------|
| Simple Script | Low | $0.50 - $2 | OpenCode / Gemini / Trae CLI |
| Web API | Medium | $5 - $20 | Gemini / Claude / Codex |
| Full Application | High | $20 - $100 | Claude / Codex |
| Data Analysis | Medium | $5 - $15 | Gemini / OpenCode |
| Documentation | Low-Medium | $2 - $10 | Pi / OpenCode / Claude |
| Debugging | Variable | $5 - $50 | Claude / Codex |

### Monthly Budget Planning

```python
# Calculate monthly budget needs
tasks_per_month = 50
avg_cost_per_task = 10.0
safety_margin = 1.5

monthly_budget = tasks_per_month * avg_cost_per_task * safety_margin
print(f"Recommended monthly budget: ${monthly_budget}")
```

Use `event_loop.max_cost_usd` per run as a guardrail, and track actual spend via your backend billing dashboard.

## Cost Optimization Profiles

### Minimal Cost Profile

Maximum savings, acceptable quality:

```yaml
event_loop:
  max_cost_usd: 2.0
  max_iterations: 15
  max_runtime_seconds: 900
  checkpoint_interval: 10
```

Run with:

```bash
ralph run -H opencode --prompt task.md
```

### Balanced Profile

Good quality, reasonable cost:

```yaml
event_loop:
  max_cost_usd: 10.0
  max_iterations: 30
  max_runtime_seconds: 1800
  checkpoint_interval: 5
```

Run with:

```bash
ralph run -H gemini --prompt task.md
```

### Quality Profile

Best results, controlled spending:

```yaml
event_loop:
  max_cost_usd: 50.0
  max_iterations: 50
  max_runtime_seconds: 3600
  checkpoint_interval: 3
```

Run with:

```bash
ralph run -H claude --prompt task.md
```

## Advanced Cost Management

### Dynamic Backend Switching

Switch presets or backends based on remaining budget. Example pseudo-code:

```python
if remaining_budget > 20:
    backend = "claude"
elif remaining_budget > 5:
    backend = "gemini"
else:
    backend = "opencode"
```

In Ralph, switch by running a different preset or overriding the backend with `-H <backend>`.

### Cost-Aware Prompts

Include cost considerations in prompts:

```markdown
## Budget Constraints
- Maximum budget: $10
- Optimize for efficiency
- Skip non-essential features if approaching limit
- Prioritize core functionality
```

### Batch Processing

Combine related small tasks into one run with a clear plan:

```bash
# Inefficient: multiple isolated runs
ralph run --prompt task1.md   # $5
ralph run --prompt task2.md   # $5
ralph run --prompt task3.md   # $5
# Total: $15

# Efficient: single batched run with a plan
ralph run --prompt batch_plan.md
# Total: $10 (33% savings)
```

## Cost Alerts

### Setting Up Alerts

You can monitor `.ralph/diagnostics/events.jsonl` for anomalous loop activity:

```bash
#!/bin/bash
# cost_monitor.sh

ITER_LIMIT=50
CURRENT_ITER=$(jq -r 'select(.event_type == "iteration_start") | .iteration' .ralph/diagnostics/events.jsonl | tail -n 1)

if [ "$CURRENT_ITER" -gt "$ITER_LIMIT" ]; then
    echo "ALERT: Run exceeded $ITER_LIMIT iterations" | mail -s "Ralph Cost Alert" admin@example.com
fi
```

### Automated Stops

Use `event_loop.max_cost_usd` as a built-in circuit breaker. For external automation, inspect the latest diagnostic event:

```bash
#!/bin/bash
PERCENT=90
LIMIT=50.0

# Placeholder: replace with your actual billing API or telemetry-derived cost
CURRENT_COST=45.0
THRESHOLD=$(echo "$LIMIT * $PERCENT / 100" | bc -l)

if (( $(echo "$CURRENT_COST > $THRESHOLD" | bc -l) )); then
    echo "WARNING: ${PERCENT}% of budget consumed"
    exit 1
fi
```

Ralph's own `max_cost_usd` enforces the hard stop so you do not need a custom breaker for runtime safety.

## ROI Analysis

### Calculating ROI

```python
# ROI calculation
hours_saved = 10  # Hours of manual work saved
hourly_rate = 50  # Developer hourly rate
ai_cost = 25  # Cost of AI orchestration

value_created = hours_saved * hourly_rate
roi = (value_created - ai_cost) / ai_cost * 100

print(f"Value created: ${value_created}")
print(f"AI cost: ${ai_cost}")
print(f"ROI: {roi:.1f}%")
```

### Cost-Benefit Matrix

| Task | Manual Hours | Manual Cost | AI Cost | Savings |
|------|-------------:|------------:|--------:|--------:|
| API Development | 40h | $2000 | $50 | $1950 |
| Documentation | 20h | $1000 | $20 | $980 |
| Testing Suite | 30h | $1500 | $30 | $1470 |
| Bug Fixing | 10h | $500 | $25 | $475 |

## Best Practices

### 1. Start Small

Test with minimal budgets first:

```bash
# Test run
ralph run --max-iterations 5 --prompt task.md

# Scale up if successful
ralph run --max-iterations 25 --prompt task.md
```

Set a low `max_cost_usd` for exploratory work:

```yaml
event_loop:
  max_cost_usd: 1.0
  max_iterations: 5
```

### 2. Monitor Continuously

Track runtime behavior:

```bash
# Run with diagnostics enabled
RALPH_DIAGNOSTICS=1 ralph run --prompt task.md

# Inspect the latest events
ralph diagnose
jq . .ralph/diagnostics/events.jsonl | tail -n 20
```

### 3. Optimize Iteratively

- Analyze diagnostic events
- Identify expensive operations
- Refine prompts and hat instructions
- Test optimizations with a small `max_iterations`

### 4. Set Realistic Budgets

- Development: 50% of production budget
- Testing: 25% of production budget
- Production: full budget with safety margin

### 5. Document Costs

Keep records for analysis:

```bash
# Save diagnostic snapshot after each run
ralph diagnose && \
  cp .ralph/diagnostics/events.jsonl "reports/run_$(date +%Y%m%d_%H%M%S).jsonl"
```

## Troubleshooting

### Common Issues

1. **Unexpected high costs**
   - Check iteration count in diagnostics
   - Review prompt efficiency
   - Verify hat instructions are not over-calling backends

2. **Budget exceeded quickly**
   - Lower `max_iterations`
   - Shorten `max_runtime_seconds`
   - Use a cheaper backend

3. **Poor results with budget constraints**
   - Increase `max_cost_usd` slightly
   - Optimize prompts
   - Consider a phased approach

## Next Steps

- Review [Backend Selection](backends.md) for cost-effective choices
- Optimize [Prompts](prompts.md) for efficiency
- Explore [Examples](../examples/index.md) for cost-optimized patterns
