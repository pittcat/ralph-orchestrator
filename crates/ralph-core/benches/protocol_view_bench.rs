//! U3 (KTD-8) `ProtocolView` 构造开销基准测试。
//!
//! 目标:验证 `from_event_loop_with_index*` 在 `ce-executor-serial`
//! BDD fixture 上的构造耗时,确保 per-batch 生成的开销 < 5% 基线
//! (plan 2026-06-21-002 U3 §Performance 验收)。
//!
//! 不引入 criterion 依赖(项目当前 Cargo.toml 只有 `harness = false`
//! 的手写 bench)。输出 `ns/op` (mean + p50/p95/p99) 到 stdout,
//! 解析方式见 `scripts/parse-bench-output.sh` 与 docs 配套脚本。
//!
//! 用法:
//!   cargo bench --bench protocol_view_bench
//!   cargo run --release -p ralph-core --bin protocol_view_bench  # 不通过 bench harness
//!
//! 环境:UNIFIED_PROTOCOL_VIEW=1 用于对比 feature-on 的开销。

use ralph_core::config::RalphConfig;
use ralph_core::preset::engine::ProtocolView;
use ralph_core::workflow_contract::handoff_index::HandoffIndex;
use std::hint::black_box;
use std::time::Instant;

const ITERATIONS: usize = 2_000;
const WARMUPS: usize = 500;

/// `ce-executor-serial` BDD fixture YAML(最小可用片段)。
/// 与 `crates/ralph-core/tests/scenarios/` 中的 serial preset
/// 拓扑一致:plan_gate → executor → reviewer。
const SERIAL_FIXTURE_YAML: &str = r#"
prompt_file: PROMPT.md
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["work.ready", "queue.advance"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
  reviewer:
    name: "Reviewer"
    triggers: ["work.done"]
    publishes: ["review.dimension.ready"]
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
  starting_event: "work.start"
  event_policy:
    enabled: true
    mode: observe
    schemas:
      work.ready:
        required_fields: ["plan_name"]
      work.done:
        required_fields: ["plan_name", "step"]
"#;

/// 构造并返回 (cfg, index) fixture。失败时 panic。
fn serial_fixture() -> (RalphConfig, HandoffIndex) {
    let cfg: RalphConfig = serde_yaml::from_str(SERIAL_FIXTURE_YAML)
        .expect("SERIAL_FIXTURE_YAML must parse as RalphConfig");
    let index = HandoffIndex::from_config(&cfg);
    (cfg, index)
}

fn main() {
    let (cfg, index) = serial_fixture();

    println!(">>> ProtocolView construction benchmark (KTD-8 / U3)");
    println!("    iterations: {}, warmups: {}", ITERATIONS, WARMUPS);
    println!("    fixture:    ce-executor-serial (3 hats, isolated)");

    // Both paths are constructed via the explicit
    // `from_event_loop_with_index_and_feature` constructor so
    // the only difference between `legacy` and `feature_on` is
    // the bool passed in. This isolates the cost of the
    // `feature_flag_enabled` field set + default branch from
    // any incidental `std::env` cost.
    //
    // (The `from_event_loop_with_index` shim reads the env once
    // and forwards to `from_event_loop_with_index_and_feature`,
    // so the env cost is one-shot at startup, not per-call.
    // The bench therefore measures steady-state cost, which is
    // what the runtime hot path actually pays.)

    let mut samples_legacy: Vec<u128> = Vec::with_capacity(ITERATIONS);
    let mut samples_feature: Vec<u128> = Vec::with_capacity(ITERATIONS);

    // Warmup both paths.
    for _ in 0..WARMUPS {
        let _ = ProtocolView::from_event_loop_with_index_and_feature(
            &cfg.event_loop,
            Some(&index),
            false,
        );
        let _ = ProtocolView::from_event_loop_with_index_and_feature(
            &cfg.event_loop,
            Some(&index),
            true,
        );
    }

    let start = Instant::now();
    for i in 0..ITERATIONS {
        let is_legacy = i % 2 == 0;
        let t0 = Instant::now();
        let view = if is_legacy {
            let v = ProtocolView::from_event_loop_with_index_and_feature(
                &cfg.event_loop,
                Some(&index),
                false,
            );
            samples_legacy.push(t0.elapsed().as_nanos());
            v
        } else {
            let v = ProtocolView::from_event_loop_with_index_and_feature(
                &cfg.event_loop,
                Some(&index),
                true,
            );
            samples_feature.push(t0.elapsed().as_nanos());
            v
        };
        black_box(view);
    }
    let total = start.elapsed();

    fn summarise(label: &str, samples: &mut Vec<u128>) -> f64 {
        samples.sort_unstable();
        let n = samples.len();
        let p50 = samples[n / 2];
        let p95 = samples[(n * 95) / 100];
        let p99 = samples[(n * 99) / 100];
        let max = samples[n - 1];
        let min = samples[0];
        let sum: u128 = samples.iter().sum();
        let mean = sum as f64 / n as f64;
        println!("\n=== {label} ===");
        println!("samples:   {}", n);
        println!("ns/op mean: {mean:.2}");
        println!("ns/op min:  {min}");
        println!("ns/op p50:  {p50}");
        println!("ns/op p95:  {p95}");
        println!("ns/op p99:  {p99}");
        println!("ns/op max:  {max}");
        println!("====================\n");
        mean
    }

    let mean_legacy = summarise("protocol_view_construction_legacy", &mut samples_legacy);
    let mean_feature = summarise(
        "protocol_view_construction_feature_on",
        &mut samples_feature,
    );

    let delta_pct = ((mean_feature - mean_legacy) / mean_legacy) * 100.0;
    println!(">>> Summary");
    println!("    iterations:  {} (interleaved)", ITERATIONS);
    println!("    total:       {:?}", total);
    println!("    legacy   mean: {mean_legacy:.2} ns/op");
    println!("    feature  mean: {mean_feature:.2} ns/op");
    println!("    delta:        {delta_pct:+.2}%");
    println!("    target:       < 5% per-batch overhead (plan 2026-06-21-002 U3 §Performance)");
    if delta_pct.abs() < 5.0 {
        println!("    STATUS:       PASS (delta within 5% baseline)");
    } else {
        println!("    STATUS:       WARN (delta exceeds 5% threshold; review implementation)");
    }
}
