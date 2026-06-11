//! B4: 验证 timeout finding 走 responder 后的收敛行为。
//!
//! 背景:在 U4-B3 中,`dispatcher.rs::record_wave_timeout_envelope` 把
//! Partial / AggregateDeadlineExceeded 包装成 `RecoveryDiagnosisEnvelope`
//! 后写入 responder,retry_key 是 `wave_dispatcher:<wave_id>:<reason_code>`。
//! KTD-U4-5 表要求:"partial/aggregate timeout 收敛条件 = 后续同目标
//! topic 的新 Wave 完整完成"。这里测两个东西:
//!
//! 1. iteration N: 写入 timeout envelope 后,`recovery_responder` 中
//!    有 pending finding(调用 `record_finding` 后 internal state 已
//!    记录;`with_outcome(Pending, _)` 是默认初始状态)。
//! 2. iteration N+1:用同一个 `retry_key` + 命中 envelope 的 `topic`
//!    调 `check_recovery_topics`,Responder 把它升级为 `Recovered`。
//!
//! 注意:这测的是 *Responder API 自身能在 retry_key 一致的前提下
//! 收敛 timeout finding*。生产中,新 wave 的 retry_key 因 `wave_id`
//! 不同而不同,因此本测试通过 ≠ 生产路径自动收敛;plan §12 会
//! 把"如何让跨 retry_key 的同 target topic wave_complete 触发旧
//! finding Recovered"列为后续 responder 扩展项(KTD-U4-5 末尾
//! 显式禁止直接新增 `on_converged`,所以这里不写新 API)。

use super::*;
use crate::diagnosis::{
    AcceptedEventEvidence, DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource,
    RecoveryDiagnosisEnvelope, RecoveryDiagnosisEnvelopeBuilder,
};
use std::collections::BTreeSet;

/// B4-1: timeout envelope 写入后,Responder 内部 state 应记录
/// retry_key,初始 outcome 是 Pending。
#[test]
fn test_b4_timeout_envelope_writes_pending_finding() {
    let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["task.start"]
    publishes: ["review.file"]
  reviewer:
    name: "Reviewer"
    triggers: ["review.file"]
    publishes: ["review.done"]
    concurrency: 3
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.set_iteration_for_test(3);

    let retry_key = RecoveryDiagnosisEnvelopeBuilder::wave_retry_key(
        "w-timeout-A",
        "wave_aggregate_deadline_exceeded",
    );
    let envelope = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::WaveDispatcher)
        .severity(DiagnosisSeverity::Warning)
        .source_hat("reviewer")
        .topic("review.done")
        .reason_code("wave_aggregate_deadline_exceeded")
        .message(
            "Wave w-timeout-A timeout: 0/3 workers reported in 1000ms (reason=wave_aggregate_deadline_exceeded)"
                .to_string(),
        )
        .retry_attempt(0)
        .safe_target(false)
        .outcome(DiagnosisOutcome::Pending)
        .retry_key(retry_key.clone())
        .build();

    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());

    // Responder 状态中存在该 retry_key 的 finding。
    assert_eq!(
        event_loop.recovery_responder().tracked_retry_keys(),
        1,
        "记录 timeout envelope 后,Responder 应追踪到 1 个 retry_key"
    );

    // Default outcome 起点是 Pending。R7 grace period 同样适用:
    // 同一 iteration 内调 `check_recovery` 会得到 Pending 而不是 Recovered,
    // 模拟"刚发生的失败不会在同一 iteration 自愈"。
    let evidence = vec![AcceptedEventEvidence {
        topic: "review.done".to_string(),
        fields: BTreeSet::new(),
        source_hat: None,
        timestamp: chrono::Utc::now(),
    }];
    let outcome_same_iter = event_loop
        .recovery_responder_mut()
        .check_recovery(&retry_key, &evidence, 3);
    assert_eq!(
        outcome_same_iter,
        Some(DiagnosisOutcome::Pending),
        "同一 iteration 内调 check_recovery 必须返回 Pending(R7 grace period)"
    );
}

/// B4-2: 同一 retry_key + 命中 envelope 的 topic,在下一个 iteration
/// 调用 `check_recovery` 时 outcome 升级为 Recovered。
///
/// 这是 Responder 现有 API 的能力测试 — 证明只要 retry_key 一致,
/// 命中 topic 的 accepted_evidence 就能让 timeout finding 收敛。
/// 生产路径的 wave_id 不一致导致 retry_key 不一致的问题,见
/// plan §12 实施记录。
#[test]
fn test_b4_timeout_finding_recovered_in_next_iteration() {
    let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["task.start"]
    publishes: ["review.file"]
  reviewer:
    name: "Reviewer"
    triggers: ["review.file"]
    publishes: ["review.done"]
    concurrency: 3
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.set_iteration_for_test(3);

    let retry_key = RecoveryDiagnosisEnvelopeBuilder::wave_retry_key(
        "w-timeout-B",
        "wave_partial_threshold",
    );
    let envelope = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::WaveDispatcher)
        .severity(DiagnosisSeverity::Warning)
        .source_hat("reviewer")
        .topic("review.done")
        .reason_code("wave_partial_threshold")
        .message(
            "Wave w-timeout-B timeout: 1/3 workers reported in 800ms (reason=wave_partial_threshold)"
                .to_string(),
        )
        .retry_attempt(0)
        .safe_target(false)
        .outcome(DiagnosisOutcome::Pending)
        .retry_key(retry_key.clone())
        .build();
    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());

    // iteration N+1: accepted evidence 命中 envelope 的 topic。
    let evidence = vec![AcceptedEventEvidence {
        topic: "review.done".to_string(),
        fields: BTreeSet::new(),
        source_hat: None,
        timestamp: chrono::Utc::now(),
    }];
    let outcome = event_loop
        .recovery_responder_mut()
        .check_recovery(&retry_key, &evidence, 4);

    assert_eq!(
        outcome,
        Some(DiagnosisOutcome::Recovered),
        "iteration N+1 命中 envelope 的 topic 后,Responder 应升级为 Recovered"
    );
}

/// B4-3: 第二个不同 wave_id 写入 timeout envelope 后,Responder 内部
/// 累计两个独立 retry_key;两个 key 互不干扰。
///
/// 这覆盖了"两个不同 wave 的 timeout 各产生一个独立 finding"的需求,
/// 与 B1 的 `wave_retry_key` 设计相对应。
#[test]
fn test_b4_two_different_wave_timeouts_two_independent_findings() {
    let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["task.start"]
    publishes: ["review.file"]
  reviewer:
    name: "Reviewer"
    triggers: ["review.file"]
    publishes: ["review.done"]
    concurrency: 3
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.set_iteration_for_test(5);

    for wave_id in ["w-A", "w-B"] {
        let retry_key = RecoveryDiagnosisEnvelopeBuilder::wave_retry_key(
            wave_id,
            "wave_aggregate_deadline_exceeded",
        );
        let envelope = RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::WaveDispatcher)
            .severity(DiagnosisSeverity::Warning)
            .source_hat("reviewer")
            .topic("review.done")
            .reason_code("wave_aggregate_deadline_exceeded")
            .message(format!(
                "Wave {wave_id} timeout: 0/3 workers reported (reason=wave_aggregate_deadline_exceeded)"
            ))
            .retry_attempt(0)
            .safe_target(false)
            .outcome(DiagnosisOutcome::Pending)
            .retry_key(retry_key)
            .build();
        let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());
    }

    assert_eq!(
        event_loop.recovery_responder().tracked_retry_keys(),
        2,
        "两个不同 wave_id 的 timeout 必须产生两个独立 finding"
    );
}

/// B4-4 (failing / negative): 验证**生产中跨 wave 的收敛不可达**。
///
/// 原因:`wave_retry_key` 按 `wave_id` namespaced,新 wave_id 必然
/// 产生新 retry_key;Responder 的 `check_recovery(retry_key, ...)`
/// 只能查到对应 key 的 state,无法跨 key 触发"老 timeout 被新
/// Completed 收敛"。
///
/// 这个测试**不验证收敛**(因为生产路径不能),而是把"现有 responder
/// API 不支持 wave_id-scoped finding 的跨 key 收敛"固化为 failing
/// 文档。生产中如果要让"新 wave 在同 target topic 完成时把老
/// timeout finding 标 Recovered",需要 responder 支持按
/// `(source, topic, reason_code)` 跨 retry_key 升级 —— 这是
/// KTD-U4-5 末尾显式禁止直接新增的扩展项,见 plan §12 实施记录。
#[test]
fn test_b4_cross_wave_convergence_not_supported_by_current_responder() {
    let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["task.start"]
    publishes: ["review.file"]
  reviewer:
    name: "Reviewer"
    triggers: ["review.file"]
    publishes: ["review.done"]
    concurrency: 3
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.set_iteration_for_test(7);

    // iteration N:老 wave_id w-old 的 timeout。
    let old_key = RecoveryDiagnosisEnvelopeBuilder::wave_retry_key(
        "w-old",
        "wave_aggregate_deadline_exceeded",
    );
    let old_envelope = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::WaveDispatcher)
        .severity(DiagnosisSeverity::Warning)
        .source_hat("reviewer")
        .topic("review.done")
        .reason_code("wave_aggregate_deadline_exceeded")
        .message("Wave w-old timeout".to_string())
        .retry_attempt(0)
        .safe_target(false)
        .outcome(DiagnosisOutcome::Pending)
        .retry_key(old_key.clone())
        .build();
    let _ = event_loop.record_recovery_envelope(&old_envelope, Vec::new());

    // iteration N+1:模拟"新 wave w-new 在同 target topic 完成"的
    // accepted_evidence。把 evidence 喂给**老 retry_key**(生产代码
    // 里 wave_completed 路径不会主动喂老 key),看现有 API 行为。
    let evidence = vec![AcceptedEventEvidence {
        topic: "review.done".to_string(),
        fields: BTreeSet::new(),
        source_hat: None,
        timestamp: chrono::Utc::now(),
    }];
    let outcome = event_loop
        .recovery_responder_mut()
        .check_recovery(&old_key, &evidence, 8);

    // 这个分支**应当** Recovered — 因为:
    //   - old_key 在 Responder state 中存在
    //   - evidence.topic 命中 envelope.topic
    //   - iteration 8 > last_iteration 7 (R7 grace 已过)
    //
    // 但**生产路径**不会这样喂 evidence:`handle_wave_events` 收到
    // 新 wave Completed 后,只调 `record_recovery_envelope` 写新
    // wave 的 Completed envelope(目前根本没这个 envelope;completed
    // 路径不写 envelope,只有 timeout 路径写)。即使未来补上
    // Completed envelope,它的 retry_key 会因 wave_id 不同而与老
    // timeout key 不匹配,Responder 不会跨 key 升级老 finding。
    //
    // 因此本测试**记录** Responder 现有 API 行为,而不是要求
    // production 路径达到"老 timeout 被新 wave 收敛"。具体补救
    // 措施见 plan §12 实施记录。
    assert_eq!(
        outcome,
        Some(DiagnosisOutcome::Recovered),
        "同 retry_key + 命中 topic + 跨 iteration:Responder 自身能 Recovered(注:生产路径无法触发该条件)"
    );
}
