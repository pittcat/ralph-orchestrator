//! 2026-09-03-0959 plan U6 — the `EventLoop` → pipeline driver.
//!
//! The runtime's `EventLoop` calls `DagSchedulerDriver::observe_accepted`
//! when a worker-emitted event passes the existing acceptance
//! gate. The driver routes the event into the right pipeline
//! slot:
//!   - `forge.exec.unit.completed` → `JobPipeline::advance(unit, Review)`
//!   - `forge.review.verdict` (approve) → `JobPipeline::advance(unit, Verify)`
//!   - `forge.review.verdict` (request_changes) →
//!     `JobPipeline::bump_attempt_and_advance(unit, Review)`
//!   - any other topic → ignored (driver is observation-only)
//!
//! On `Block`, the driver returns the typed reason so the caller
//! can publish `forge.plan.blocked` (or `forge.final.correction.settled`,
//! per U4 / U5 wiring).
//!
//! The driver does NOT spawn subprocesses. It only routes
//! events. Subprocesses are launched by the runtime's job
//! kernel (`runtime_job::worker`), which the runtime invokes
//! after the driver returns `Admitted`.

#[cfg(test)]
use serde_json::Value;

#[cfg(test)]
use super::jobs::{AdvanceOutcome, JobPipeline};
#[cfg(test)]
use crate::loop_runner::runtime_job::JobToken;
#[cfg(test)]
use crate::loop_runner::runtime_job::{RuntimeJobError, Stage};

/// Topics the driver recognises. The list is intentionally
/// narrow — anything outside it is a no-op so the driver never
/// silently corrupts a pipeline slot.
///
/// `#[cfg(test)]` for U6: only the driver test mod and the
/// `inspect` integration test reference these constants.
/// U7 promotes them to pub once the integration half hands the
/// driver to the live runtime.
#[cfg(test)]
pub mod topics {
    pub const EXEC_UNIT_COMPLETED: &str = "forge.exec.unit.completed";
    pub const REVIEW_VERDICT: &str = "forge.review.verdict";
}

/// `#[cfg(test)]` for U6 — see `topics` rationale. U7 promotes
/// it.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewVerdict {
    Approve,
    RequestChanges,
}

#[cfg(test)]
impl ReviewVerdict {
    /// Parse from the event payload's `verdict` field. Returns
    /// `None` if the field is missing or unrecognised — the
    /// driver treats that as a no-op so a malformed event does
    /// not advance state.
    pub fn from_payload(payload: &Value) -> Option<Self> {
        let s = payload.get("verdict")?.as_str()?;
        match s {
            "approve" => Some(Self::Approve),
            "request_changes" => Some(Self::RequestChanges),
            _ => None,
        }
    }
}

/// Outcome of a single `observe_accepted` call.
///
/// `#[cfg(test)]` for U6 — see `topics` rationale. U7 promotes
/// it.
#[cfg(test)]
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
/// `#[cfg(test)]` for U6 — see `topics` rationale. U7 promotes
/// it once the live runtime drives the driver.
#[cfg(test)]
pub struct DagSchedulerDriver<'a> {
    pipeline: &'a mut JobPipeline,
}

#[cfg(test)]
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
            topics::EXEC_UNIT_COMPLETED => {
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
            topics::REVIEW_VERDICT => match ReviewVerdict::from_payload(payload) {
                Some(ReviewVerdict::Approve) => {
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
                Some(ReviewVerdict::RequestChanges) => {
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
        let out = driver.observe_accepted(topics::EXEC_UNIT_COMPLETED, "U-1", &json!({}));
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

    /// Review-approve routes to Verify.
    #[test]
    fn review_approve_routes_to_verify() {
        let (_pools, mut pipeline) = driver_fixture();
        pipeline.ensure_unit("U-2", "j-2", "executor", Stage::Execute);
        let _ = pipeline.advance("U-2", Stage::Execute);
        pipeline.release("U-2");
        let _ = pipeline.advance("U-2", Stage::Review);
        let mut driver = DagSchedulerDriver::new(&mut pipeline);
        let out = driver.observe_accepted(
            topics::REVIEW_VERDICT,
            "U-2",
            &json!({"verdict": "approve"}),
        );
        match out {
            DriverOutcome::Routed { next_stage, .. } => {
                assert_eq!(next_stage, Stage::Verify);
            }
            other => panic!("expected Routed, got {other:?}"),
        }
    }

    /// Review-request_changes bumps attempt and re-routes to
    /// Review with a fresh token.
    #[test]
    fn review_request_changes_bumps_attempt() {
        let (_pools, mut pipeline) = driver_fixture();
        pipeline.ensure_unit("U-3", "j-3", "executor", Stage::Execute);
        let _ = pipeline.advance("U-3", Stage::Execute);
        pipeline.release("U-3");
        let _ = pipeline.advance("U-3", Stage::Review);
        pipeline.release("U-3");
        let mut driver = DagSchedulerDriver::new(&mut pipeline);
        let out = driver.observe_accepted(
            topics::REVIEW_VERDICT,
            "U-3",
            &json!({"verdict": "request_changes"}),
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

    // =====================================================================
    // TG-S06 (P2 / interface-topic, PMI-002 §4 + PMI-003): driver 平行
    // topic 宇宙与真实拓扑不相交 pin。真实体系的 topic 名是
    // `exec.unit.done` / `forge.wave.reviewed`(presets/schemas/
    // parallel-forge.yml 声明,dispatcher/reviewer 实际发布);driver
    // 期望的是 `forge.exec.unit.completed` / `forge.review.verdict`
    // ——两套命名不相交。本组测试钉住: 真实 topic 喂 driver 时必须
    // 全部 `Ignored` 且 pipeline 状态零推进;静态断言两套 topic 集合
    // 交集为空。promote 焊接时若直接把真实事件流接进 driver(无映射
    // 决策),事件会被静默丢弃(driver 对未知 topic 是 no-op,fail-safe
    // 但静默)——届时本组测试变红,强制先做映射决策。
    // =====================================================================

    /// TG-S06 步骤 1+2: 用真实 topology topic(`exec.unit.done` /
    /// `forge.wave.reviewed` 的 payload 形态——exec.unit.done 带
    /// wave_id/slot_index/content_hash;forge.wave.reviewed 带
    /// unit_verdicts/aggregate_verdict,verdict 值为
    /// ACCEPTED/REJECTED,driver 的 `ReviewVerdict::from_payload`
    /// 读的是 `verdict` 字符串字段)喂 driver → 断言 `Ignored` 且
    /// pipeline 状态零推进(用 `still_executing` 探测 unit 仍在
    /// Execute 初始 stage)。
    #[test]
    fn tg_s06_real_topology_topics_are_ignored_by_driver() {
        let (_pools, mut pipeline) = driver_fixture();
        pipeline.ensure_unit("U-real", "j-real", "executor", Stage::Execute);
        let mut driver = DagSchedulerDriver::new(&mut pipeline);

        // 真实 exec.unit.done payload 形态(schema: wave_id/slot_index/
        // content_hash/unit_id/plan_key...)。
        let exec_done = json!({
            "wave_id": "tg-s06-wave",
            "slot_index": 0,
            "content_hash": "tg-s06-h0",
            "unit_id": "unit-u1",
            "plan_key": "tg-s06"
        });
        let out = driver.observe_accepted("exec.unit.done", "U-real", &exec_done);
        assert!(
            matches!(out, DriverOutcome::Ignored { .. }),
            "TG-S06: 真实 topic `exec.unit.done` 被 driver 消费了({out:?})\
             ——真实事件流被直接接进 driver,review 语义错位。promote 前\
             必须先做 forge.exec.unit.completed ↔ exec.unit.done 映射决策"
        );

        // 真实 forge.wave.reviewed payload 形态(schema: wave_id/
        // wave_index/unit_verdicts/aggregate_verdict;verdict 值为
        // ACCEPTED/REJECTED,而 driver 的 ReviewVerdict 读
        // `verdict: "approve"` ——字段名与值域都不相交)。
        let wave_reviewed = json!({
            "wave_id": "tg-s06-wave",
            "wave_index": 0,
            "unit_verdicts": {"unit-u1": "ACCEPTED"},
            "aggregate_verdict": "ACCEPTED",
            "plan_key": "tg-s06"
        });
        let out = driver.observe_accepted("forge.wave.reviewed", "U-real", &wave_reviewed);
        assert!(
            matches!(out, DriverOutcome::Ignored { .. }),
            "TG-S06: 真实 topic `forge.wave.reviewed` 被 driver 消费了({out:?})\
             ——promote 前必须先做 forge.review.verdict ↔ \
             forge.wave.reviewed 映射决策"
        );

        // pipeline 状态零推进: U-real 仍在 Execute(初始注册 stage)。
        // `still_executing` 返回 unit 的当前 stage——若 driver 曾把
        // unit 推进到 Review,这里会返回 Review。
        match pipeline.still_executing("U-real") {
            AdvanceOutcome::StillExecuting { stage, .. } => {
                assert_eq!(
                    stage,
                    Stage::Execute,
                    "TG-S06: driver 对真实 topic 产生了 pipeline 状态推进\
                     (U-real 已离开 Execute)"
                );
            }
            other => panic!("TG-S06: still_executing 应返回 StillExecuting,得到 {other:?}"),
        }
    }

    /// TG-S06 步骤 3(静态一致性): driver 引用的 topic 字符串集合与
    /// preset schema topic 集合交集为空。附带 PMI-003 的两个平行定义
    /// 检查: `compute_resource_aware_digest` 零生产消费、`PHASE_*`
    /// 无 `WaveDeliveryState` 枚举绑定断言——一旦有人接线,此静态
    /// 断言红,提示先做映射决策。
    #[test]
    fn tg_s06_driver_topics_disjoint_from_real_topology_and_parallel_defs_unwired() {
        // ── 1. driver topic 宇宙 vs preset schema topic 宇宙 ──
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
        // 证据自检: 真实体系的两个 topic 必须在 schema 集合里。
        assert!(schema_topics.contains(&"exec.unit.done"));
        assert!(schema_topics.contains(&"forge.wave.reviewed"));

        let driver_topics = [topics::EXEC_UNIT_COMPLETED, topics::REVIEW_VERDICT];
        for dt in driver_topics {
            assert!(
                !schema_topics.contains(&dt),
                "TG-S06: driver topic `{dt}` 已出现在 preset schema topic 集\
                 合中——两套命名开始焊接。焊接前必须先做显式映射决策\
                 (PMI-002 §4),否则真实事件流按错误命名被 driver 静默消费"
            );
        }

        // ── 2. PMI-003①: compute_resource_aware_digest 零生产消费 ──
        // 生产链路(emit 桥 canonicalize_plan_ready_payload + projector
        // load_plan_handoff)用的是 artifact_canonicalizer::canonicalize;
        // 资源感知摘要只在 parallel_forge_handoff.rs 自身的单测内自用。
        // 断言: 整个 ralph-core src 树中,该函数名除定义文件自身外
        // 零命中(注释引用算命中——接线者会先在注释里声明意图,那时
        // 本断言变红,提示同时更新权威链)。
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
