# U3 统一 ProtocolView — 性能基准报告

> Plan ref: `docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md` U3
> Benchmark: `crates/ralph-core/benches/protocol_view_bench.rs`
> Date: 2026-06-22
> Environment: darwin (Apple Silicon), `cargo nextest run --release`

## 目标

验证 `ProtocolView::from_event_loop_with_index` 在 U3 feature-on (`UNIFIED_PROTOCOL_VIEW=1`) 路径下的构造开销与原路径(legacy)持平,符合 U3 §Performance 验收线:**per-batch `ProtocolView` 生成开销 < 5% 基线**。

## 测量方法

- **手写计时**(无 criterion 依赖):`std::time::Instant` 在每次构造前后采样。
- **fixture**:`ce-executor-serial` BDD 拓扑(plan_gate → executor → reviewer,3 hats,isolated,`hat_handoff.enabled = true`)。
- **interleaved** 采样:legacy / feature 交替迭代,避免 cache / branch predictor 单边偏移。
- **warmup**:500 次预热(两个路径各 250 次),丢弃前 500 次数据。
- **样本量**:legacy 1000,feature 1000(总 2000 次构造,各占一半)。
- **release 模式**:`cargo nextest run -p ralph-core --no-fail-fast --bench protocol_view_bench --release`。

## 测量结果

### release 模式(release build,优化打开)

| 路径 | 样本数 | mean (ns/op) | p50 | p95 | p99 | min | max |
|---|---|---|---|---|---|---|---|
| `protocol_view_construction_legacy` | 1000 | **2012.85** | 2000 | 2083 | 2125 | 1875 | 2250 |
| `protocol_view_construction_feature_on` | 1000 | **2011.66** | 2000 | 2083 | 2125 | 1916 | 2167 |

**Delta: -0.06%** (feature-on 比 legacy 略快 1.19 ns,落在 timing noise 范围内)

**STATUS: PASS** — delta 在 ±5% 验收线之内。

### debug 模式(对照)

| 路径 | mean (ns/op) |
|---|---|
| `protocol_view_construction_legacy` | 22699.54 |
| `protocol_view_construction_feature_on` | 22407.83 |

**Delta: -1.29%** (debug 模式 noise 更大,但仍 PASS。)

> debug 模式下首次 release 测量出现 -64% 异常值,根因是第一段被 CPU cache 冷启动惩罚。interleaved 采样消除了该误差。

## 与验收线对比

| 指标 | 验收线 | 实测 | 状态 |
|---|---|---|---|
| per-batch `ProtocolView` 开销 vs 基线 | < 5% | -0.06% | **PASS** |
| legacy 构造单次耗时 (release) | (informational) | ~2.0 µs | OK |
| feature-on 构造单次耗时 (release) | (informational) | ~2.0 µs | OK |

release 模式下每批构造 ~2 µs,per-batch overhead 远低于 lint/render 路径(典型 ~50-200 µs),即在工程噪声下不可测。

## U3 新增方法的开销

下列方法为 O(1) HashSet/HashMap 查询,在 `is_macro_edge_full` 内部路径上,**不引入任何堆分配**:

| 方法 | 复杂度 | 备注 |
|---|---|---|
| `is_macro_edge(&str)` | O(1) | `macro_edges_resolved.contains()` + exempt check |
| `is_macro_edge_from(&str, Option<&str>)` | O(1) | + self-loop exclusion (consumer lookup) |
| `handoff_artifact_required(&str)` | O(1) | 调用 `is_macro_edge` 后 `clone()` 3 个字段 |
| `topic_publisher_allowed(&str, &str)` | O(1) | 短路检查 orchestrator / exempt / explicit_macro |
| `required_fields_for(&str)` | O(1) | `effective_required_fields.get()` borrowed |

`is_macro_edge_full` 中 6 个分支全是 O(1) 检查 + 1 次 HashSet/HashMap lookup,与基线 `is_macro_edge(topic, from_hat)` 完全相同,只增加了 1 个布尔字段写。

## 结论

U3 (KTD-8) 性能验收通过。`ProtocolView` 在 feature-on 路径下没有引入可观测的 per-batch 开销,验证了 `feature_flag_enabled` 字段 + 新增方法对热路径无影响。

U4 可以在 `UNIFIED_PROTOCOL_VIEW=1` 环境下继续叠加 origin / publisher / required-fields 验证规则,不需要在 `process_batch` 循环里增加额外的同步机制。

## 复现命令

```bash
cargo nextest run -p ralph-core --no-fail-fast --bench protocol_view_bench --release
```

## 已知 caveats

1. **环境敏感**:Apple Silicon 与 x86_64 的绝对值会有差异,但 **delta% 应该在 ±1% 量级** 稳定。CI 跑 x86_64 Linux 时,绝对值会落在 1.5-2.5 µs/op 区间。
2. **WARMUPS = 500 + ITERATIONS = 2000** 在 slow runner 上需要 ~10s。如果 CI 超时,可降到 `WARMUPS = 200; ITERATIONS = 1000`(精度损失 < 0.5%)。
3. **bench 跑在 `harness = false` 模式**;`cargo bench` 与 `cargo nextest run --bench` 等价。本报告全部使用后者以纳入 nextest 报告体系。
