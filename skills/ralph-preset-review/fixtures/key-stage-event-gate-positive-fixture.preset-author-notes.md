# Key-stage event gate positive fixture notes

```yaml
key_stages:
  - key_stage: executor -> reporter handoff
    guard_selection: precheck
    precheck_guard: true
    precheck_retry_budget: 3
    payload_consistency_guard: false
    payload_consistency_retry_budget: null
    reason: "主观验收需要在 work.done 下游前独立复核。"
    confirmation_status: confirmed
  - key_stage: reporter terminal
    guard_selection: payload_consistency
    precheck_guard: false
    precheck_retry_budget: null
    payload_consistency_guard: true
    payload_consistency_retry_budget: 3
    reason: "终态字段之间需要确定性一致性检查。"
    confirmation_status: confirmed
```
