"""ralph-task-discovery task brief 硬门禁 validator。

输入:YAML 文本(``validate_brief_text``)或已解析的 mapping
(``validate_brief_data``);输出:类型化 ``ValidationResult``。

校验顺序(每步都产生稳定 code 的错误,绝不因总平均分高而绕过单维度门禁):

1. YAML 可解析性与顶层 mapping 结构;
2. provenance:schema_version 与 project_root;
3. status 枚举、goal、attempt_count;
4. 五个关键置信度维度(缺失/非法/分带);
5. Evidence 台账字段、等级与矛盾证据分组;
6. Decision / Candidate 的字段与证据引用完整性;
7. 候选方案独立覆盖门禁(与 confidence 无关)、候选 status 一致性、
   等分歧义(至多一个 selected);
8. investigation_attempts 重算审计(round 连续、证据存在、链式衔接、
   声明分不得超过去重证据支持度 → score_inflation);
9. author_ready 充要条件(含 selected 候选 E3/E4 完成证据、矛盾证据
   未裁决)+ 状态一致性 + 单向转换表。

评分规则 SSOT:references/confidence-and-candidate-rubric.md。
"""
from __future__ import annotations

from typing import Any, Mapping

import yaml

from task_brief import (
    ALLOWED_TRANSITIONS,
    ATTEMPT_LIMIT,
    AUTHOR_READY_THRESHOLD,
    CANDIDATE_ACCEPTANCE_COVERAGE_MIN,
    CANDIDATE_GOAL_COVERAGE_MIN,
    CANDIDATE_PROJECT_FIT_MIN,
    CANDIDATE_STATUSES,
    COMPLETION_EVIDENCE_LEVELS,
    CONFLICTING_EVIDENCE_MARKER,
    EVIDENCE_LEVELS,
    KEY_DIMENSIONS,
    NEXT_CONFIRM_USER,
    NEXT_EMIT_BLOCKED,
    NEXT_HANDOFF,
    NEXT_INVESTIGATE,
    NEXT_SWITCH_CANDIDATE,
    REJECTED_INSUFFICIENT_COVERAGE,
    REJECTED_LOW_CONFIDENCE,
    REJECT_THRESHOLD,
    SCORE_INFLATION_TOLERANCE,
    SUPPORTED_SCHEMA_VERSION,
    USER_CONFIRMATION_KEYS,
    VALID_STATUSES,
    GateResult,
    Rejection,
    TaskBrief,
    ValidationError,
    ValidationResult,
    compute_support,
)

# --- 稳定错误 code(冻结契约) ----------------------------------------------

CODE_UNKNOWN_STATUS = "unknown_status"
CODE_MISSING_REQUIRED_FIELD = "missing_required_field"
CODE_INVALID_SCORE = "invalid_score"
CODE_UNREFERENCED_EVIDENCE = "unreferenced_evidence"
CODE_INVALID_EVIDENCE_LEVEL = "invalid_evidence_level"
CODE_AUTHOR_READY_GATE_VIOLATION = "author_ready_gate_violation"
CODE_CANDIDATE_COVERAGE_GATE_FAILED = "candidate_coverage_gate_failed"
CODE_STATE_TRANSITION_INVALID = "state_transition_invalid"
CODE_SCHEMA_VERSION_INVALID = "schema_version_invalid"
CODE_ROOT_PROVENANCE_MISSING = "root_provenance_missing"
CODE_DUPLICATE_EVIDENCE_ID = "duplicate_evidence_id"
CODE_DUPLICATE_CANDIDATE_ID = "duplicate_candidate_id"
CODE_DUPLICATE_DECISION_ID = "duplicate_decision_id"
CODE_INVALID_YAML = "invalid_yaml"
# U3 新增(向后兼容,均为增量 code):
#: 声明(重算)分数显著超出去重证据可支持的上限(invalid_score 家族)。
CODE_SCORE_INFLATION = "score_inflation"
#: 两个及以上候选同时标 selected 且都达标:选择歧义,需显式选择或用户裁决。
CODE_AMBIGUOUS_SELECTED_CANDIDATES = "ambiguous_selected_candidates"
#: selected 且达标的候选缺少 E3/E4 完成证据(author_ready 被阻断)。
CODE_SELECTED_MISSING_ACCEPTANCE_EVIDENCE = (
    "selected_candidate_missing_acceptance_evidence"
)
#: 声明的候选 status 与门禁结论矛盾。
CODE_CANDIDATE_STATUS_INCONSISTENT = "candidate_status_inconsistent"
#: investigation_attempts 结构违规(round 跳号 / 候选引用缺失 / 链断裂)。
CODE_INVESTIGATION_ATTEMPT_INVALID = "investigation_attempt_invalid"

# 候选方案覆盖门禁定义:(字段名, 最小值)。
_COVERAGE_GATES: tuple[tuple[str, float], ...] = (
    ("goal_coverage", CANDIDATE_GOAL_COVERAGE_MIN),
    ("acceptance_coverage", CANDIDATE_ACCEPTANCE_COVERAGE_MIN),
    ("project_fit", CANDIDATE_PROJECT_FIT_MIN),
)


def _is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _band(score: float) -> str:
    """分带:< 0.70 丢弃带;[0.70, 0.85) 调查带;>= 0.85 达标带。

    边界包含语义:0.70 属于调查带,0.85 属于达标带。
    """
    if score < REJECT_THRESHOLD:
        return "rejected"
    if score < AUTHOR_READY_THRESHOLD:
        return "investigate"
    return "pass"


def validate_brief_text(text: str) -> ValidationResult:
    """从 YAML 文本直接验证(真实 yaml.safe_load,不绕过格式错误)。"""
    try:
        data = yaml.safe_load(text)
    except yaml.YAMLError as exc:
        return ValidationResult(
            valid=False,
            author_ready=False,
            recommended_status="draft",
            next_action=NEXT_INVESTIGATE,
            errors=(
                ValidationError(
                    code=CODE_INVALID_YAML,
                    path="$",
                    message=f"YAML 解析失败,无法读取 task brief:{exc}",
                    next_action=NEXT_INVESTIGATE,
                ),
            ),
            rejections=(),
            candidate_gates=(),
            handoff_block_reasons=("YAML 格式错误,无法解析 task brief",),
            missing_evidence=(),
            brief=None,
        )
    return validate_brief_data(data)


def validate_brief_data(data: Any) -> ValidationResult:
    """从已解析的 mapping 验证(YAML 文本入口也汇聚到这里)。"""
    errors: list[ValidationError] = []
    rejections: list[Rejection] = []
    gates: list[GateResult] = []
    missing_evidence: list[str] = []

    if not isinstance(data, Mapping):
        errors.append(
            ValidationError(
                code=CODE_MISSING_REQUIRED_FIELD,
                path="$",
                message="task brief 顶层必须是 YAML mapping(键值对结构)",
                next_action=NEXT_INVESTIGATE,
            )
        )
        return _finalize(
            errors=errors,
            rejections=rejections,
            gates=gates,
            missing_evidence=missing_evidence,
            brief=None,
            recommended="draft",
            unmet_messages=("task brief 顶层结构无效,无法评估任何门禁",),
        )

    # --- provenance:schema_version / project_root --------------------------
    schema_version = data.get("schema_version")
    schema_ok = schema_version == SUPPORTED_SCHEMA_VERSION
    if not schema_ok:
        if schema_version is None:
            message = f"缺少 schema_version 字段,必须为字符串 \"{SUPPORTED_SCHEMA_VERSION}\""
        else:
            message = (
                f"schema_version '{schema_version}' 不受支持,"
                f"当前唯一支持的版本是 \"{SUPPORTED_SCHEMA_VERSION}\""
            )
        errors.append(
            ValidationError(
                code=CODE_SCHEMA_VERSION_INVALID,
                path="$.schema_version",
                message=message,
                next_action=NEXT_INVESTIGATE,
            )
        )

    project_root = data.get("project_root")
    root_ok = isinstance(project_root, str) and bool(project_root.strip())
    if not root_ok:
        errors.append(
            ValidationError(
                code=CODE_ROOT_PROVENANCE_MISSING,
                path="$.project_root",
                message="缺少 project_root provenance 字段(必须为非空字符串,记录 brief 所属项目根)",
                next_action=NEXT_INVESTIGATE,
            )
        )
    provenance_ok = schema_ok and root_ok

    # --- status / goal / attempt_count --------------------------------------
    declared_status = data.get("status")
    if declared_status is None:
        errors.append(
            ValidationError(
                code=CODE_MISSING_REQUIRED_FIELD,
                path="$.status",
                message=f"缺少 status 字段,允许值:{' / '.join(VALID_STATUSES)}",
                next_action=NEXT_INVESTIGATE,
            )
        )
        declared_status = None
    elif declared_status not in VALID_STATUSES:
        errors.append(
            ValidationError(
                code=CODE_UNKNOWN_STATUS,
                path="$.status",
                message=(
                    f"status 值 '{declared_status}' 不在允许枚举中,"
                    f"允许值:{' / '.join(VALID_STATUSES)}"
                ),
                next_action=NEXT_INVESTIGATE,
            )
        )
        declared_status = None

    previous_status = data.get("previous_status")
    if previous_status is not None and previous_status not in VALID_STATUSES:
        errors.append(
            ValidationError(
                code=CODE_STATE_TRANSITION_INVALID,
                path="$.previous_status",
                message=(
                    f"previous_status 值 '{previous_status}' 不是合法状态,"
                    f"无法验证单向转换规则"
                ),
                next_action=NEXT_CONFIRM_USER,
            )
        )
        previous_status = None

    goal = data.get("goal")
    if not isinstance(goal, str) or not goal.strip():
        errors.append(
            ValidationError(
                code=CODE_MISSING_REQUIRED_FIELD,
                path="$.goal",
                message="缺少 goal 字段(任务目标的一句话描述,非空字符串)",
                next_action=NEXT_INVESTIGATE,
            )
        )

    attempt_count = data.get("attempt_count", 1)
    if isinstance(attempt_count, bool) or not isinstance(attempt_count, int) or attempt_count < 1:
        errors.append(
            ValidationError(
                code=CODE_INVALID_SCORE,
                path="$.attempt_count",
                message=f"attempt_count 必须是 >= 1 的整数,当前为 {attempt_count!r}",
                next_action=NEXT_INVESTIGATE,
            )
        )
        attempt_count = 1

    # --- 五个关键置信度维度 ---------------------------------------------------
    dimension_scores: dict[str, float] = {}
    dimensions_available = True
    confidence_raw = data.get("confidence")
    if not isinstance(confidence_raw, Mapping):
        dimensions_available = False
        errors.append(
            ValidationError(
                code=CODE_MISSING_REQUIRED_FIELD,
                path="$.confidence",
                message="缺少 confidence 段(五个关键置信度维度的 mapping)",
                next_action=NEXT_INVESTIGATE,
            )
        )
    else:
        for dim in KEY_DIMENSIONS:
            path = f"$.confidence.{dim}"
            value = confidence_raw.get(dim)
            if value is None:
                dimensions_available = False
                errors.append(
                    ValidationError(
                        code=CODE_MISSING_REQUIRED_FIELD,
                        path=path,
                        message=f"缺少关键置信度维度 {dim}",
                        next_action=NEXT_INVESTIGATE,
                    )
                )
                continue
            if not _is_number(value) or not (0.0 <= float(value) <= 1.0):
                dimensions_available = False
                errors.append(
                    ValidationError(
                        code=CODE_INVALID_SCORE,
                        path=path,
                        message=f"{dim} 必须是 [0, 1] 区间内的数值,当前为 {value!r}",
                        next_action=NEXT_INVESTIGATE,
                    )
                )
                continue
            dimension_scores[dim] = float(value)

    dims_ok = len(dimension_scores) == len(KEY_DIMENSIONS) and all(
        score >= AUTHOR_READY_THRESHOLD for score in dimension_scores.values()
    )
    for dim in KEY_DIMENSIONS:
        score = dimension_scores.get(dim)
        if score is not None and _band(score) == "rejected":
            rejections.append(
                Rejection(
                    kind="dimension",
                    id=dim,
                    reason=REJECTED_LOW_CONFIDENCE,
                    next_action=NEXT_CONFIRM_USER if attempt_count >= ATTEMPT_LIMIT else NEXT_INVESTIGATE,
                )
            )

    # --- Evidence 台账 --------------------------------------------------------
    ledger_ids: set[str] = set()
    #: 成功登记的证据 id → 等级(支持度计算与完成证据检查用)。
    evidence_levels: dict[str, str] = {}
    #: 矛盾证据分组:主题 → 带 conflicting_evidence: 前缀的 E3/E4 证据 id。
    conflict_groups: dict[str, list[str]] = {}
    evidence_raw = data.get("evidence", [])
    if not isinstance(evidence_raw, list):
        errors.append(
            ValidationError(
                code=CODE_MISSING_REQUIRED_FIELD,
                path="$.evidence",
                message="evidence 必须是证据条目列表",
                next_action=NEXT_INVESTIGATE,
            )
        )
        evidence_raw = []
    for index, entry in enumerate(evidence_raw):
        path = f"$.evidence[{index}]"
        if not isinstance(entry, Mapping):
            errors.append(
                ValidationError(
                    code=CODE_MISSING_REQUIRED_FIELD,
                    path=path,
                    message="每条证据必须是 mapping(含 id/source/observation/level)",
                    next_action=NEXT_INVESTIGATE,
                )
            )
            continue
        entry_id = entry.get("id")
        if not isinstance(entry_id, str) or not entry_id.strip():
            errors.append(
                ValidationError(
                    code=CODE_MISSING_REQUIRED_FIELD,
                    path=f"{path}.id",
                    message="证据缺少 id 字段(如 E1),无法被决策/候选引用",
                    next_action=NEXT_INVESTIGATE,
                )
            )
        elif entry_id in ledger_ids:
            errors.append(
                ValidationError(
                    code=CODE_DUPLICATE_EVIDENCE_ID,
                    path=f"{path}.id",
                    message=f"证据 id '{entry_id}' 重复,引用将产生歧义",
                    next_action=NEXT_INVESTIGATE,
                )
            )
        else:
            ledger_ids.add(entry_id)
        for field_name in ("source", "observation"):
            value = entry.get(field_name)
            if not isinstance(value, str) or not value.strip():
                errors.append(
                    ValidationError(
                        code=CODE_MISSING_REQUIRED_FIELD,
                        path=f"{path}.{field_name}",
                        message=f"证据缺少 {field_name} 字段",
                        next_action=NEXT_INVESTIGATE,
                    )
                )
        level = entry.get("level")
        if level is None:
            errors.append(
                ValidationError(
                    code=CODE_MISSING_REQUIRED_FIELD,
                    path=f"{path}.level",
                    message=f"证据缺少 level 字段,允许值:{' / '.join(EVIDENCE_LEVELS)}",
                    next_action=NEXT_INVESTIGATE,
                )
            )
        elif level not in EVIDENCE_LEVELS:
            errors.append(
                ValidationError(
                    code=CODE_INVALID_EVIDENCE_LEVEL,
                    path=f"{path}.level",
                    message=(
                        f"证据等级 '{level}' 非法,允许值:"
                        f"{' / '.join(EVIDENCE_LEVELS)}(E0 用户陈述 → E4 独立验收)"
                    ),
                    next_action=NEXT_INVESTIGATE,
                )
            )
        # 登记合法证据的等级;矛盾证据(同主题互相矛盾的 E3/E4)按主题分组。
        # 只认首个成功登记的 id(重复 id 已在上面报错,不参与分组)。
        if (
            isinstance(entry_id, str)
            and entry_id in ledger_ids
            and level in EVIDENCE_LEVELS
        ):
            evidence_levels[entry_id] = level
            observation_value = entry.get("observation")
            if (
                level in COMPLETION_EVIDENCE_LEVELS
                and isinstance(observation_value, str)
                and observation_value.startswith(CONFLICTING_EVIDENCE_MARKER)
            ):
                topic = (
                    observation_value[len(CONFLICTING_EVIDENCE_MARKER) :]
                    .split(":", 1)[0]
                    .strip()
                )
                conflict_groups.setdefault(topic, []).append(entry_id)

    # --- 引用完整性(Decision / Candidate 共用) ------------------------------
    def _check_references(
        refs: Any, owner: str, owner_path: str, require_non_empty: bool
    ) -> None:
        if not isinstance(refs, list):
            errors.append(
                ValidationError(
                    code=CODE_MISSING_REQUIRED_FIELD,
                    path=f"{owner_path}.supporting_evidence",
                    message=f"{owner} 的 supporting_evidence 必须是证据 id 字符串列表",
                    next_action=NEXT_INVESTIGATE,
                )
            )
            return
        string_refs = [ref for ref in refs if isinstance(ref, str)]
        if require_non_empty and not string_refs:
            errors.append(
                ValidationError(
                    code=CODE_UNREFERENCED_EVIDENCE,
                    path=f"{owner_path}.supporting_evidence",
                    message=f"{owner} 没有引用任何证据,结论必须建立在证据台账之上",
                    next_action=NEXT_INVESTIGATE,
                )
            )
            return
        for ref in string_refs:
            if ref not in ledger_ids:
                errors.append(
                    ValidationError(
                        code=CODE_UNREFERENCED_EVIDENCE,
                        path=f"{owner_path}.supporting_evidence",
                        message=f"{owner} 引用的证据 '{ref}' 不存在于 evidence 台账",
                        next_action=NEXT_INVESTIGATE,
                    )
                )
                if ref not in missing_evidence:
                    missing_evidence.append(ref)

    # --- Decision Records -----------------------------------------------------
    decision_flags = {"resolved_blocking_pending": False, "blocking_below_threshold": False}
    #: 已裁决的 blocking 决策引用的证据集合。矛盾证据的用户裁决必须是一个
    #: resolved blocking 决策,同时引用该主题的全部冲突证据 id。
    adjudication_refs: list[set[str]] = []
    decisions_raw = data.get("decisions", [])
    if not isinstance(decisions_raw, list):
        errors.append(
            ValidationError(
                code=CODE_MISSING_REQUIRED_FIELD,
                path="$.decisions",
                message="decisions 必须是决策记录列表",
                next_action=NEXT_INVESTIGATE,
            )
        )
        decisions_raw = []
    #: 已登记的决策 id(唯一性门禁;缺失 id 回退 #<index>,按 index 天然唯一)。
    decision_ids: set[str] = set()
    for index, entry in enumerate(decisions_raw):
        path = f"$.decisions[{index}]"
        if not isinstance(entry, Mapping):
            errors.append(
                ValidationError(
                    code=CODE_MISSING_REQUIRED_FIELD,
                    path=path,
                    message="每条决策记录必须是 mapping",
                    next_action=NEXT_INVESTIGATE,
                )
            )
            continue
        entry_id = entry.get("id") if isinstance(entry.get("id"), str) and entry.get("id") else f"#{index}"
        if str(entry_id) in decision_ids:
            errors.append(
                ValidationError(
                    code=CODE_DUPLICATE_DECISION_ID,
                    path=f"{path}.id",
                    message=f"决策 id '{entry_id}' 重复,引用将产生歧义",
                    next_action=NEXT_INVESTIGATE,
                )
            )
        else:
            decision_ids.add(str(entry_id))
        label = f"决策 {entry_id}"
        question = entry.get("question")
        if not isinstance(question, str) or not question.strip():
            errors.append(
                ValidationError(
                    code=CODE_MISSING_REQUIRED_FIELD,
                    path=f"{path}.question",
                    message=f"{label} 缺少 question 字段",
                    next_action=NEXT_INVESTIGATE,
                )
            )
        confidence = entry.get("confidence")
        confidence_value: float | None = None
        if confidence is None:
            errors.append(
                ValidationError(
                    code=CODE_MISSING_REQUIRED_FIELD,
                    path=f"{path}.confidence",
                    message=f"{label} 缺少 confidence 字段",
                    next_action=NEXT_INVESTIGATE,
                )
            )
        elif not _is_number(confidence) or not (0.0 <= float(confidence) <= 1.0):
            errors.append(
                ValidationError(
                    code=CODE_INVALID_SCORE,
                    path=f"{path}.confidence",
                    message=f"{label} 的 confidence 必须是 [0, 1] 区间内的数值,当前为 {confidence!r}",
                    next_action=NEXT_INVESTIGATE,
                )
            )
        else:
            confidence_value = float(confidence)
        blocking = entry.get("blocking")
        if not isinstance(blocking, bool):
            errors.append(
                ValidationError(
                    code=CODE_MISSING_REQUIRED_FIELD,
                    path=f"{path}.blocking",
                    message=f"{label} 缺少布尔 blocking 字段(是否为 author-blocking 决策)",
                    next_action=NEXT_INVESTIGATE,
                )
            )
            blocking = False
        resolved = entry.get("resolved")
        if not isinstance(resolved, bool):
            errors.append(
                ValidationError(
                    code=CODE_MISSING_REQUIRED_FIELD,
                    path=f"{path}.resolved",
                    message=f"{label} 缺少布尔 resolved 字段",
                    next_action=NEXT_INVESTIGATE,
                )
            )
            resolved = False
        _check_references(entry.get("supporting_evidence"), label, path, require_non_empty=True)

        if confidence_value is not None:
            if _band(confidence_value) == "rejected":
                rejections.append(
                    Rejection(
                        kind="decision",
                        id=str(entry_id),
                        reason=REJECTED_LOW_CONFIDENCE,
                        next_action=(
                            NEXT_CONFIRM_USER if attempt_count >= ATTEMPT_LIMIT else NEXT_INVESTIGATE
                        ),
                    )
                )
            if blocking:
                if confidence_value < AUTHOR_READY_THRESHOLD:
                    decision_flags["blocking_below_threshold"] = True
                if (
                    resolved
                    and confidence_value >= AUTHOR_READY_THRESHOLD
                    and "uncovered_risks" not in entry
                ):
                    errors.append(
                        ValidationError(
                            code=CODE_MISSING_REQUIRED_FIELD,
                            path=f"{path}.uncovered_risks",
                            message=(
                                f"{label} 置信度 >= {AUTHOR_READY_THRESHOLD} 且为 blocking 正式决策,"
                                "必须显式列出 uncovered_risks(可为空列表)"
                            ),
                            next_action=NEXT_INVESTIGATE,
                        )
                    )
        if blocking and not resolved:
            decision_flags["resolved_blocking_pending"] = True
        if blocking and resolved:
            refs_raw = entry.get("supporting_evidence")
            adjudication_refs.append(
                {ref for ref in refs_raw if isinstance(ref, str)}
                if isinstance(refs_raw, list)
                else set()
            )

    decisions_ok = not (
        decision_flags["resolved_blocking_pending"] or decision_flags["blocking_below_threshold"]
    )

    # --- Candidates 与独立覆盖门禁 --------------------------------------------
    candidate_ok = False
    coverage_rejected_count = 0
    #: 通过 confidence 分带与三门禁且被标 selected 的候选记录
    #: (等分歧义 / 完成证据 / 支持度审计用)。
    selected_qualified: list[dict[str, Any]] = []
    #: 候选 id → 记录(重算审计用)。
    candidate_records: dict[str, dict[str, Any]] = {}
    #: 已登记的候选 id(唯一性门禁;缺失 id 回退 #<index>,按 index 天然唯一)。
    candidate_ids: set[str] = set()
    candidates_raw = data.get("candidates", [])
    if not isinstance(candidates_raw, list):
        errors.append(
            ValidationError(
                code=CODE_MISSING_REQUIRED_FIELD,
                path="$.candidates",
                message="candidates 必须是候选方案列表",
                next_action=NEXT_INVESTIGATE,
            )
        )
        candidates_raw = []
    for index, entry in enumerate(candidates_raw):
        path = f"$.candidates[{index}]"
        if not isinstance(entry, Mapping):
            errors.append(
                ValidationError(
                    code=CODE_MISSING_REQUIRED_FIELD,
                    path=path,
                    message="每个候选方案必须是 mapping",
                    next_action=NEXT_INVESTIGATE,
                )
            )
            continue
        entry_id = entry.get("id") if isinstance(entry.get("id"), str) and entry.get("id") else f"#{index}"
        if str(entry_id) in candidate_ids:
            errors.append(
                ValidationError(
                    code=CODE_DUPLICATE_CANDIDATE_ID,
                    path=f"{path}.id",
                    message=f"候选 id '{entry_id}' 重复,引用将产生歧义",
                    next_action=NEXT_INVESTIGATE,
                )
            )
        else:
            candidate_ids.add(str(entry_id))
        label = f"候选 {entry_id}"
        summary = entry.get("summary")
        if not isinstance(summary, str) or not summary.strip():
            errors.append(
                ValidationError(
                    code=CODE_MISSING_REQUIRED_FIELD,
                    path=f"{path}.summary",
                    message=f"{label} 缺少 summary 字段",
                    next_action=NEXT_INVESTIGATE,
                )
            )
        scores: dict[str, float | None] = {}
        for field_name in ("confidence", "goal_coverage", "acceptance_coverage", "project_fit"):
            value = entry.get(field_name)
            if value is None:
                errors.append(
                    ValidationError(
                        code=CODE_MISSING_REQUIRED_FIELD,
                        path=f"{path}.{field_name}",
                        message=f"{label} 缺少 {field_name} 字段",
                        next_action=NEXT_INVESTIGATE,
                    )
                )
                scores[field_name] = None
            elif not _is_number(value) or not (0.0 <= float(value) <= 1.0):
                errors.append(
                    ValidationError(
                        code=CODE_INVALID_SCORE,
                        path=f"{path}.{field_name}",
                        message=f"{label} 的 {field_name} 必须是 [0, 1] 区间内的数值,当前为 {value!r}",
                        next_action=NEXT_INVESTIGATE,
                    )
                )
                scores[field_name] = None
            else:
                scores[field_name] = float(value)
        # 新增可选字段(向后兼容)。risk_coverage 仅展示/追踪,不是第四个覆盖门禁。
        risk_coverage_value = entry.get("risk_coverage")
        if risk_coverage_value is not None and (
            not _is_number(risk_coverage_value)
            or not (0.0 <= float(risk_coverage_value) <= 1.0)
        ):
            errors.append(
                ValidationError(
                    code=CODE_INVALID_SCORE,
                    path=f"{path}.risk_coverage",
                    message=(
                        f"{label} 的 risk_coverage 必须是 [0, 1] 区间内的数值"
                        f"(仅展示/追踪字段),当前为 {risk_coverage_value!r}"
                    ),
                    next_action=NEXT_INVESTIGATE,
                )
            )
        declared_candidate_status = entry.get("status")
        if declared_candidate_status is not None:
            if declared_candidate_status not in CANDIDATE_STATUSES:
                errors.append(
                    ValidationError(
                        code=CODE_UNKNOWN_STATUS,
                        path=f"{path}.status",
                        message=(
                            f"{label} 的 status 值 '{declared_candidate_status}' 不在允许枚举中,"
                            f"允许值:{' / '.join(CANDIDATE_STATUSES)}"
                        ),
                        next_action=NEXT_INVESTIGATE,
                    )
                )
                declared_candidate_status = None
        rejection_reason_value = entry.get("rejection_reason")
        if rejection_reason_value is not None and not isinstance(rejection_reason_value, str):
            errors.append(
                ValidationError(
                    code=CODE_MISSING_REQUIRED_FIELD,
                    path=f"{path}.rejection_reason",
                    message=f"{label} 的 rejection_reason 必须是字符串",
                    next_action=NEXT_INVESTIGATE,
                )
            )
        selected = entry.get("selected")
        if not isinstance(selected, bool):
            errors.append(
                ValidationError(
                    code=CODE_MISSING_REQUIRED_FIELD,
                    path=f"{path}.selected",
                    message=f"{label} 缺少布尔 selected 字段",
                    next_action=NEXT_INVESTIGATE,
                )
            )
            selected = False
        refs_raw = entry.get("supporting_evidence")
        _check_references(refs_raw, label, path, require_non_empty=True)
        ref_ids = (
            [ref for ref in refs_raw if isinstance(ref, str)]
            if isinstance(refs_raw, list)
            else []
        )

        confidence_value = scores["confidence"]
        failed: list[str] = []
        if confidence_value is not None and _band(confidence_value) == "rejected":
            rejections.append(
                Rejection(
                    kind="candidate",
                    id=str(entry_id),
                    reason=REJECTED_LOW_CONFIDENCE,
                    next_action=(
                        NEXT_CONFIRM_USER if attempt_count >= ATTEMPT_LIMIT else NEXT_INVESTIGATE
                    ),
                )
            )
            outcome = REJECTED_LOW_CONFIDENCE
            failed = ["confidence"]
        else:
            for field_name, minimum in _COVERAGE_GATES:
                value = scores[field_name]
                if value is None or value < minimum:
                    failed.append(field_name)
            if failed:
                coverage_rejected_count += 1
                if selected:
                    rendered = ", ".join(
                        f"{name}({scores[name] if scores[name] is not None else '缺失'} < {minimum})"
                        for name, minimum in _COVERAGE_GATES
                        if name in failed
                    )
                    errors.append(
                        ValidationError(
                            code=CODE_CANDIDATE_COVERAGE_GATE_FAILED,
                            path=path,
                            message=(
                                f"{label} 被标 selected 但未通过独立覆盖门禁:{rendered}。"
                                "覆盖门禁与 confidence 无关,confidence 再高也不能绕过"
                            ),
                            next_action=NEXT_SWITCH_CANDIDATE,
                        )
                    )
                outcome = REJECTED_INSUFFICIENT_COVERAGE
            elif confidence_value is not None and confidence_value >= AUTHOR_READY_THRESHOLD:
                outcome = "selected" if selected else "viable"
                if selected:
                    candidate_ok = True
            else:
                outcome = "needs_investigation"

        gates.append(
            GateResult(
                candidate_id=str(entry_id), outcome=outcome, failed_gates=tuple(failed)
            )
        )

        # 声明 status 与门禁结论的一致性审计(pending 不做断言)。
        if declared_candidate_status is not None and declared_candidate_status != "pending":
            if outcome != declared_candidate_status:
                errors.append(
                    ValidationError(
                        code=CODE_CANDIDATE_STATUS_INCONSISTENT,
                        path=f"{path}.status",
                        message=(
                            f"{label} 声明 status '{declared_candidate_status}' 但门禁结论为 "
                            f"'{outcome}',二者矛盾;status 必须与门禁结论一致"
                        ),
                        next_action=NEXT_INVESTIGATE,
                    )
                )

        record = {
            "id": str(entry_id),
            "path": path,
            "label": label,
            "confidence": confidence_value,
            "ref_ids": ref_ids,
            "outcome": outcome,
        }
        candidate_records[str(entry_id)] = record
        if outcome == "selected":
            selected_qualified.append(record)

    # --- selected 候选完成证据(E3/E4)与选择歧义 ------------------------------
    selected_without_completion: list[dict[str, Any]] = []
    if candidate_ok:
        for record in selected_qualified:
            deduped = list(dict.fromkeys(record["ref_ids"]))
            if not any(
                evidence_levels.get(ref) in COMPLETION_EVIDENCE_LEVELS for ref in deduped
            ):
                selected_without_completion.append(record)
    completion_ok = not selected_without_completion

    ambiguous_selected = len(selected_qualified) >= 2
    if ambiguous_selected:
        ids = ", ".join(record["id"] for record in selected_qualified)
        errors.append(
            ValidationError(
                code=CODE_AMBIGUOUS_SELECTED_CANDIDATES,
                path="$.candidates",
                message=(
                    f"{len(selected_qualified)} 个候选({ids})同时标 selected 且都通过全部硬门禁:"
                    "选择歧义。必须显式选择其一,或标记 needs_user_decision 交由用户裁决"
                ),
                next_action=NEXT_CONFIRM_USER,
            )
        )

    # --- 调查/重算尝试记录(可审计) ------------------------------------------
    attempts_by_candidate: dict[str, list[dict[str, Any]]] = {}
    attempts_raw = data.get("investigation_attempts")
    if attempts_raw is not None and not isinstance(attempts_raw, list):
        errors.append(
            ValidationError(
                code=CODE_MISSING_REQUIRED_FIELD,
                path="$.investigation_attempts",
                message="investigation_attempts 必须是调查尝试记录列表",
                next_action=NEXT_INVESTIGATE,
            )
        )
        attempts_raw = []
    for index, attempt_entry in enumerate(attempts_raw or []):
        path = f"$.investigation_attempts[{index}]"
        if not isinstance(attempt_entry, Mapping):
            errors.append(
                ValidationError(
                    code=CODE_MISSING_REQUIRED_FIELD,
                    path=path,
                    message="每条调查尝试记录必须是 mapping",
                    next_action=NEXT_INVESTIGATE,
                )
            )
            continue
        round_value = attempt_entry.get("round")
        if (
            isinstance(round_value, bool)
            or not isinstance(round_value, int)
            or round_value != index + 1
        ):
            errors.append(
                ValidationError(
                    code=CODE_INVESTIGATION_ATTEMPT_INVALID,
                    path=f"{path}.round",
                    message=(
                        f"调查尝试 round 序号必须从 1 开始连续递增,第 {index} 位应为 "
                        f"{index + 1},当前为 {round_value!r}"
                    ),
                    next_action=NEXT_INVESTIGATE,
                )
            )
        candidate_ref = attempt_entry.get("candidate_id")
        attempt_record: dict[str, Any] | None = None
        if not isinstance(candidate_ref, str) or not candidate_ref.strip():
            errors.append(
                ValidationError(
                    code=CODE_INVESTIGATION_ATTEMPT_INVALID,
                    path=f"{path}.candidate_id",
                    message="调查尝试记录缺少 candidate_id(被重算的候选)",
                    next_action=NEXT_INVESTIGATE,
                )
            )
        elif candidate_ref not in candidate_records:
            errors.append(
                ValidationError(
                    code=CODE_INVESTIGATION_ATTEMPT_INVALID,
                    path=f"{path}.candidate_id",
                    message=f"调查尝试记录引用的候选 '{candidate_ref}' 不存在于 candidates 列表",
                    next_action=NEXT_INVESTIGATE,
                )
            )
        else:
            attempt_record = {"path": path}
            attempts_by_candidate.setdefault(candidate_ref, []).append(attempt_record)
        added = attempt_entry.get("added_evidence")
        if not isinstance(added, list):
            errors.append(
                ValidationError(
                    code=CODE_MISSING_REQUIRED_FIELD,
                    path=f"{path}.added_evidence",
                    message="调查尝试记录缺少 added_evidence(本轮新增证据 id 列表)",
                    next_action=NEXT_INVESTIGATE,
                )
            )
        else:
            for ref in added:
                if not isinstance(ref, str) or ref not in ledger_ids:
                    errors.append(
                        ValidationError(
                            code=CODE_UNREFERENCED_EVIDENCE,
                            path=f"{path}.added_evidence",
                            message=f"调查尝试记录新增的证据 '{ref}' 不存在于证据台账",
                            next_action=NEXT_INVESTIGATE,
                        )
                    )
                    if isinstance(ref, str) and ref not in missing_evidence:
                        missing_evidence.append(ref)
        for score_field in ("score_before", "score_after"):
            value = attempt_entry.get(score_field)
            if value is None:
                errors.append(
                    ValidationError(
                        code=CODE_MISSING_REQUIRED_FIELD,
                        path=f"{path}.{score_field}",
                        message=f"调查尝试记录缺少 {score_field}(重算前/后分数)",
                        next_action=NEXT_INVESTIGATE,
                    )
                )
            elif not _is_number(value) or not (0.0 <= float(value) <= 1.0):
                errors.append(
                    ValidationError(
                        code=CODE_INVALID_SCORE,
                        path=f"{path}.{score_field}",
                        message=f"调查尝试记录的 {score_field} 必须是 [0, 1] 区间内的数值,当前为 {value!r}",
                        next_action=NEXT_INVESTIGATE,
                    )
                )
            elif attempt_record is not None:
                attempt_record[score_field] = float(value)

    # 重算审计:链式衔接 + 声明分不得超过去重证据可支持的上限。
    for candidate_id, attempts in attempts_by_candidate.items():
        record = candidate_records[candidate_id]
        support = compute_support(record["ref_ids"], evidence_levels)
        previous_after: float | None = None
        for attempt in attempts:
            after = attempt.get("score_after")
            before = attempt.get("score_before")
            if (
                previous_after is not None
                and before is not None
                and abs(before - previous_after) > 1e-9
            ):
                errors.append(
                    ValidationError(
                        code=CODE_INVESTIGATION_ATTEMPT_INVALID,
                        path=attempt["path"],
                        message=(
                            f"候选 {candidate_id} 的重算审计链断裂:本轮 score_before({before})"
                            f"与上一轮 score_after({previous_after})不一致"
                        ),
                        next_action=NEXT_INVESTIGATE,
                    )
                )
            if after is not None:
                if after > support + SCORE_INFLATION_TOLERANCE:
                    errors.append(
                        ValidationError(
                            code=CODE_SCORE_INFLATION,
                            path=f"{attempt['path']}.score_after",
                            message=(
                                f"候选 {candidate_id} 的重算分数 {after} 超出去重证据可支持的"
                                f"上限 {support:.2f}(容差 {SCORE_INFLATION_TOLERANCE}),"
                                "声明分必须可回溯到证据"
                            ),
                            next_action=NEXT_INVESTIGATE,
                        )
                    )
                previous_after = after
        confidence_value = record["confidence"]
        last_after = attempts[-1].get("score_after")
        if (
            confidence_value is not None
            and last_after is not None
            and confidence_value > last_after + SCORE_INFLATION_TOLERANCE
        ):
            errors.append(
                ValidationError(
                    code=CODE_SCORE_INFLATION,
                    path=f"{record['path']}.confidence",
                    message=(
                        f"候选 {candidate_id} 声明 confidence {confidence_value} 显著高于最后一轮"
                        f"重算分 {last_after}(容差 {SCORE_INFLATION_TOLERANCE})"
                    ),
                    next_action=NEXT_INVESTIGATE,
                )
            )
        if (
            confidence_value is not None
            and confidence_value > support + SCORE_INFLATION_TOLERANCE
        ):
            errors.append(
                ValidationError(
                    code=CODE_SCORE_INFLATION,
                    path=f"{record['path']}.confidence",
                    message=(
                        f"候选 {candidate_id} 声明 confidence {confidence_value} 超出去重证据"
                        f"可支持的上限 {support:.2f}(容差 {SCORE_INFLATION_TOLERANCE})"
                    ),
                    next_action=NEXT_INVESTIGATE,
                )
            )

    # --- 矛盾证据(同主题互相矛盾的 E3/E4) -----------------------------------
    conflicting_topics = {
        topic: ids for topic, ids in conflict_groups.items() if len(ids) >= 2
    }
    conflict_ok = True
    for ids in conflicting_topics.values():
        required = set(ids)
        # 唯一解除路径:resolved blocking 决策同时引用该主题全部冲突证据。
        if not any(required <= refs for refs in adjudication_refs):
            conflict_ok = False

    # --- 用户确认记录 ----------------------------------------------------------
    confirmations_ok = False
    confirmations_raw = data.get("user_confirmations")
    if isinstance(confirmations_raw, Mapping):
        confirmations_ok = True
        for key in USER_CONFIRMATION_KEYS:
            entry = confirmations_raw.get(key)
            if not isinstance(entry, Mapping) or entry.get("confirmed") is not True:
                confirmations_ok = False

    # --- author_ready 充要条件 -------------------------------------------------
    conditions: list[tuple[str, bool, str, str, str]] = [
        # (条件名, 是否满足, path, message, next_action)
        (
            "dimensions",
            dims_ok,
            "$.confidence",
            f"五个关键置信度维度必须全部 >= {AUTHOR_READY_THRESHOLD}(逐个判定,禁止用平均值)",
            NEXT_INVESTIGATE,
        ),
        (
            "candidate",
            candidate_ok,
            "$.candidates",
            "至少需要一个候选方案通过全部硬门禁并被标 selected",
            NEXT_SWITCH_CANDIDATE if coverage_rejected_count else NEXT_INVESTIGATE,
        ),
        (
            "user_confirmations",
            confirmations_ok,
            "$.user_confirmations",
            "缺少用户对 goal / scope / 完成证据 / 关键失败边界的确认记录",
            NEXT_CONFIRM_USER,
        ),
        (
            "decisions",
            decisions_ok,
            "$.decisions",
            "存在未决的 author-blocking 决策,或 blocking 决策置信度不足 0.85",
            NEXT_CONFIRM_USER,
        ),
        (
            "provenance",
            provenance_ok,
            "$",
            "schema_version 或 project_root provenance 字段无效",
            NEXT_INVESTIGATE,
        ),
        (
            "acceptance_evidence_level",
            completion_ok,
            "$.candidates",
            (
                "被标 selected 且达标的候选必须至少引用一条 E3/E4 完成证据"
                "(实际执行/独立验收级别)"
            ),
            NEXT_INVESTIGATE,
        ),
        (
            "conflicting_evidence",
            conflict_ok,
            "$.evidence",
            (
                "同一主题存在互相矛盾的 E3/E4 证据,需用户裁决"
                "(resolved blocking 决策引用全部冲突证据);不得用新证据静默覆盖旧事实"
            ),
            NEXT_CONFIRM_USER,
        ),
    ]
    unmet = [condition for condition in conditions if not condition[1]]

    if declared_status == "author_ready":
        for record in selected_without_completion:
            errors.append(
                ValidationError(
                    code=CODE_SELECTED_MISSING_ACCEPTANCE_EVIDENCE,
                    path=record["path"],
                    message=(
                        f"{record['label']} 被标 selected 且达标,但 supporting_evidence 中没有"
                        "任何 E3/E4 完成证据;交接前必须补真实执行/独立验收证据"
                    ),
                    next_action=NEXT_INVESTIGATE,
                )
            )

    if declared_status == "author_ready" and (unmet or errors):
        for _, _, path, message, next_action in unmet:
            errors.append(
                ValidationError(
                    code=CODE_AUTHOR_READY_GATE_VIOLATION,
                    path=path,
                    message=f"声明 author_ready 但门禁未满足:{message}",
                    next_action=next_action,
                )
            )
        if errors and not unmet:
            errors.append(
                ValidationError(
                    code=CODE_AUTHOR_READY_GATE_VIOLATION,
                    path="$",
                    message=(
                        f"声明 author_ready 但 brief 存在 {len(errors)} 个结构性/完整性错误,"
                        "禁止 handoff"
                    ),
                    next_action=NEXT_INVESTIGATE,
                )
            )

    # --- 状态推导与一致性 -------------------------------------------------------
    hard_ok = dims_ok and candidate_ok and provenance_ok
    all_conditions_met = not unmet
    failing = bool(errors) or not all_conditions_met
    if not dimensions_available:
        recommended = "draft"
    elif attempt_count >= ATTEMPT_LIMIT and failing:
        recommended = "blocked"
    elif not errors and all_conditions_met:
        recommended = "author_ready"
    elif ambiguous_selected:
        recommended = "needs_user_decision"
    elif not conflict_ok:
        recommended = "needs_user_decision"
    elif hard_ok and (not confirmations_ok or not decisions_ok):
        recommended = "needs_user_decision"
    else:
        recommended = "needs_investigation"

    unmet_messages = tuple(message for _, ok, _, message, _ in conditions if not ok)

    if recommended == "blocked":
        # blocked 的 handoff_block_reasons 必须列出已尝试候选与所需人工输入,
        # 且不得建议第四轮自动调查。
        attempted = "; ".join(f"{gate.candidate_id}={gate.outcome}" for gate in gates)
        attempted = attempted or "无候选通过硬门禁"
        unmet_messages = unmet_messages + (
            f"已达 {ATTEMPT_LIMIT} 轮调查上限,已尝试候选:{attempted}",
            (
                "需要人工输入:确认目标/完成标准、提供新证据线索或显式更换候选;"
                "不得自动开启第四轮调查"
            ),
        )

    if recommended == "blocked" and declared_status is not None and declared_status != "blocked":
        errors.append(
            ValidationError(
                code=CODE_STATE_TRANSITION_INVALID,
                path="$.status",
                message=(
                    f"attempt_count >= {ATTEMPT_LIMIT} 且仍不达标,状态必须声明 blocked,"
                    f"当前声明 '{declared_status}'。validator 不会建议第四轮自动调查"
                ),
                next_action=NEXT_EMIT_BLOCKED,
            )
        )

    if (
        previous_status is not None
        and declared_status is not None
        and declared_status not in ALLOWED_TRANSITIONS[previous_status]
    ):
        errors.append(
            ValidationError(
                code=CODE_STATE_TRANSITION_INVALID,
                path="$.status",
                message=(
                    f"状态转换 {previous_status} → {declared_status} 不在允许的单向转换表中"
                    "(blocked 之后只能 needs_user_decision 或保持 blocked;"
                    "draft 不能直接跳到 author_ready)"
                ),
                next_action=NEXT_CONFIRM_USER,
            )
        )

    return _finalize(
        errors=errors,
        rejections=rejections,
        gates=gates,
        missing_evidence=missing_evidence,
        brief=TaskBrief.from_mapping(data),
        recommended=recommended,
        unmet_messages=unmet_messages,
    )


def _finalize(
    *,
    errors: list[ValidationError],
    rejections: list[Rejection],
    gates: list[GateResult],
    missing_evidence: list[str],
    brief: TaskBrief | None,
    recommended: str,
    unmet_messages: tuple[str, ...],
) -> ValidationResult:
    """汇总错误/丢弃/门禁结果,推导整体 next_action 与禁止 handoff 原因。"""
    if errors:
        unmet_messages = tuple(unmet_messages) + (
            f"brief 存在 {len(errors)} 个结构性/完整性错误",
        )

    certified = recommended == "author_ready" and not errors
    if certified:
        next_action = NEXT_HANDOFF
    elif recommended in ("blocked", "needs_user_decision"):
        next_action = NEXT_CONFIRM_USER
    elif any(
        gate.outcome == REJECTED_INSUFFICIENT_COVERAGE for gate in gates
    ) and not any(gate.outcome == "selected" for gate in gates):
        next_action = NEXT_SWITCH_CANDIDATE
    else:
        next_action = NEXT_INVESTIGATE

    return ValidationResult(
        valid=not errors,
        author_ready=certified,
        recommended_status=recommended,
        next_action=next_action,
        errors=tuple(errors),
        rejections=tuple(rejections),
        candidate_gates=tuple(gates),
        handoff_block_reasons=() if certified else unmet_messages,
        missing_evidence=tuple(missing_evidence),
        brief=brief,
    )
