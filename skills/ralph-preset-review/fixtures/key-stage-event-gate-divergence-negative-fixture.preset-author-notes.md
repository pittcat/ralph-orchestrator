# Key-stage event gate divergence fixture notes

```yaml
key_stages:
  - key_stage: executor -> reporter handoff
    guard_selection: precheck
    precheck_guard: true
    precheck_retry_budget: 3
    payload_consistency_guard: false
    payload_consistency_retry_budget: null
    reason: "handoff 需要在下游激活前通过主观验收。"
    confirmation_status: confirmed
  - key_stage: reporter terminal
    guard_selection: both
    precheck_guard: true
    precheck_retry_budget: 3
    payload_consistency_guard: true
    payload_consistency_retry_budget: 3
    reason: "终态同时需要质量和字段一致性保护。"
    confirmation_status: confirmed
  - key_stage: optional handoff
    guard_selection: neither
    precheck_guard: false
    precheck_retry_budget: null
    payload_consistency_guard: false
    payload_consistency_retry_budget: null
    reason: "用户偏好"
    confirmation_status: confirmed
```
