# Key-stage event gate no-reason fixture notes

```yaml
key_stages:
  - key_stage: executor -> reporter handoff
    guard_selection: neither
    precheck_guard: false
    precheck_retry_budget: null
    payload_consistency_guard: false
    payload_consistency_retry_budget: null
    reason: ""
    confirmation_status: confirmed
  - key_stage: reporter terminal
    guard_selection: precheck
    precheck_guard: true
    precheck_retry_budget: 1
    payload_consistency_guard: false
    payload_consistency_retry_budget: null
    reason: ""
    confirmation_status: confirmed
```
