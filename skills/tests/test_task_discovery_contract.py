"""ralph-task-discovery task brief 数据契约 + 硬门禁 validator 的 contract 测试(U1 + U3)。

被测契约(frozen):

* 五个关键置信度维度各自独立过 >= 0.85 硬门禁,禁止用平均值绕过;
* 候选方案除 confidence 外还有独立覆盖门禁(goal_coverage >= 0.80、
  acceptance_coverage >= 0.85、project_fit >= 0.75);
* Evidence 台账引用完整性;Decision / Candidate 引用的证据必须存在;
* author_ready 的充要条件全部满足才允许 handoff;
* attempt_count >= 3 且不达标 → blocked,不得建议第四轮自动调查;
* 每个错误带稳定 code、JSON-path 位置、message 与 next_action。

U3 增补契约(评分 SSOT 见 references/confidence-and-candidate-rubric.md):

* 确定性支持度 compute_support:证据等级权重、按 id 去重(重复引用不加分);
* 候选淘汰:低置信度 / 覆盖不足分别记 rejected_low_confidence /
  rejected_insufficient_coverage,高分不掩盖单维度短板;
* investigation_attempts 重算审计:round 连续、证据存在、链式衔接、
  声明分不得超过去重证据支持度(score_inflation);
* 至多一个 selected(ambiguous_selected_candidates);
* selected 达标候选必须含 E3/E4 完成证据;
* 同主题矛盾 E3/E4 证据未裁决前禁止 author_ready。

所有 fixture 都走 YAML 文本入口(``validate_brief_text``)验证,
确保真实经过 ``yaml.safe_load``,不被 Python dict 构造绕过格式检查。
"""
from __future__ import annotations

import json
import re
from pathlib import Path

import pytest
import yaml

from brief_validator import validate_brief_data, validate_brief_text
from task_brief import KEY_DIMENSIONS, TaskBrief

FIXTURES = Path(__file__).resolve().parents[1] / "ralph-task-discovery" / "fixtures"
ALL_FIXTURES = (
    "valid.yml",
    "missing-evidence.yml",
    "medium-confidence.yml",
    "low-confidence.yml",
    "coverage-fail.yml",
    "blocked.yml",
    # U3:候选淘汰 / 重算审计 / 矛盾证据 / 等分歧义
    "alternative.yml",
    "recompute.yml",
    "conflicting-evidence.yml",
    "ambiguous-selected.yml",
)


def _fixture_text(name: str) -> str:
    return (FIXTURES / name).read_text(encoding="utf-8")


def _validate_fixture(name: str):
    return validate_brief_text(_fixture_text(name))


def _as_data(name: str) -> dict:
    return yaml.safe_load(_fixture_text(name))


def _mutated(name: str, mutate) -> str:
    data = _as_data(name)
    mutate(data)
    return yaml.safe_dump(data, allow_unicode=True)


def _codes(result) -> set[str]:
    return {error.code for error in result.errors}


# --- fixtures 基础可用性 ----------------------------------------------------


@pytest.mark.parametrize("name", ALL_FIXTURES)
def test_fixture_exists_and_parses_as_yaml(name: str) -> None:
    data = yaml.safe_load(_fixture_text(name))
    assert isinstance(data, dict)


# --- valid brief:author_ready=true ----------------------------------------


def test_valid_fixture_is_author_ready() -> None:
    result = _validate_fixture("valid.yml")
    assert result.valid is True
    assert result.author_ready is True
    assert result.recommended_status == "author_ready"
    assert result.errors == ()
    assert result.handoff_block_reasons == ()
    assert result.missing_evidence == ()
    assert result.next_action == "ready_for_handoff"
    selected = [g for g in result.candidate_gates if g.outcome == "selected"]
    assert [g.candidate_id for g in selected] == ["C1"]
    assert selected[0].failed_gates == ()


def test_valid_fixture_parses_into_typed_brief() -> None:
    result = _validate_fixture("valid.yml")
    brief = result.brief
    assert isinstance(brief, TaskBrief)
    assert brief.schema_version == "1.0"
    assert brief.status == "author_ready"
    assert brief.previous_status == "needs_investigation"
    assert brief.attempt_count == 2
    assert set(brief.confidence) == set(KEY_DIMENSIONS)
    assert {e.id for e in brief.evidence} == {"E1", "E2", "E3", "E4"}
    assert brief.decisions[0].blocking is True
    assert brief.candidates[0].selected is True
    assert brief.user_confirmations["goal"].confirmed is True


def test_parsed_mapping_input_is_supported() -> None:
    result = validate_brief_data(_as_data("valid.yml"))
    assert result.valid is True
    assert result.author_ready is True


# --- 维度阈值硬门禁(禁止平均值绕过) --------------------------------------


def test_key_dimension_084_prevents_author_ready() -> None:
    def mutate(data: dict) -> None:
        data["confidence"]["goal_clarity"] = 0.84

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    assert result.author_ready is False
    assert result.recommended_status == "needs_investigation"
    assert "author_ready_gate_violation" in _codes(result)


def test_key_dimension_069_is_rejected_low_confidence() -> None:
    result = _validate_fixture("low-confidence.yml")
    assert result.valid is True  # 诚实声明 needs_investigation,与门禁一致
    assert result.author_ready is False
    assert result.recommended_status == "needs_investigation"
    dimension_rejections = [r for r in result.rejections if r.kind == "dimension"]
    assert any(
        r.id == "goal_clarity" and r.reason == "rejected_low_confidence"
        for r in dimension_rejections
    )
    # 下一步要求重新调查/逐题确认,绝不能 handoff
    assert result.next_action == "rerun_investigation"


def test_medium_confidence_fixture_is_honest_needs_investigation() -> None:
    result = _validate_fixture("medium-confidence.yml")
    assert result.valid is True
    assert result.author_ready is False
    assert result.recommended_status == "needs_investigation"
    assert result.next_action == "rerun_investigation"


def test_boundary_070_belongs_to_investigation_band_not_rejected() -> None:
    def mutate(data: dict) -> None:
        data["status"] = "needs_investigation"
        data["confidence"]["risk_coverage"] = 0.70

    result = validate_brief_text(_mutated("valid.yml", mutate))
    # 0.70 是调查带的下边界(包含),不应产生 rejected_low_confidence
    assert not any(
        r.kind == "dimension"
        and r.id == "risk_coverage"
        and r.reason == "rejected_low_confidence"
        for r in result.rejections
    )
    assert result.author_ready is False
    assert result.recommended_status == "needs_investigation"


def test_boundary_085_belongs_to_pass_band() -> None:
    # valid fixture 的五维恰好都是 0.85,应全部判为达标
    result = _validate_fixture("valid.yml")
    assert result.author_ready is True
    assert not any(r.kind == "dimension" for r in result.rejections)


def test_out_of_range_score_is_invalid_score() -> None:
    def mutate(data: dict) -> None:
        data["confidence"]["goal_clarity"] = 1.2

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert any(
        e.code == "invalid_score" and e.path == "$.confidence.goal_clarity"
        for e in result.errors
    )


def test_non_numeric_score_is_invalid_score() -> None:
    def mutate(data: dict) -> None:
        data["confidence"]["goal_clarity"] = "high"

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert "invalid_score" in _codes(result)


def test_missing_confidence_section_falls_back_to_draft() -> None:
    def mutate(data: dict) -> None:
        del data["confidence"]

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    assert any(
        e.code == "missing_required_field" and e.path == "$.confidence"
        for e in result.errors
    )
    assert result.recommended_status == "draft"


# --- Evidence 台账与引用完整性 ----------------------------------------------


def test_missing_evidence_fixture_reports_integrity_errors() -> None:
    result = _validate_fixture("missing-evidence.yml")
    assert result.valid is False
    assert {"unreferenced_evidence", "missing_required_field"} <= _codes(result)
    assert "E9" in result.missing_evidence
    assert result.author_ready is False


def test_evidence_entry_without_id_is_rejected() -> None:
    def mutate(data: dict) -> None:
        del data["evidence"][0]["id"]

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    # 证据完整性家族错误(missing_required_field / unreferenced_evidence)
    assert _codes(result) & {"missing_required_field", "unreferenced_evidence"}


def test_evidence_entry_without_source_is_rejected() -> None:
    def mutate(data: dict) -> None:
        del data["evidence"][1]["source"]

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    assert any(
        e.code == "missing_required_field" and e.path == "$.evidence[1].source"
        for e in result.errors
    )


def test_decision_without_supporting_evidence_is_rejected() -> None:
    def mutate(data: dict) -> None:
        data["decisions"][0]["supporting_evidence"] = []

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    assert "unreferenced_evidence" in _codes(result)


def test_dangling_evidence_reference_is_reported() -> None:
    def mutate(data: dict) -> None:
        data["decisions"][0]["supporting_evidence"] = ["E1", "E42"]

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    assert "unreferenced_evidence" in _codes(result)
    assert "E42" in result.missing_evidence


def test_invalid_evidence_level_is_rejected() -> None:
    def mutate(data: dict) -> None:
        data["evidence"][0]["level"] = "E7"

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    assert any(
        e.code == "invalid_evidence_level" and e.path == "$.evidence[0].level"
        for e in result.errors
    )


# --- 候选方案独立覆盖门禁 ----------------------------------------------------


def test_coverage_fail_fixture_blocks_author_ready() -> None:
    result = _validate_fixture("coverage-fail.yml")
    assert result.valid is False
    assert result.author_ready is False
    assert {"candidate_coverage_gate_failed", "author_ready_gate_violation"} <= _codes(result)
    gate = next(g for g in result.candidate_gates if g.candidate_id == "C1")
    assert gate.outcome == "rejected_insufficient_coverage"
    assert "acceptance_coverage" in gate.failed_gates
    assert result.next_action == "switch_candidate"


def test_high_confidence_but_acceptance_coverage_084_rejected() -> None:
    def mutate(data: dict) -> None:
        data["candidates"][0]["acceptance_coverage"] = 0.84

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.author_ready is False
    assert "candidate_coverage_gate_failed" in _codes(result)


# --- attempt 耗尽 → blocked,禁止第四轮自动调查 -----------------------------


def test_blocked_fixture_is_terminal_with_human_input() -> None:
    result = _validate_fixture("blocked.yml")
    assert result.valid is True  # 诚实声明 blocked,与门禁结论一致
    assert result.author_ready is False
    assert result.recommended_status == "blocked"
    assert result.next_action == "confirm_with_user"
    # 任何错误/丢弃建议都不得指向第四轮自动调查
    assert all(e.next_action != "rerun_investigation" for e in result.errors)
    assert all(r.next_action != "rerun_investigation" for r in result.rejections)


def test_attempt_exhaustion_requires_blocked_declaration() -> None:
    def mutate(data: dict) -> None:
        data["status"] = "needs_investigation"

    result = validate_brief_text(_mutated("blocked.yml", mutate))
    assert result.valid is False
    assert result.recommended_status == "blocked"
    assert any(
        e.code == "state_transition_invalid" and e.next_action == "emit_blocked"
        for e in result.errors
    )


# --- author_ready 其余充要条件 ----------------------------------------------


def test_unconfirmed_scope_blocks_author_ready() -> None:
    def mutate(data: dict) -> None:
        data["user_confirmations"]["scope"]["confirmed"] = False

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    assert result.author_ready is False
    assert result.recommended_status == "needs_user_decision"
    assert "author_ready_gate_violation" in _codes(result)
    assert result.next_action == "confirm_with_user"
    assert result.handoff_block_reasons


def test_unresolved_blocking_decision_blocks_author_ready() -> None:
    def mutate(data: dict) -> None:
        data["decisions"][0]["resolved"] = False

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    assert result.author_ready is False
    assert result.recommended_status == "needs_user_decision"
    assert "author_ready_gate_violation" in _codes(result)


def test_blocking_decision_low_confidence_rejected() -> None:
    def mutate(data: dict) -> None:
        data["decisions"][0]["confidence"] = 0.60

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.author_ready is False
    assert any(
        r.kind == "decision" and r.id == "D1" and r.reason == "rejected_low_confidence"
        for r in result.rejections
    )


# --- 状态枚举与单向转换 ------------------------------------------------------


def test_unknown_status_value_is_rejected() -> None:
    def mutate(data: dict) -> None:
        data["status"] = "ready"

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    assert "unknown_status" in _codes(result)


def test_blocked_cannot_transition_to_auto_investigation() -> None:
    def mutate(data: dict) -> None:
        data["previous_status"] = "blocked"
        data["status"] = "needs_investigation"
        data["attempt_count"] = 1  # 隔离耗尽规则,只测转换表

    result = validate_brief_text(_mutated("blocked.yml", mutate))
    assert result.valid is False
    assert "state_transition_invalid" in _codes(result)


def test_draft_cannot_jump_directly_to_author_ready() -> None:
    def mutate(data: dict) -> None:
        data["previous_status"] = "draft"

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    assert "state_transition_invalid" in _codes(result)


# --- schema_version / project_root provenance -------------------------------


def test_missing_schema_version_is_rejected() -> None:
    def mutate(data: dict) -> None:
        del data["schema_version"]

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    assert "schema_version_invalid" in _codes(result)


def test_unsupported_schema_version_is_rejected() -> None:
    def mutate(data: dict) -> None:
        data["schema_version"] = "2.0"

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    assert "schema_version_invalid" in _codes(result)


def test_missing_project_root_is_rejected() -> None:
    def mutate(data: dict) -> None:
        del data["project_root"]

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    assert "root_provenance_missing" in _codes(result)


# --- YAML 文本入口与可序列化输出 --------------------------------------------


def test_malformed_yaml_text_is_rejected() -> None:
    result = validate_brief_text("schema_version: [unclosed")
    assert result.valid is False
    assert "invalid_yaml" in _codes(result)


def test_non_mapping_yaml_is_rejected() -> None:
    result = validate_brief_text("- just\n- a\n- list\n")
    assert result.valid is False
    assert "missing_required_field" in _codes(result)


def test_result_serializes_to_dict_json_and_yaml() -> None:
    result = _validate_fixture("coverage-fail.yml")
    payload = result.to_dict()
    for key in (
        "valid",
        "author_ready",
        "recommended_status",
        "next_action",
        "errors",
        "rejections",
        "candidate_gates",
        "handoff_block_reasons",
        "missing_evidence",
    ):
        assert key in payload
    for error in payload["errors"]:
        assert set(error) == {"code", "path", "message", "next_action"}
    round_tripped = json.loads(json.dumps(payload))
    assert round_tripped["valid"] is False
    assert result.to_yaml()


# --- U3:确定性支持度计算(纯函数,去重语义) ---------------------------------


def test_compute_support_dedups_repeated_evidence_ids() -> None:
    # 验收:同一 Evidence id 重复引用 3 次 → 支持度与只出现 1 次相同
    from task_brief import EVIDENCE_LEVEL_SUPPORT, compute_support

    levels = {"E1": "E2"}
    once = compute_support(["E1"], levels)
    thrice = compute_support(["E1", "E1", "E1"], levels)
    assert once == pytest.approx(EVIDENCE_LEVEL_SUPPORT["E2"])
    assert thrice == once  # 重复引用不加分
    # 台账中不存在的 id 贡献 0(引用完整性由 validator 单独审计)
    assert compute_support(["E1", "GHOST"], levels) == once
    assert compute_support([], levels) == 0.0
    # 支持度上限 1.0
    many = {f"X{i}": "E4" for i in range(5)}
    assert compute_support(list(many), many) == pytest.approx(1.0)


def test_support_weights_strictly_increase_by_level() -> None:
    from task_brief import EVIDENCE_LEVELS, EVIDENCE_LEVEL_SUPPORT

    weights = [EVIDENCE_LEVEL_SUPPORT[level] for level in EVIDENCE_LEVELS]
    assert weights == sorted(weights), "E0<E1<E2<E3<E4 必须单调"
    assert len(set(weights)) == len(weights), "权重必须严格递增"
    assert all(0.0 < weight <= 1.0 for weight in weights)
    assert set(EVIDENCE_LEVEL_SUPPORT) == set(EVIDENCE_LEVELS)


# --- U3:多候选淘汰(alternative.yml) ----------------------------------------


def test_alternative_fixture_rejects_low_coverage_candidate() -> None:
    # 验收:两候选均 confidence 0.9,A acceptance_coverage=0.70、B 全达标
    # → A rejected_insufficient_coverage,B selected,author_ready 成立
    result = _validate_fixture("alternative.yml")
    assert result.valid is True
    assert result.author_ready is True
    assert result.recommended_status == "author_ready"
    assert result.next_action == "ready_for_handoff"
    gates = {g.candidate_id: g for g in result.candidate_gates}
    assert gates["C-A"].outcome == "rejected_insufficient_coverage"
    assert gates["C-A"].failed_gates == ("acceptance_coverage",)
    assert gates["C-B"].outcome == "selected"
    assert gates["C-B"].failed_gates == ()
    # 新增字段进入类型化视图:risk_coverage 仅展示/追踪,status 记录淘汰结论
    brief = result.brief
    assert brief is not None
    by_id = {c.id: c for c in brief.candidates}
    assert by_id["C-A"].risk_coverage == pytest.approx(0.62)
    assert by_id["C-A"].status == "rejected_insufficient_coverage"
    assert by_id["C-A"].rejection_reason
    assert by_id["C-B"].status == "selected"


def test_candidate_confidence_069_rejected_low_confidence() -> None:
    # 验收:候选 confidence 0.69 → rejected_low_confidence,保留证据引用
    def mutate(data: dict) -> None:
        data["candidates"][0]["confidence"] = 0.69

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    assert result.author_ready is False
    candidate_rejections = [r for r in result.rejections if r.kind == "candidate"]
    assert any(
        r.id == "C1" and r.reason == "rejected_low_confidence"
        for r in candidate_rejections
    )
    gate = next(g for g in result.candidate_gates if g.candidate_id == "C1")
    assert gate.outcome == "rejected_low_confidence"
    assert gate.failed_gates == ("confidence",)
    # 淘汰不破坏证据引用完整性(C1 的 [E1, E3] 引用仍在台账中)
    assert result.missing_evidence == ()
    assert "unreferenced_evidence" not in _codes(result)


def test_candidate_confidence_078_stays_in_investigation_band() -> None:
    # 验收:候选 confidence 0.78 → 不进 author_ready,补证据动作可见
    def mutate(data: dict) -> None:
        data["candidates"][0]["confidence"] = 0.78
        data["candidates"] = data["candidates"][:1]  # 隔离候选替换分支

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False  # 声明 author_ready 但门禁未满足
    assert result.author_ready is False
    assert result.recommended_status == "needs_investigation"
    assert result.next_action == "rerun_investigation"
    gate = next(g for g in result.candidate_gates if g.candidate_id == "C1")
    assert gate.outcome == "needs_investigation"
    assert result.handoff_block_reasons


# --- U3:重算审计(recompute.yml) --------------------------------------------


def test_recompute_fixture_audit_trail_reaches_author_ready() -> None:
    # 验收:investigation_attempts 记录新增 E3/E4 后 0.78→0.87,
    # validator 确认重算一致且 author_ready
    result = _validate_fixture("recompute.yml")
    assert result.valid is True
    assert result.author_ready is True
    assert result.recommended_status == "author_ready"
    assert result.next_action == "ready_for_handoff"
    assert "score_inflation" not in _codes(result)
    brief = result.brief
    assert brief is not None
    attempts = brief.investigation_attempts
    assert [a.round for a in attempts] == [1, 2]
    assert [a.candidate_id for a in attempts] == ["C1", "C1"]
    # 审计链:0.50 → 0.78 → 0.87
    assert attempts[0].score_before == pytest.approx(0.50)
    assert attempts[0].score_after == pytest.approx(0.78)
    assert attempts[1].score_before == pytest.approx(0.78)
    assert attempts[1].score_after == pytest.approx(0.87)
    assert brief.candidates[0].confidence == pytest.approx(0.87)
    # 第二轮新增的证据必须存在且为 E3/E4 完成证据
    ledger = {e.id: e.level for e in brief.evidence}
    assert set(attempts[1].added_evidence) <= set(ledger)
    assert {ledger[eid] for eid in attempts[1].added_evidence} == {"E3", "E4"}


def test_recompute_inflated_confidence_is_score_inflation() -> None:
    def mutate(data: dict) -> None:
        # 声明分显著高于最后一轮重算分(0.87)+ 容差 → score_inflation
        data["candidates"][0]["confidence"] = 0.95

    result = validate_brief_text(_mutated("recompute.yml", mutate))
    assert result.valid is False
    assert result.author_ready is False
    assert "score_inflation" in _codes(result)


def test_attempt_score_beyond_evidence_support_is_inflation() -> None:
    # 最小场景:只有一条 E1 级证据(支持度 0.15),attempt 却声称重算到 0.75
    data = {
        "schema_version": "1.0",
        "project_root": "/workspace/demo-repo",
        "status": "needs_investigation",
        "previous_status": "draft",
        "attempt_count": 1,
        "goal": "最小重算审计场景",
        "confidence": {dim: 0.85 for dim in KEY_DIMENSIONS},
        "evidence": [
            {
                "id": "E1",
                "source": "docs/design.md",
                "observation": "只有一条文档级证据",
                "level": "E1",
            }
        ],
        "decisions": [],
        "candidates": [
            {
                "id": "C1",
                "summary": "最小候选",
                "confidence": 0.75,
                "goal_coverage": 0.90,
                "acceptance_coverage": 0.90,
                "project_fit": 0.90,
                "supporting_evidence": ["E1"],
                "selected": True,
            }
        ],
        "investigation_attempts": [
            {
                "round": 1,
                "candidate_id": "C1",
                "added_evidence": ["E1"],
                "score_before": 0.50,
                "score_after": 0.75,
                "provenance": "声称一条 E1 证据可支持 0.75",
            }
        ],
        "user_confirmations": {
            key: {"confirmed": True}
            for key in ("goal", "scope", "completion_evidence", "failure_boundaries")
        },
    }
    result = validate_brief_data(data)
    assert result.author_ready is False
    assert "score_inflation" in _codes(result)


def test_attempt_round_gap_is_invalid() -> None:
    def mutate(data: dict) -> None:
        data["investigation_attempts"][1]["round"] = 3

    result = validate_brief_text(_mutated("recompute.yml", mutate))
    assert result.valid is False
    assert result.author_ready is False
    assert "investigation_attempt_invalid" in _codes(result)


def test_attempt_referencing_unknown_evidence_is_unreferenced() -> None:
    def mutate(data: dict) -> None:
        data["investigation_attempts"][1]["added_evidence"] = ["E99"]

    result = validate_brief_text(_mutated("recompute.yml", mutate))
    assert result.valid is False
    assert "unreferenced_evidence" in _codes(result)
    assert "E99" in result.missing_evidence


def test_attempt_chain_break_is_invalid() -> None:
    def mutate(data: dict) -> None:
        # 第二轮 score_before 与第一轮 score_after(0.78)不一致 → 断链
        data["investigation_attempts"][1]["score_before"] = 0.60

    result = validate_brief_text(_mutated("recompute.yml", mutate))
    assert result.valid is False
    assert "investigation_attempt_invalid" in _codes(result)


# --- U3:三轮耗尽 blocked 的人工输入清单 --------------------------------------


def test_blocked_lists_attempted_candidates_and_required_human_input() -> None:
    # 验收:attempt_count=3 且无达标候选 → blocked,列出已尝试候选与人工所需输入
    result = _validate_fixture("blocked.yml")
    assert result.valid is True
    assert result.author_ready is False
    assert result.recommended_status == "blocked"
    assert result.next_action == "confirm_with_user"
    assert any("C1" in reason for reason in result.handoff_block_reasons)
    assert any("人工输入" in reason for reason in result.handoff_block_reasons)
    # 不得建议第四轮自动调查
    assert all(e.next_action != "rerun_investigation" for e in result.errors)
    assert all(r.next_action != "rerun_investigation" for r in result.rejections)


# --- U3:等分歧义(ambiguous-selected.yml) ------------------------------------


def test_ambiguous_dual_selected_candidates_rejected() -> None:
    # 验收:两候选同分且都达标但都标 selected → 拒绝(歧义)
    result = _validate_fixture("ambiguous-selected.yml")
    assert result.valid is False
    assert result.author_ready is False
    assert "ambiguous_selected_candidates" in _codes(result)
    assert result.recommended_status == "needs_user_decision"
    assert result.next_action == "confirm_with_user"


# --- U3:selected 候选的完成证据(E3/E4)要求 ---------------------------------


def test_selected_candidate_without_completion_evidence_rejected() -> None:
    # 验收:selected 候选无 E3/E4 完成证据 → 拒绝 author-ready
    def mutate(data: dict) -> None:
        data["candidates"][0]["supporting_evidence"] = ["E1"]  # E1 为 E2 级

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is False
    assert result.author_ready is False
    assert "selected_candidate_missing_acceptance_evidence" in _codes(result)


def test_missing_completion_evidence_blocks_certification_even_honest() -> None:
    # 诚实声明 needs_investigation:不产生专属错误,但认证仍被阻断
    def mutate(data: dict) -> None:
        data["candidates"][0]["supporting_evidence"] = ["E1"]
        data["status"] = "needs_investigation"

    result = validate_brief_text(_mutated("valid.yml", mutate))
    assert result.valid is True
    assert result.author_ready is False
    assert result.recommended_status == "needs_investigation"
    assert "selected_candidate_missing_acceptance_evidence" not in _codes(result)


# --- U3:矛盾证据(conflicting-evidence.yml) ----------------------------------


def test_conflicting_e3_evidence_blocks_author_ready() -> None:
    # 验收:同主题两条冲突 E3 → 不 author_ready,建议 needs_user_decision
    result = _validate_fixture("conflicting-evidence.yml")
    assert result.valid is True  # 诚实声明 needs_user_decision
    assert result.author_ready is False
    assert result.recommended_status == "needs_user_decision"
    assert result.next_action == "confirm_with_user"
    assert result.handoff_block_reasons


def test_conflicting_evidence_cannot_be_declared_author_ready() -> None:
    def mutate(data: dict) -> None:
        data["status"] = "author_ready"

    result = validate_brief_text(_mutated("conflicting-evidence.yml", mutate))
    assert result.valid is False
    assert result.author_ready is False
    assert "author_ready_gate_violation" in _codes(result)


def test_conflicting_evidence_requires_full_adjudication() -> None:
    # 裁决决策只引用冲突的一侧 → 矛盾仍未解除,不得 author_ready
    def partial(data: dict) -> None:
        data["decisions"][0]["resolved"] = True
        data["decisions"][0]["uncovered_risks"] = []
        data["decisions"][0]["supporting_evidence"] = ["E1"]

    result = validate_brief_text(_mutated("conflicting-evidence.yml", partial))
    assert result.author_ready is False
    assert result.recommended_status == "needs_user_decision"

    # 裁决决策引用全部冲突事实 → 矛盾视为已裁决,可 author_ready
    def full(data: dict) -> None:
        data["decisions"][0]["resolved"] = True
        data["decisions"][0]["uncovered_risks"] = []
        data["decisions"][0]["supporting_evidence"] = ["E1", "E2"]

    result = validate_brief_text(_mutated("conflicting-evidence.yml", full))
    assert result.valid is True
    assert result.author_ready is True
    assert result.recommended_status == "author_ready"


# --- U3:候选 status 一致性 ----------------------------------------------------


def test_candidate_status_inconsistent_with_gate_outcome() -> None:
    def mutate(data: dict) -> None:
        # C-A 未通过覆盖门禁,声明 status=selected 与门禁结论矛盾
        data["candidates"][0]["status"] = "selected"

    result = validate_brief_text(_mutated("alternative.yml", mutate))
    assert result.valid is False
    assert "candidate_status_inconsistent" in _codes(result)


def test_unknown_candidate_status_is_rejected() -> None:
    def mutate(data: dict) -> None:
        data["candidates"][1]["status"] = "winner"

    result = validate_brief_text(_mutated("alternative.yml", mutate))
    assert result.valid is False
    assert "unknown_status" in _codes(result)


# --- U3:新字段向后兼容 --------------------------------------------------------


def test_new_candidate_fields_are_optional_backward_compat() -> None:
    result = _validate_fixture("valid.yml")
    brief = result.brief
    assert brief is not None
    assert brief.investigation_attempts == ()
    for candidate in brief.candidates:
        assert candidate.risk_coverage is None
        assert candidate.status is None
        assert candidate.rejection_reason is None


# --- U3:rubric 文档与代码常量一致性(SSOT) -----------------------------------

RUBRIC_PATH = (
    Path(__file__).resolve().parents[1]
    / "ralph-task-discovery"
    / "references"
    / "confidence-and-candidate-rubric.md"
)


def _rubric_block() -> dict:
    text = RUBRIC_PATH.read_text(encoding="utf-8")
    match = re.search(
        r"<!-- rubric-yaml:start -->(.*?)<!-- rubric-yaml:end -->", text, re.S
    )
    assert match, "confidence-and-candidate-rubric.md 缺少机器可读 rubric 块"
    lines = [
        line
        for line in match.group(1).splitlines()
        if not line.strip().startswith("```")
    ]
    return yaml.safe_load("\n".join(lines))


def test_rubric_document_constants_match_code() -> None:
    import task_brief

    rubric = _rubric_block()
    assert rubric["author_ready_threshold"] == task_brief.AUTHOR_READY_THRESHOLD
    assert rubric["reject_threshold"] == task_brief.REJECT_THRESHOLD
    assert rubric["attempt_limit"] == task_brief.ATTEMPT_LIMIT
    assert rubric["score_inflation_tolerance"] == task_brief.SCORE_INFLATION_TOLERANCE
    assert rubric["evidence_level_support"] == dict(task_brief.EVIDENCE_LEVEL_SUPPORT)
    assert rubric["evidence_levels"] == list(task_brief.EVIDENCE_LEVELS)
    assert rubric["completion_evidence_levels"] == list(
        task_brief.COMPLETION_EVIDENCE_LEVELS
    )
    assert rubric["key_dimensions"] == list(task_brief.KEY_DIMENSIONS)
    assert rubric["candidate_statuses"] == list(task_brief.CANDIDATE_STATUSES)
    assert rubric["candidate_coverage_gates"] == {
        "goal_coverage": task_brief.CANDIDATE_GOAL_COVERAGE_MIN,
        "acceptance_coverage": task_brief.CANDIDATE_ACCEPTANCE_COVERAGE_MIN,
        "project_fit": task_brief.CANDIDATE_PROJECT_FIT_MIN,
    }
