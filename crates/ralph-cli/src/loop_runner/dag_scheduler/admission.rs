//! 2026-09-09-0917 P1 收口计划 U1: admission candidate 构造。
//!
//! 把 U1「slot refill」从 `DagSchedulerRuntime::tick` 内联抽出来,
//! 形成纯函数 `build_candidates(plan, &JobPipeline)`:
//!   - 过滤已 integrated 的 unit(快照里已脱离活跃图);
//!   - 过滤当前已有 live job 的 unit(避免 slot 重复发放);
//!   - 其余 unit 透传给 admission 引擎,引擎内部做依赖 / cap 判定。
//!
//! 本模块无副作用、无 I/O,纯 CPU,可被 `cargo nextest run -p ralph-cli -- dag_scheduler`
//! 子集稳定覆盖。

use ralph_core::supervisor::dag_scheduler::UnitAdmissionInput;

use super::jobs::JobPipeline;
use super::PlanTopology;

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
pub(crate) fn build_candidates(plan: &PlanTopology, pipeline: &JobPipeline) -> Vec<UnitAdmissionInput> {
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

    fn plan(units: Vec<super::super::UnitTopology>, integrated: &[&str]) -> super::super::PlanTopology {
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
        assert_eq!(candidates[0].integrated_units, std::iter::once("U1".to_string()).collect());
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
        assert!(matches!(outcome, super::super::jobs::AdvanceOutcome::Admitted { .. }));

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
        assert!(matches!(outcome, super::super::jobs::AdvanceOutcome::Admitted { .. }));

        let candidates = build_candidates(&plan, &pipeline);
        let ids: Vec<&str> = candidates.iter().map(|c| c.unit_id.as_str()).collect();
        assert_eq!(ids, vec!["U2"]);
    }
}
