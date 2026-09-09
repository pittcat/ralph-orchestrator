//! 2026-09-09-0917 P1 收口计划 U1: admission candidate 构造 + U3 补槽快照。
//!
//! 把 U1「slot refill」从 `DagSchedulerRuntime::tick` 内联抽出来,
//! 形成纯函数 `build_candidates(plan, &JobPipeline)` 与
//! `compute_admission_snapshot(plan, &JobPipeline, target_head)`:
//!   - `build_candidates`: 过滤已 integrated + live job 持有的 unit;
//!   - `compute_admission_snapshot`: 在候选基础上附加 pipeline 实时计数,
//!     返回 owned 的 `AdmissionInputs`,供 `tick` 转成借用的
//!     `AdmissionSnapshot` 喂给 admission 引擎。
//!
//! `compute_admission_snapshot` 是 U1 / U3 「完成 job 后持续补槽」的
//! 生产路径入口:引擎通过 snapshot 拿到 `live_stage_counts` 与
//! `current_job_unit_ids`,据此重新释放 slot 并发现新可调度 unit。
//!
//! 本模块无副作用、无 I/O,纯 CPU,可被 `cargo nextest run -p ralph-cli -- dag_scheduler`
//! 子集稳定覆盖。

use std::collections::{BTreeMap, HashSet};

use ralph_core::supervisor::dag_scheduler::UnitAdmissionInput;

use super::PlanTopology;
use super::jobs::JobPipeline;

/// 从 plan 与 pipeline 派生 admission 候选输入。
///
/// 过滤规则(均 fail-closed:不符合条件的 unit 不出现在 candidates
/// 里,admission 引擎不会收到它们,因此也不会被重复 dispatch):
///   1. **已 integrated**: 出现在 `plan.integrated` 里的 unit
///      已被下游 E2 落库收口,无资格再次进入 dispatch。
///   2. **live job 已持有**: unit 的 `in_flight > 0`,说明上一个
///      tick 已成功 admission 并保留 slot;同一 unit 不应被再次
///      admission(否则 slot 重复扣减)。这是 U1 修复的核心:
///      之前 `tick` 直接把整张 plan.units 传给 admission 引擎,
///      引擎看不到 runtime 侧的 live 状态,所以 U2 会在 U1 还在
///      Review 时被错误地再次 admission,造成同一个 unit 的
///      executor slot 被双重发放。
///
/// 返回的 `UnitAdmissionInput` 仍然带 `integrated_units` 字段
/// (plan.integrated 的 clone),因为 admission 引擎自身的依赖
/// 判定是依赖 `integrated_units` 的;本过滤只排除「unit 自己
/// 已经 integrated」的特例。
pub(crate) fn build_candidates(
    plan: &PlanTopology,
    pipeline: &JobPipeline,
) -> Vec<UnitAdmissionInput> {
    let live_unit_ids = pipeline.live_unit_ids();
    plan.units
        .iter()
        .filter(|u| !plan.integrated.contains(&u.unit_id))
        .filter(|u| !live_unit_ids.contains(&u.unit_id))
        .map(|u| UnitAdmissionInput {
            unit_id: u.unit_id.clone(),
            integration_order: u.integration_order,
            depends_on: u.depends_on.clone(),
            integrated_units: plan.integrated.clone(),
            resource_claims: u.resource_claims.clone(),
        })
        .collect()
}

/// Owned snapshot used by `DagSchedulerRuntime::tick` to feed
/// the borrow-based `AdmissionSnapshot` into
/// `compute_admissions`.
///
/// `live_stage_counts` 与 `current_job_unit_ids` 是 runtime pipeline
/// 在 tick 入口捕获的真实计数:job 完成事件驱动 `pipeline.release`,
/// 下一个 tick 立刻把新的 in_flight 计数通过 `compute_admission_snapshot`
/// 反映到 snapshot;admission 引擎据此释放已完成的 slot 并发现新
/// 可调度 unit(U1 / U3「持续补槽」)。`integration_target_head` 是
/// 集成 lane 的 tip,`None` 表示尚未初始化——admission 引擎会把所有
/// Ready unit 标 `BlockedNoTargetHead`,直到 runtime 提交首个集成 head。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdmissionInputs {
    pub inputs: Vec<UnitAdmissionInput>,
    pub integration_target_head: Option<String>,
    pub live_stage_counts: BTreeMap<String, u32>,
    pub current_job_unit_ids: HashSet<String>,
}

/// Compute the owned admission snapshot for one tick of one plan.
///
/// 1. `build_candidates` 排除已 integrated / live job 持有的 unit;
/// 2. pipeline 的 `live_stage_counts` / `live_unit_ids` 是真实计数
///    (job 完成事件驱动 `release` 后立即反映);
/// 3. `target_head` 是 integration lane 当前 tip(无则 `None`,
///    引擎走 `BlockedNoTargetHead` fail-closed 分支)。
///
/// `tick` 在调用 `compute_admissions` 前,把 `AdmissionInputs` 转换
/// 成借用的 `AdmissionSnapshot`(`inputs` 与 `target_head` borrow)。
pub(crate) fn compute_admission_snapshot(
    plan: &PlanTopology,
    pipeline: &JobPipeline,
    target_head: Option<&str>,
) -> AdmissionInputs {
    AdmissionInputs {
        inputs: build_candidates(plan, pipeline),
        integration_target_head: target_head.map(str::to_owned),
        live_stage_counts: pipeline.live_stage_counts(),
        current_job_unit_ids: pipeline.live_unit_ids(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_core::parallel_forge_handoff::ResourceClaim;

    use crate::loop_runner::dag_scheduler::jobs::DagPools;
    use crate::loop_runner::runtime_job::Stage;

    fn unit_topology(
        id: &str,
        order: u32,
        deps: &[&str],
        claims: Vec<ResourceClaim>,
    ) -> super::super::UnitTopology {
        super::super::UnitTopology {
            unit_id: id.to_string(),
            integration_order: order,
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            tests: Vec::new(),
            resource_claims: claims,
            allowed_paths: Vec::new(),
            forbidden_paths: Vec::new(),
        }
    }

    fn plan(
        units: Vec<super::super::UnitTopology>,
        integrated: &[&str],
    ) -> super::super::PlanTopology {
        super::super::PlanTopology {
            artifact_path: String::new(),
            artifact_digest: String::new(),
            target_branch: String::new(),
            verified_base_commit: None,
            units,
            resource_capacities: Vec::new(),
            integrated: integrated.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn empty_pipeline() -> JobPipeline {
        JobPipeline::new(DagPools::new(8, 2, 2, 2))
    }

    /// 三个未 integrated、live pipeline 为空的 unit 全部进入候选;
    /// `integrated_units` 字段被 clone 进每个 input(供 admission
    /// 引擎判定依赖)。
    #[test]
    fn build_candidates_includes_all_fresh_units() {
        let plan = plan(
            vec![
                unit_topology("U1", 1, &[], Vec::new()),
                unit_topology("U2", 2, &[], Vec::new()),
                unit_topology("U3", 3, &[], Vec::new()),
            ],
            &[],
        );
        let pipeline = empty_pipeline();
        let candidates = build_candidates(&plan, &pipeline);
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].unit_id, "U1");
        assert_eq!(candidates[1].unit_id, "U2");
        assert_eq!(candidates[2].unit_id, "U3");
        for c in &candidates {
            assert!(c.integrated_units.is_empty());
        }
    }

    /// plan.integrated 里的 unit 必须被过滤;否则 admission 引擎
    /// 会再次把已收口的 unit 放进 decisions。
    #[test]
    fn build_candidates_excludes_integrated_units() {
        let plan = plan(
            vec![
                unit_topology("U1", 1, &[], Vec::new()),
                unit_topology("U2", 2, &["U1"], Vec::new()),
            ],
            &["U1"],
        );
        let pipeline = empty_pipeline();
        let candidates = build_candidates(&plan, &pipeline);
        let ids: Vec<&str> = candidates.iter().map(|c| c.unit_id.as_str()).collect();
        assert_eq!(ids, vec!["U2"]);
        // U2 的依赖图里仍能看到 U1 integrated(供 admission 引擎解锁)
        assert_eq!(
            candidates[0].integrated_units,
            std::iter::once("U1".to_string()).collect()
        );
    }

    /// 持有 live job 的 unit 必须被过滤,即使它没有 integrated。
    /// 这是 U1 修复的直接验证:之前 `tick` 漏过这一步,导致 live
    /// unit 二次 admission。
    #[test]
    fn build_candidates_excludes_units_with_live_jobs() {
        let plan = plan(
            vec![
                unit_topology("U1", 1, &[], Vec::new()),
                unit_topology("U2", 2, &[], Vec::new()),
            ],
            &[],
        );
        let mut pipeline = empty_pipeline();
        pipeline.ensure_unit("U1", "j-U1", "executor", Stage::Execute);
        // 模拟 U1 处于 live Execute 状态(已 admission 但尚未 release)
        let outcome = pipeline.advance("U1", Stage::Execute);
        assert!(matches!(
            outcome,
            super::super::jobs::AdvanceOutcome::Admitted { .. }
        ));

        let candidates = build_candidates(&plan, &pipeline);
        let ids: Vec<&str> = candidates.iter().map(|c| c.unit_id.as_str()).collect();
        assert_eq!(ids, vec!["U2"]);
    }

    /// 同时命中 integrated + live 时仍然被过滤(规则串联)。
    #[test]
    fn build_candidates_excludes_when_both_filters_match() {
        let plan = plan(
            vec![
                unit_topology("U1", 1, &[], Vec::new()),
                unit_topology("U2", 2, &[], Vec::new()),
            ],
            &["U1"],
        );
        let mut pipeline = empty_pipeline();
        pipeline.ensure_unit("U1", "j-U1", "executor", Stage::Execute);
        let outcome = pipeline.advance("U1", Stage::Execute);
        assert!(matches!(
            outcome,
            super::super::jobs::AdvanceOutcome::Admitted { .. }
        ));

        let candidates = build_candidates(&plan, &pipeline);
        let ids: Vec<&str> = candidates.iter().map(|c| c.unit_id.as_str()).collect();
        assert_eq!(ids, vec!["U2"]);
    }

    // -----------------------------------------------------------------
    // U3 acceptance tests — `compute_admission_snapshot` 生产路径
    // (fix-plan 2026-09-09-0917-fix-forge-dag-p1-closure-plan §U3.4)。
    // 验证 owned snapshot 字段一致地反映 runtime pipeline 真实状态:
    //   - candidate input 携带 integrated_units,引擎据此判定 deps;
    //   - live_stage_counts / current_job_unit_ids 来自 pipeline 的
    //     in_flight 计数,job release 后立即可见(U1/U3 持续补槽);
    //   - integration_target_head 与当前 lane tip 一致。
    // -----------------------------------------------------------------

    /// U3 / U5 base_pin:依赖链上的 unit,其 `depends_on` 必须全部
    /// 出现在 `integrated_units` 里才会被引擎放行。snapshot 把
    /// `plan.integrated` clone 进每个 input 的 `integrated_units`,
    /// 因此未 ack 的依赖会让该 unit 走到 `BlockedDependencies`。
    #[test]
    fn compute_admission_snapshot_all_dependencies_must_be_acked_ancestors() {
        let plan = plan(
            vec![
                unit_topology("U1", 1, &[], Vec::new()),
                unit_topology("U2", 2, &["U1"], Vec::new()),
                unit_topology("U3", 3, &["U1", "U2"], Vec::new()),
            ],
            &[], // U1 尚未 integrated → U2 / U3 都还有未 ack 的依赖
        );
        let snapshot = compute_admission_snapshot(&plan, &empty_pipeline(), Some("HEAD"));

        // U1 deps 为空 → integrated_units 就是 plan.integrated(clone)。
        let u1 = snapshot
            .inputs
            .iter()
            .find(|u| u.unit_id == "U1")
            .expect("U1 candidate present");
        assert!(
            u1.depends_on.is_empty(),
            "U1 has no deps, must report empty depends_on"
        );
        assert!(
            u1.integrated_units.is_empty(),
            "U1 sees an empty plan.integrated (no ack yet)"
        );

        // U2 / U3 仍在 candidates 里(候选过滤只排除 integrated 自身),
        // 但其 deps 仍未 ack,引擎会基于 integrated_units 标记
        // BlockedDependencies —— U3 acceptance 验证 snapshot 把这个
        // 信息如实透传。
        let u3 = snapshot
            .inputs
            .iter()
            .find(|u| u.unit_id == "U3")
            .expect("U3 candidate present");
        assert_eq!(u3.depends_on, vec!["U1".to_string(), "U2".to_string()]);
        assert!(
            !u3.integrated_units.contains("U1"),
            "U3 sees U1 not yet acked (acks must be required)"
        );
        assert!(
            !u3.integrated_units.contains("U2"),
            "U3 sees U2 not yet acked"
        );
    }

    /// U3 / U5 base_pin:base first write 之后,snapshot 的
    /// `integration_target_head` 与 `live_stage_counts` 在两次调用间
    /// 不可变(同输入同输出)。这是 replay determinism 的基础,也是
    /// admission base 的「first write immutable」语义(snapshot 反映
    /// 同一 base 多次,引擎拿到同一 SHA)。
    #[test]
    fn compute_admission_snapshot_base_first_write_immutable() {
        let plan = plan(
            vec![
                unit_topology("U1", 1, &[], Vec::new()),
                unit_topology("U2", 2, &["U1"], Vec::new()),
            ],
            &[],
        );
        let mut pipeline = empty_pipeline();
        // 模拟一次 job 完成:launch U1,再 release 掉 → 下个 tick
        // 的 in_flight 计数为 0。
        pipeline.ensure_unit("U1", "j-U1", "executor", Stage::Execute);
        let _ = pipeline.advance("U1", Stage::Execute);

        let first = compute_admission_snapshot(&plan, &pipeline, Some("base-SHA"));
        // 在没有新的 release / launch 之前,两次 snapshot 必须 bit-equal:
        // engine 拿到同一份 base SHA 与同一份 live 计数,admission 决策
        // 必然一致(replay 确定性)。
        let second = compute_admission_snapshot(&plan, &pipeline, Some("base-SHA"));
        assert_eq!(first, second, "snapshot must be deterministic across calls");

        // base 不可变:target_head 字段就是「first write 锁定的 base」
        assert_eq!(first.integration_target_head.as_deref(), Some("base-SHA"));
        assert_eq!(second.integration_target_head.as_deref(), Some("base-SHA"));
    }

    /// U3 / U5 base_pin:无依赖的 unit,base pin 直接落到
    /// 当前 integration_target_head;snapshot 透传该 SHA 供引擎做
    /// diff 计算。
    #[test]
    fn compute_admission_snapshot_no_dependencies_pin_current_target() {
        let plan = plan(
            vec![
                unit_topology("U1", 1, &[], Vec::new()),
                unit_topology("U2", 2, &[], Vec::new()),
            ],
            &[],
        );
        // 无依赖 unit 的 base = 当前 target head。
        let snapshot =
            compute_admission_snapshot(&plan, &empty_pipeline(), Some("target-SHA"));

        // 两个无依赖 unit 的 input 都应在 candidates 里,且 depends_on
        // 为空(由 build_candidates / PlanTopology 派生)。
        for u in &snapshot.inputs {
            assert!(
                u.depends_on.is_empty(),
                "no-deps unit must report empty depends_on: {}",
                u.unit_id
            );
        }
        // 引擎拿到的 integration_target_head = 当前 target SHA,
        // admission base 直接 pin 到该 SHA(U5 派生语义)。
        assert_eq!(
            snapshot.integration_target_head.as_deref(),
            Some("target-SHA")
        );
    }

    /// U3 / U5 base_pin:有依赖的 unit,snapshot 必须同时携带
    /// `integration_target_head` 与各 input 的 `integrated_units`,
    /// 引擎据二者拼出 deps 链的 base(用于 diff 与 spawn 的 base)。
    #[test]
    fn compute_admission_snapshot_diff_uses_unit_base() {
        let plan = plan(
            vec![
                unit_topology("U1", 1, &[], Vec::new()),
                unit_topology("U2", 2, &["U1"], Vec::new()),
            ],
            &["U1"], // U1 已 ack → U2 deps 已满足,可进入 ready 集合
        );
        let snapshot = compute_admission_snapshot(&plan, &empty_pipeline(), Some("target-SHA"));

        // U2 是有依赖 unit;U1 已被 integrated,出现在 U2.integrated_units。
        let u2 = snapshot
            .inputs
            .iter()
            .find(|u| u.unit_id == "U2")
            .expect("U2 candidate present");
        assert_eq!(u2.depends_on, vec!["U1".to_string()]);
        assert!(
            u2.integrated_units.contains("U1"),
            "U2 must see U1 in integrated_units so deps pass"
        );

        // base 透传:diff 用 target head + deps acked commits 派生
        // unit_base(plan §U5 base_pin)。snapshot 暴露的
        // integration_target_head 必须与调用方传入的一致。
        assert_eq!(snapshot.integration_target_head.as_deref(), Some("target-SHA"));

        // 无依赖的 U1 仍出现在 candidates(filter 只排除 integrated
        // 自身,U1 不在 candidate filter 排除集合的 unit_id 列里)。
        // 此处 plan 已将 U1 标 integrated → U1 自身被 filter 排除,
        // 仅 U2 留在 inputs。
        assert_eq!(snapshot.inputs.len(), 1);
        assert_eq!(snapshot.inputs[0].unit_id, "U2");
    }

    /// U3 第 5 个 acceptance(plan §U1.7 衍生):snapshot 的
    /// `current_job_unit_ids` 必须与 pipeline 的 `live_unit_ids`
    /// bit-equal;`live_stage_counts` 必须反映每个 in_flight stage
    /// 的真实计数。这是「job 完成事件驱动下个 tick 补槽」的关键:
    /// release 之后,in_flight 计数下降,snapshot 立即把变化传给引擎,
    /// 引擎释放已完成的 slot 并发现新可调度 unit。
    #[test]
    fn compute_admission_snapshot_active_jobs_match_pipeline_live_unit_ids() {
        let plan = plan(
            vec![
                unit_topology("U1", 1, &[], Vec::new()),
                unit_topology("U2", 2, &[], Vec::new()),
                unit_topology("U3", 3, &[], Vec::new()),
            ],
            &[],
        );
        let mut pipeline = empty_pipeline();
        // 模拟 2 个 job 在 execute 中(in_flight.execute = 2)。
        pipeline.ensure_unit("U1", "j-U1", "executor", Stage::Execute);
        pipeline.ensure_unit("U2", "j-U2", "executor", Stage::Execute);
        let _ = pipeline.advance("U1", Stage::Execute);
        let _ = pipeline.advance("U2", Stage::Execute);

        let snapshot = compute_admission_snapshot(&plan, &pipeline, Some("target-SHA"));

        // live_unit_ids 透传:build_candidates 已用它过滤 inputs,
        // snapshot.current_job_unit_ids 是同一份数据的 owned 镜像。
        let expected_ids = pipeline.live_unit_ids();
        assert_eq!(
            snapshot.current_job_unit_ids, expected_ids,
            "snapshot.current_job_unit_ids must mirror pipeline.live_unit_ids()"
        );

        // live_stage_counts 反映当前 stage 占用:execute=2,
        // review/verify 还未出现。
        let expected_counts = pipeline.live_stage_counts();
        assert_eq!(
            snapshot.live_stage_counts, expected_counts,
            "snapshot.live_stage_counts must mirror pipeline.live_stage_counts()"
        );
        assert_eq!(snapshot.live_stage_counts.get("execute"), Some(&2));

        // U3 还没被 launch,因此出现在 candidates 里(空 integrated 集合
        // + 不在 live_unit_ids)。
        let u3 = snapshot
            .inputs
            .iter()
            .find(|u| u.unit_id == "U3")
            .expect("U3 must appear as a candidate (not live, not integrated)");
        assert!(u3.depends_on.is_empty());
    }
}
