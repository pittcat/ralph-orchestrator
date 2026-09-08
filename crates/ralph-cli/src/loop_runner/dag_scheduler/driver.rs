//! 2026-09-03-0959 plan U6 — the `EventLoop` → pipeline driver.
//!
//! The runtime's `EventLoop` calls `DagSchedulerDriver::observe_accepted`
//! when a worker-emitted event passes the existing acceptance
//! gate. The driver routes the event into the right pipeline
//! slot:
//!   - `forge.unit.executed` → `JobPipeline::advance(unit, Review)`
//!   - `forge.unit.reviewed` (ACCEPTED) → `JobPipeline::advance(unit, Verify)`
//!   - `forge.unit.reviewed` (REJECTED) →
//!     `JobPipeline::bump_attempt_and_advance(unit, Review)`
//!   - any other topic → ignored (driver is observation-only)
//!
//! The driver consumes the `forge.unit.*` per-unit typed topic
//! family. The retired wave control path is not part of active DAG
//! scheduling. `Stage`
//! 枚举只有 Execute/Review/Verify——verified 之后的 integration
//! 推进(`forge.unit.integrated` 由 runtime 在 lane CAS FF 后发射)
//! 属后续 Step,driver 暂不消费。
//!
//! On `Block`, the driver returns the typed reason so the caller
//! can publish `forge.plan.blocked` (or `forge.final.correction.settled`,
//! per U4 / U5 wiring).
//!
//! The driver does NOT spawn subprocesses. It only routes
//! events. Subprocesses are launched by the runtime's job
//! kernel (`runtime_job::worker`), which the runtime invokes
//! after the driver returns `Admitted`.

use serde_json::Value;

use super::jobs::{AdvanceOutcome, JobPipeline};
use crate::loop_runner::runtime_job::JobToken;
use crate::loop_runner::runtime_job::{RuntimeJobError, Stage};

/// Topics the driver recognises. The list is intentionally
/// narrow — anything outside it is a no-op so the driver never
/// silently corrupts a pipeline slot.
///
/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见;EventLoop
/// acceptance 路径接线前无 bin 侧消费方,item 级
/// `#[allow(dead_code)]`——promote 义务见
/// `presets/en/parallel-forge-preset-author-notes.md`「promote 前置
/// 义务清单」#1/#2,接线落地后移除。
#[allow(dead_code)]
pub mod topics {
    pub const UNIT_EXECUTED: &str = "forge.unit.executed";
    pub const UNIT_REVIEWED: &str = "forge.unit.reviewed";
}

/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见;verdict
/// 值域与 preset 词汇对齐为 `ACCEPTED` / `REJECTED`。无 bin 侧生产
/// 调用方,item 级 `#[allow(dead_code)]`(同 `topics` 的注释)。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewVerdict {
    Accepted,
    Rejected,
}

#[allow(dead_code)] // 同 `topics` 的 Step 1+2 promote 注释。
impl ReviewVerdict {
    /// Parse from the event payload's `verdict` field. Returns
    /// `None` if the field is missing or unrecognised — the
    /// driver treats that as a no-op so a malformed event does
    /// not advance state.
    pub fn from_payload(payload: &Value) -> Option<Self> {
        let s = payload.get("verdict")?.as_str()?;
        match s {
            "ACCEPTED" => Some(Self::Accepted),
            "REJECTED" => Some(Self::Rejected),
            _ => None,
        }
    }
}

/// Outcome of a single `observe_accepted` call.
///
/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见;无 bin 侧
/// 生产调用方,item 级 `#[allow(dead_code)]`(同 `topics` 的注释)。
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverOutcome {
    /// Driver routed the event into the pipeline; the caller
    /// can proceed with the (token, next_stage) pair.
    Routed {
        unit_key: String,
        next_stage: Stage,
        token: JobToken,
    },
    /// Event did not match any driver topic — the runtime
    /// should leave the pipeline alone.
    Ignored { topic: String },
    /// Driver routed but the pipeline rejected (pool exhausted,
    /// global cap, illegal transition, fix budget exhausted).
    Blocked {
        unit_key: String,
        error: RuntimeJobError,
    },
    /// Driver routed but the kernel's collect returned
    /// `CollectFailed`; pipeline state is unchanged.
    StillExecuting { unit_key: String, stage: Stage },
}

/// The driver is a thin handle over a `JobPipeline` so the
/// `EventLoop` can hand accepted events to it without owning
/// the pipeline's mutable state.
///
/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见;live
/// runtime 驱动 driver 前无 bin 侧生产调用方,item 级
/// `#[allow(dead_code)]`(同 `topics` 的注释)。
#[allow(dead_code)]
pub struct DagSchedulerDriver<'a> {
    pipeline: &'a mut JobPipeline,
}

#[allow(dead_code)] // 同 `topics` 的 Step 1+2 promote 注释。
impl<'a> DagSchedulerDriver<'a> {
    pub fn new(pipeline: &'a mut JobPipeline) -> Self {
        Self { pipeline }
    }

    /// Route one accepted event into the pipeline.
    pub fn observe_accepted(
        &mut self,
        topic: &str,
        unit_key: &str,
        payload: &Value,
    ) -> DriverOutcome {
        match topic {
            topics::UNIT_EXECUTED => {
                // Executor finished — advance to Review.
                match self.pipeline.advance(unit_key, Stage::Review) {
                    AdvanceOutcome::Admitted { token } => DriverOutcome::Routed {
                        unit_key: unit_key.to_string(),
                        next_stage: Stage::Review,
                        token,
                    },
                    AdvanceOutcome::StillExecuting { unit_key, stage } => {
                        DriverOutcome::StillExecuting { unit_key, stage }
                    }
                    AdvanceOutcome::Blocked(error) => DriverOutcome::Blocked {
                        unit_key: unit_key.to_string(),
                        error,
                    },
                }
            }
            topics::UNIT_REVIEWED => match ReviewVerdict::from_payload(payload) {
                Some(ReviewVerdict::Accepted) => {
                    match self.pipeline.advance(unit_key, Stage::Verify) {
                        AdvanceOutcome::Admitted { token } => DriverOutcome::Routed {
                            unit_key: unit_key.to_string(),
                            next_stage: Stage::Verify,
                            token,
                        },
                        AdvanceOutcome::StillExecuting { unit_key, stage } => {
                            DriverOutcome::StillExecuting { unit_key, stage }
                        }
                        AdvanceOutcome::Blocked(error) => DriverOutcome::Blocked {
                            unit_key: unit_key.to_string(),
                            error,
                        },
                    }
                }
                Some(ReviewVerdict::Rejected) => {
                    match self
                        .pipeline
                        .bump_attempt_and_advance(unit_key, Stage::Review)
                    {
                        AdvanceOutcome::Admitted { token } => DriverOutcome::Routed {
                            unit_key: unit_key.to_string(),
                            next_stage: Stage::Review,
                            token,
                        },
                        AdvanceOutcome::StillExecuting { unit_key, stage } => {
                            DriverOutcome::StillExecuting { unit_key, stage }
                        }
                        AdvanceOutcome::Blocked(error) => DriverOutcome::Blocked {
                            unit_key: unit_key.to_string(),
                            error,
                        },
                    }
                }
                None => DriverOutcome::Ignored {
                    topic: topic.to_string(),
                },
            },
            _ => DriverOutcome::Ignored {
                topic: topic.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_runner::dag_scheduler::jobs::DagPools;
    use serde_json::json;

    fn driver_fixture() -> (DagPools, JobPipeline) {
        let pools = DagPools::new(4, 2, 2, 2);
        let pipeline = JobPipeline::new(pools.clone());
        (pools, pipeline)
    }

    /// Exec-complete routes to Review.
    #[test]
    fn exec_complete_routes_to_review() {
        let (_pools, mut pipeline) = driver_fixture();
        pipeline.ensure_unit("U-1", "j-1", "executor", Stage::Execute);
        let _ = pipeline.advance("U-1", Stage::Execute);
        pipeline.release("U-1");
        let mut driver = DagSchedulerDriver::new(&mut pipeline);
        let out = driver.observe_accepted(topics::UNIT_EXECUTED, "U-1", &json!({}));
        match out {
            DriverOutcome::Routed {
                unit_key,
                next_stage,
                ..
            } => {
                assert_eq!(unit_key, "U-1");
                assert_eq!(next_stage, Stage::Review);
            }
            other => panic!("expected Routed, got {other:?}"),
        }
    }

    /// Review-Accepted routes to Verify.
    #[test]
    fn review_accepted_routes_to_verify() {
        let (_pools, mut pipeline) = driver_fixture();
        pipeline.ensure_unit("U-2", "j-2", "executor", Stage::Execute);
        let _ = pipeline.advance("U-2", Stage::Execute);
        pipeline.release("U-2");
        let _ = pipeline.advance("U-2", Stage::Review);
        let mut driver = DagSchedulerDriver::new(&mut pipeline);
        let out = driver.observe_accepted(
            topics::UNIT_REVIEWED,
            "U-2",
            &json!({"verdict": "ACCEPTED"}),
        );
        match out {
            DriverOutcome::Routed { next_stage, .. } => {
                assert_eq!(next_stage, Stage::Verify);
            }
            other => panic!("expected Routed, got {other:?}"),
        }
    }

    /// Review-Rejected bumps attempt and re-routes to
    /// Review with a fresh token.
    #[test]
    fn review_rejected_bumps_attempt() {
        let (_pools, mut pipeline) = driver_fixture();
        pipeline.ensure_unit("U-3", "j-3", "executor", Stage::Execute);
        let _ = pipeline.advance("U-3", Stage::Execute);
        pipeline.release("U-3");
        let _ = pipeline.advance("U-3", Stage::Review);
        pipeline.release("U-3");
        let mut driver = DagSchedulerDriver::new(&mut pipeline);
        let out = driver.observe_accepted(
            topics::UNIT_REVIEWED,
            "U-3",
            &json!({"verdict": "REJECTED"}),
        );
        match out {
            DriverOutcome::Routed { token, .. } => {
                assert_eq!(token.attempt(), 1);
            }
            other => panic!("expected Routed, got {other:?}"),
        }
    }

    /// Unrecognised topics are no-ops.
    #[test]
    fn unrecognised_topic_is_ignored() {
        let (_pools, mut pipeline) = driver_fixture();
        let mut driver = DagSchedulerDriver::new(&mut pipeline);
        let out = driver.observe_accepted("ralph.unknown.topic", "U-x", &json!({}));
        assert!(matches!(out, DriverOutcome::Ignored { .. }));
    }

    /// P1-4 (verdict payload shape): the REAL ingress output
    /// feeds `ReviewVerdict::from_payload` end-to-end. The
    /// ingress unwraps the worker payload's `verdict` string so
    /// the accepted event is FLAT (`payload.verdict ==
    /// "ACCEPTED"`); the driver must route it to Verify instead
    /// of returning `Ignored` (which would silently drop the
    /// review event and wedge the pipeline in Review forever).
    #[test]
    fn ingress_review_payload_routes_through_driver() {
        use crate::loop_runner::runtime_job::result_ingress::submit_accepted_result;
        use crate::loop_runner::runtime_job::{JobDescriptor, ProcessResult};

        let (_pools, mut pipeline) = driver_fixture();
        pipeline.ensure_unit("U-ing", "j-ing", "executor", Stage::Execute);
        let _ = pipeline.advance("U-ing", Stage::Execute);
        pipeline.release("U-ing");
        let _ = pipeline.advance("U-ing", Stage::Review);

        // Real ingress output: the worker payload nests the
        // verdict; the receipt must carry it unwrapped.
        let descriptor = JobDescriptor::new("U-ing", "j-ing", "executor", Stage::Review);
        let result = ProcessResult::new(
            json!({"verdict": "ACCEPTED", "notes": "ok"}),
            Some(0),
            42,
            7,
        );
        let receipt = submit_accepted_result(&descriptor, &result).expect("ingress accepts");

        let mut driver = DagSchedulerDriver::new(&mut pipeline);
        let out = driver.observe_accepted(topics::UNIT_REVIEWED, "U-ing", receipt.payload());
        match out {
            DriverOutcome::Routed { next_stage, .. } => {
                assert_eq!(next_stage, Stage::Verify);
            }
            other => panic!(
                "expected Routed(Verify) — an ingress/driver shape \
                 mismatch silently drops the review event, got {other:?}"
            ),
        }
    }

    /// S4 (refill): once an executor reaches a durable terminal
    /// and its slot is released, a Ready unit queued behind the
    /// global cap is admitted in the SAME scheduling tick — no
    /// wave settlement in between.
    #[test]
    fn s4_ready_unit_refills_slot_after_terminal_same_tick() {
        // global=1 / executor=1: exactly one Unit may be in
        // flight; the second one queues.
        let pools = DagPools::new(1, 1, 2, 2);
        let mut pipeline = JobPipeline::new(pools);
        pipeline.ensure_unit("U-done", "j-done", "executor", Stage::Execute);
        pipeline.ensure_unit("U-ready", "j-ready", "executor", Stage::Execute);

        let first = pipeline.advance("U-done", Stage::Execute);
        assert!(matches!(first, AdvanceOutcome::Admitted { .. }));
        let queued = pipeline.advance("U-ready", Stage::Execute);
        assert!(
            matches!(
                queued,
                AdvanceOutcome::Blocked(RuntimeJobError::GlobalCapExceeded { .. })
            ),
            "U-ready must queue behind the global cap, got {queued:?}"
        );

        // Same tick: U-done runs to its durable terminal through
        // the driver (exec-complete → Review, ACCEPTED → Verify).
        {
            let mut driver = DagSchedulerDriver::new(&mut pipeline);
            let out = driver.observe_accepted(topics::UNIT_EXECUTED, "U-done", &json!({}));
            assert!(
                matches!(
                    out,
                    DriverOutcome::Routed {
                        next_stage: Stage::Review,
                        ..
                    }
                ),
                "exec-complete must route U-done to Review, got {out:?}"
            );
            let out = driver.observe_accepted(
                topics::UNIT_REVIEWED,
                "U-done",
                &json!({"verdict": "ACCEPTED"}),
            );
            assert!(
                matches!(
                    out,
                    DriverOutcome::Routed {
                        next_stage: Stage::Verify,
                        ..
                    }
                ),
                "ACCEPTED must route U-done to Verify, got {out:?}"
            );
            // No premature refill while U-done still holds its
            // (migrated) slot.
        }
        let still_queued = pipeline.advance("U-ready", Stage::Execute);
        assert!(
            matches!(still_queued, AdvanceOutcome::Blocked(_)),
            "U-ready must NOT refill while U-done is still in flight, got {still_queued:?}"
        );

        // Durable terminal reached: the terminal collect releases
        // the slot. The queued Ready unit refills it immediately
        // — same tick, no wave settlement.
        pipeline.release("U-done");
        let refill = pipeline.advance("U-ready", Stage::Execute);
        assert!(
            matches!(refill, AdvanceOutcome::Admitted { .. }),
            "U-ready must refill the freed slot in the same tick, got {refill:?}"
        );
    }

    // =====================================================================
    // TG-S06 (P2 / interface-topic, PMI-002 §4 + PMI-003) — 已按 pin 内建
    // 指引翻转(2026-09-03-0959 plan DAG 接线 Step 1+2):topic 映射决策
    // 已做(方案 (b):dag 模式新增 `forge.unit.*` per-unit typed topic 族,
    // wave 路径的 `exec.unit.done` / `forge.wave.reviewed` 不动),verdict
    // 值域统一为 ACCEPTED/REJECTED。原「真实 topic 喂 driver 必须
    // Ignored」pin 随映射决策落地删除;保留的静态 pin 翻转为正包含:
    // driver 消费的 topic 必须全部声明在 schema 的 `forge.unit.*` 族内,
    // 且该族必须恰好是本测试钉住的 6 个成员(族增删 → 本测试红,强制
    // 同步 driver / schema / preset publishes / BDD)。PMI-003 的两个
    // 平行定义检查(compute_resource_aware_digest 零生产消费、PHASE_*
    // 无 as u8 绑定)不属于 topic 映射,保持原样。
    // =====================================================================

    /// TG-S06: driver topic 集合与 preset schema 声明的
    /// `forge.unit.*` per-unit topic 族正包含 + 族成员精确钉住。附带
    /// PMI-003 的两个平行定义检查: `compute_resource_aware_digest` 零
    /// 生产消费、`PHASE_*` 无 `WaveDeliveryState` 枚举绑定断言。
    #[test]
    fn tg_s06_driver_topics_match_schema_unit_family_and_parallel_defs() {
        // ── 1. driver topic 集合 ⊆ schema 的 forge.unit.* 族,且族成员
        //    精确等于钉住的 6 个 topic ──
        // schema 文件里 `schemas:` 块下的顶层 topic 键(两空格缩进、
        // 形如 `xxx.yyy:` 的行)。
        let schema_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("presets/schemas/parallel-forge.yml");
        let schema_body = std::fs::read_to_string(&schema_path).unwrap_or_else(|e| {
            panic!(
                "TG-S06: 无法读取 preset schema {}: {e}\
                 (从 repo root 跑 cargo nextest)",
                schema_path.display()
            )
        });
        let mut schema_topics: Vec<&str> = Vec::new();
        for line in schema_body.lines() {
            let trimmed_end = line.trim_end();
            if let Some(rest) = line.strip_prefix("  ")
                && !rest.starts_with(' ')
                && !rest.starts_with('#')
                && trimmed_end.ends_with(':')
                && let Some(topic) = rest.strip_suffix(':')
                && topic.contains('.')
            {
                schema_topics.push(topic);
            }
        }
        assert!(
            !schema_topics.is_empty(),
            "TG-S06 前置失效: preset schema topic 提取为空——schema 文件\
             结构可能已改,需同步本测试的解析规则"
        );
        // DAG runtime 的控制入口也必须保留在 schema 中：审批允许并发
        // admission，correction.requested 驱动修复重入。
        assert!(schema_topics.contains(&"forge.concurrency.approved"));
        assert!(schema_topics.contains(&"forge.correction.requested"));

        // 族成员精确钉住: 6 个 per-unit topic(方案 (b))。族增删 →
        // 本断言红,强制同步 driver / preset publishes / BDD。
        let mut schema_unit_topics: Vec<&str> = schema_topics
            .iter()
            .copied()
            .filter(|t| t.starts_with("forge.unit."))
            .collect();
        schema_unit_topics.sort_unstable();
        assert_eq!(
            schema_unit_topics,
            vec![
                "forge.unit.executed",
                "forge.unit.execution_failed",
                "forge.unit.integrated",
                "forge.unit.reviewed",
                "forge.unit.verification_failed",
                "forge.unit.verified",
            ],
            "TG-S06: schema 的 forge.unit.* 族成员漂移——per-unit topic 的\
             增删必须同批同步 driver / preset publishes / BDD(方案 (b) \
             topic 族是 DAG 接线的单一事实源)"
        );

        // 正包含: driver 消费的每个 topic 都已声明在 schema 族内。
        let driver_topics = [topics::UNIT_EXECUTED, topics::UNIT_REVIEWED];
        for dt in driver_topics {
            assert!(
                schema_unit_topics.contains(&dt),
                "TG-S06: driver topic `{dt}` 未声明在 schema 的 \
                 forge.unit.* 族中——driver 与 schema 失配,accepted \
                 事件会被 emit_schema_gate 拒绝或静默无消费者"
            );
        }

        // ── 2. PMI-003①: compute_resource_aware_digest 零生产消费 ──
        // 生产链路(emit 桥 canonicalize_plan_ready_payload + projector
        // load_plan_handoff)用的是 artifact_canonicalizer::canonicalize;
        // 资源感知摘要只在 parallel_forge_handoff.rs 自身的单测内自用。
        // 断言: 整个 ralph-core src 树中,该函数名除定义文件自身外
        // 零命中(注释引用算命中——若接线,需同步更新权威链)。
        let core_src_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("crates/ralph-core/src");
        let mut digest_consumers: Vec<String> = Vec::new();
        visit_rs_files(&core_src_root, &mut |path, body| {
            if path.ends_with("parallel_forge_handoff.rs") {
                return; // 定义文件自身豁免
            }
            if body.contains("compute_resource_aware_digest") {
                digest_consumers.push(path);
            }
        });
        assert!(
            digest_consumers.is_empty(),
            "TG-S06/PMI-003: `compute_resource_aware_digest` 出现生产消费\
             ({digest_consumers:?})——资源感知摘要被接线。此时必须显式\
             决策它与 artifact_canonicalizer canonical digest 的权威关系\
             (两套 digest 语义不同: canonical YAML 序列化 vs \
             |-分隔文本),不得并存双权威"
        );

        // ── 3. PMI-003②: PHASE_* 常量无 WaveDeliveryState 枚举绑定 ──
        // recovery.rs 的 PHASE_PENDING..PHASE_COORDINATION_COMMITTED
        // (0..4) 是 WaveDeliveryState 枚举声明序的手写数值镜像,无
        // 编译期绑定(没有任何 `WaveDeliveryState::X as u8` 对照或
        // 常量断言测试)。断言: recovery.rs 中不存在 `as u8` 形式的
        // 枚举-常量绑定——一旦有人补绑定或接线,本断言变红,提示
        // 同步更新枚举声明序契约。
        let recovery_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/loop_runner/dag_scheduler/recovery.rs");
        let recovery_body = std::fs::read_to_string(&recovery_path)
            .unwrap_or_else(|e| panic!("TG-S06: 无法读取 {}: {e}", recovery_path.display()));
        assert!(
            !recovery_body.contains("WaveDeliveryState::") || !recovery_body.contains(" as u8"),
            "TG-S06/PMI-003: recovery.rs 出现 WaveDeliveryState 枚举绑定\
             (`as u8` 形式)——PHASE_* 常量被接线。此时必须补枚举声明序\
             的编译期绑定(WaveDeliveryState::X as u8 对照或常量断言\
             测试),否则枚举重排会静默漂移"
        );
    }

    /// TG-S06 helper: 递归访问目录下全部 `.rs` 文件。
    fn visit_rs_files(dir: &std::path::Path, f: &mut dyn FnMut(String, &str)) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit_rs_files(&path, f);
            } else if path.extension().is_some_and(|e| e == "rs")
                && let Ok(body) = std::fs::read_to_string(&path)
            {
                let display = path.display().to_string();
                f(display, &body);
            }
        }
    }
}
