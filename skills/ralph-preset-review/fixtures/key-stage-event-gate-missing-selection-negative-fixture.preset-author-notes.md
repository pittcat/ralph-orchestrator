# Key-stage event gate missing-selection fixture notes

```yaml
key_stages:
  - key_stage: executor -> reporter handoff
    precheck_guard: true
    precheck_retry_budget: 3
    payload_consistency_guard: false
    payload_consistency_retry_budget: null
    reason: "需要复核 handoff 的主观验收。"
    confirmation_status: pending
```
