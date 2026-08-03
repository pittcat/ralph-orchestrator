"""ralph-task-discovery task brief 数据契约 + 硬门禁 validator 的 contract 测试(U1)。

被测契约(frozen):

* 五个关键置信度维度各自独立过 >= 0.85 硬门禁,禁止用平均值绕过;
* 候选方案除 confidence 外还有独立覆盖门禁(goal_coverage >= 0.80、
  acceptance_coverage >= 0.85、project_fit >= 0.75);
* Evidence 台账引用完整性;Decision / Candidate 引用的证据必须存在;
* author_ready 的充要条件全部满足才允许 handoff;
* attempt_count >= 3 且不达标 → blocked,不得建议第四轮自动调查;
* 每个错误带稳定 code、JSON-path 位置、message 与 next_action。

所有 fixture 都走 YAML 文本入口(``validate_brief_text``)验证,
确保真实经过 ``yaml.safe_load``,不被 Python dict 构造绕过格式检查。
"""
from __future__ import annotations

import json
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
