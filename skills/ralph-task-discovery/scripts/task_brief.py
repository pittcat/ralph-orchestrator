"""ralph-task-discovery task brief 数据契约(纯数据结构)。

本模块只定义 task brief(任务简报)的类型化数据模型与冻结契约常量:
TaskBrief、Evidence、DecisionRecord、Candidate、InvestigationAttempt、
GateResult、ValidationError、Rejection、ValidationResult。

术语约定(首次出现即解释):

* task brief(任务简报):进入正式计划/Unit 之前,对一次任务的结构化
  描述,包括目标、证据、决策记录、候选方案与置信度评估。
* Evidence(证据):支撑决策与候选方案的事实条目,带证据等级(E0-E4)。
* Decision Record(决策记录):一个"问题 → 结论"的显式决策,带置信度
  与支撑证据引用。
* Candidate(候选方案):完成任务的一种可行方案,带独立的覆盖度评估。
* InvestigationAttempt(调查/重算尝试):低置信度候选补证据重算时的
  可审计记录("第 N 轮新增哪些证据 → 分数从 X 重算到 Y")。
* author_ready:brief 的一种状态,表示可以把任务交接(handoff)给计划
  作者去写正式计划。

本模块不做任何 I/O、不访问文件系统、不依赖 Ralph runtime;所有门禁
判定逻辑都在 ``brief_validator.py`` 中。
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any, Iterable, Mapping

# --- 冻结契约常量 -----------------------------------------------------------

SUPPORTED_SCHEMA_VERSION = "1.0"

#: brief.status 允许的全部状态(全部小写)。
VALID_STATUSES: tuple[str, ...] = (
    "draft",
    "needs_investigation",
    "needs_user_decision",
    "blocked",
    "author_ready",
)

#: 单向状态转换表:previous_status -> 允许进入的 status 集合。
#: 语义:draft 不能直接跳到 author_ready;blocked 之后只能等待人工输入
#: (needs_user_decision)或保持 blocked,不得自动重新调查。
ALLOWED_TRANSITIONS: Mapping[str, frozenset[str]] = {
    "draft": frozenset({"needs_investigation", "needs_user_decision", "blocked"}),
    "needs_investigation": frozenset(
        {"needs_investigation", "needs_user_decision", "blocked", "author_ready"}
    ),
    "needs_user_decision": frozenset(
        {"needs_user_decision", "needs_investigation", "blocked", "author_ready"}
    ),
    "blocked": frozenset({"blocked", "needs_user_decision"}),
    "author_ready": frozenset({"author_ready", "blocked"}),
}

#: 五个关键置信度维度。每个维度独立过硬门禁,禁止用平均值代替。
KEY_DIMENSIONS: tuple[str, ...] = (
    "goal_clarity",  # 目标/范围/非目标/用户决策已确认
    "project_fact_coverage",  # 入口、调用链、现有模式、验证命令、影响面有证据
    "acceptance_evidence",  # 每个重要结果有可执行或可观察的完成证据
    "execution_feasibility",  # 至少一个候选方案能在项目能力与约束下执行
    "risk_coverage",  # 关键失败、兼容、权限、外部依赖、恢复风险已处理
)

#: 证据等级:E0 用户陈述/未验证直觉;E1 项目文档/配置/规则文件;
#: E2 源码/类型/调用链/测试入口;E3 实际执行的构建/测试/CLI/HTTP/replay 结果;
#: E4 独立验收场景/真实用户路径/可复现回归证据。
EVIDENCE_LEVELS: tuple[str, ...] = ("E0", "E1", "E2", "E3", "E4")

#: >= 0.85 达标带:可作为正式决策(仍须列出证据与未覆盖风险)。
AUTHOR_READY_THRESHOLD = 0.85
#: < 0.70 丢弃带(rejected_low_confidence);[0.70, 0.85) 为调查带,
#: 必须产生新证据后重算。两个边界都包含在"较松"的一侧:
#: 0.70 属于调查带,0.85 属于达标带。
REJECT_THRESHOLD = 0.70

#: 候选方案独立覆盖门禁(与 confidence 无关,confidence 再高也不能绕过)。
CANDIDATE_GOAL_COVERAGE_MIN = 0.80
CANDIDATE_ACCEPTANCE_COVERAGE_MIN = 0.85
CANDIDATE_PROJECT_FIT_MIN = 0.75

#: attempt_count >= 3 且仍不达标 → blocked,validator 不得建议第四轮自动调查。
ATTEMPT_LIMIT = 3

#: 证据等级 → 单条证据可用支持度(冻结权重)。支持度按证据 id 去重后求和,
#: 上限 1.0;重复引用同一证据不提升分数。SSOT 文档:
#: references/confidence-and-candidate-rubric.md(一致性由 contract 测试锁定)。
EVIDENCE_LEVEL_SUPPORT: Mapping[str, float] = {
    "E0": 0.05,
    "E1": 0.15,
    "E2": 0.25,
    "E3": 0.40,
    "E4": 0.55,
}

#: 声明(重算)分数超过去重证据支持度上限的容差;超过即 score_inflation。
SCORE_INFLATION_TOLERANCE = 0.05

#: selected 候选的完成证据等级要求:至少引用一条 E3/E4(实际执行/独立验收)。
COMPLETION_EVIDENCE_LEVELS: tuple[str, ...] = ("E3", "E4")

#: 矛盾证据观察前缀:同一主题两条及以上互相矛盾的 E3/E4 证据用该前缀标记,
#: 格式为 "conflicting_evidence:<主题>: <陈述>"。矛盾只能由用户裁决解除,
#: 不得用新证据静默覆盖旧事实。
CONFLICTING_EVIDENCE_MARKER = "conflicting_evidence:"

#: 用户确认记录的四个必备键(作者可交接前必须逐项 confirmed: true)。
USER_CONFIRMATION_KEYS: tuple[str, ...] = (
    "goal",
    "scope",
    "completion_evidence",
    "failure_boundaries",
)

# --- next_action 稳定词表(机器可读) ---------------------------------------

NEXT_HANDOFF = "ready_for_handoff"  # 可交接给计划作者
NEXT_INVESTIGATE = "rerun_investigation"  # 补调查、产生新证据后重算
NEXT_CONFIRM_USER = "confirm_with_user"  # 逐题问用户 / 等待人工输入
NEXT_SWITCH_CANDIDATE = "switch_candidate"  # 换候选方案
NEXT_EMIT_BLOCKED = "emit_blocked"  # 输出 blocked 状态

# --- 丢弃/门禁结果标记 -------------------------------------------------------

REJECTED_LOW_CONFIDENCE = "rejected_low_confidence"
REJECTED_INSUFFICIENT_COVERAGE = "rejected_insufficient_coverage"

#: Candidate.status 允许值(展示/追踪 + validator 一致性审计)。
CANDIDATE_STATUSES: tuple[str, ...] = (
    "pending",
    "selected",
    REJECTED_LOW_CONFIDENCE,
    REJECTED_INSUFFICIENT_COVERAGE,
)


# --- 确定性支持度计算(纯函数) ----------------------------------------------


def compute_support(
    evidence_ids: Iterable[str], evidence_levels: Mapping[str, str]
) -> float:
    """计算一组证据对声明分数的可审计支持度(0..1)。

    * ``evidence_ids`` 按 id **去重**:同一证据被重复引用只计一次
      (重复引用不提升分数);
    * 每条存在的证据按其等级贡献 ``EVIDENCE_LEVEL_SUPPORT`` 权重;
      不在 ``evidence_levels`` 台账中的 id 贡献 0(引用完整性由
      validator 的 unreferenced_evidence 单独审计);
    * 结果上限 1.0。

    该值是声明 confidence 的可审计上限:声明分 ≤ 支持度总是允许
    (保守声明);显著超出(> 支持度 + SCORE_INFLATION_TOLERANCE)→
    score_inflation。
    """
    support = 0.0
    seen: set[str] = set()
    for evidence_id in evidence_ids:
        if evidence_id in seen:
            continue
        seen.add(evidence_id)
        level = evidence_levels.get(evidence_id)
        if level is not None:
            support += EVIDENCE_LEVEL_SUPPORT.get(level, 0.0)
    return min(1.0, support)


# --- 纯数据结构 --------------------------------------------------------------


@dataclass(frozen=True)
class Evidence:
    """证据台账中的一条证据。"""

    id: str
    source: str
    observation: str
    level: str


@dataclass(frozen=True)
class DecisionRecord:
    """一条"问题 → 结论"的决策记录。"""

    id: str
    question: str
    confidence: float | None
    supporting_evidence: tuple[str, ...]
    blocking: bool
    resolved: bool
    resolution: str | None = None
    uncovered_risks: tuple[str, ...] = ()


@dataclass(frozen=True)
class Candidate:
    """一个候选方案及其独立覆盖度评估。

    新增字段均为可选(向后兼容):

    * ``risk_coverage``:仅展示/追踪用,**不是**第四个覆盖门禁
      (冻结的三个硬覆盖门禁仍是 goal_coverage / acceptance_coverage /
      project_fit);
    * ``status``:候选状态(pending / selected / rejected_low_confidence /
      rejected_insufficient_coverage),validator 审计其与门禁结论一致;
    * ``rejection_reason``:淘汰原因(被淘汰候选的可追溯说明)。
    """

    id: str
    summary: str
    confidence: float | None
    goal_coverage: float | None
    acceptance_coverage: float | None
    project_fit: float | None
    supporting_evidence: tuple[str, ...]
    selected: bool
    risk_coverage: float | None = None
    status: str | None = None
    rejection_reason: str | None = None


@dataclass(frozen=True)
class InvestigationAttempt:
    """一次低分调查/重算尝试的可审计记录。

    记录「第 N 轮新增哪些证据 → 分数从 X 重算到 Y」:

    * ``round``:轮序号,从 1 开始连续递增;
    * ``candidate_id``:被重算的候选;
    * ``added_evidence``:本轮新增的证据 id(必须存在于台账);
    * ``score_before`` / ``score_after``:重算前后分数;同一候选相邻
      两轮必须链式衔接(本轮 before == 上轮 after);
    * ``provenance``:出处/结论说明。

    ``score_after`` 不得超过该候选去重证据的支持度上限
    (``compute_support`` + ``SCORE_INFLATION_TOLERANCE``)。
    """

    round: int
    candidate_id: str
    added_evidence: tuple[str, ...]
    score_before: float | None
    score_after: float | None
    provenance: str | None = None


@dataclass(frozen=True)
class UserConfirmation:
    """用户对某一事项(goal/scope/完成证据/失败边界)的确认记录。"""

    confirmed: bool
    note: str | None = None


@dataclass(frozen=True)
class TaskBrief:
    """一份任务简报的类型化视图。

    ``from_mapping`` 做的是宽容解析(缺字段/类型不符的项记为 None 或省略),
    不替代 ``brief_validator`` 的门禁校验。
    """

    schema_version: str | None
    project_root: str | None
    status: str | None
    previous_status: str | None
    attempt_count: int
    goal: str | None
    confidence: Mapping[str, float | None]
    evidence: tuple[Evidence, ...]
    decisions: tuple[DecisionRecord, ...]
    candidates: tuple[Candidate, ...]
    user_confirmations: Mapping[str, UserConfirmation]
    investigation_attempts: tuple[InvestigationAttempt, ...] = ()

    @classmethod
    def from_mapping(cls, data: Mapping[str, Any]) -> "TaskBrief":
        def _opt_str(value: Any) -> str | None:
            return value if isinstance(value, str) and value else None

        def _opt_score(value: Any) -> float | None:
            if isinstance(value, bool) or not isinstance(value, (int, float)):
                return None
            return float(value)

        def _str_tuple(value: Any) -> tuple[str, ...]:
            if not isinstance(value, list):
                return ()
            return tuple(item for item in value if isinstance(item, str))

        confidence_raw = data.get("confidence")
        confidence: dict[str, float | None] = {}
        for dim in KEY_DIMENSIONS:
            raw = confidence_raw.get(dim) if isinstance(confidence_raw, Mapping) else None
            confidence[dim] = _opt_score(raw)

        evidence: list[Evidence] = []
        evidence_raw = data.get("evidence")
        if isinstance(evidence_raw, list):
            for entry in evidence_raw:
                if not isinstance(entry, Mapping):
                    continue
                entry_id = _opt_str(entry.get("id"))
                source = _opt_str(entry.get("source"))
                observation = _opt_str(entry.get("observation"))
                level = _opt_str(entry.get("level"))
                if entry_id is None or source is None or observation is None or level is None:
                    continue  # 残缺条目不进类型化视图,由 validator 报错
                evidence.append(
                    Evidence(id=entry_id, source=source, observation=observation, level=level)
                )

        decisions: list[DecisionRecord] = []
        decisions_raw = data.get("decisions")
        if isinstance(decisions_raw, list):
            for entry in decisions_raw:
                if not isinstance(entry, Mapping):
                    continue
                decisions.append(
                    DecisionRecord(
                        id=_opt_str(entry.get("id")) or "",
                        question=_opt_str(entry.get("question")) or "",
                        confidence=_opt_score(entry.get("confidence")),
                        supporting_evidence=_str_tuple(entry.get("supporting_evidence")),
                        blocking=entry.get("blocking") is True,
                        resolved=entry.get("resolved") is True,
                        resolution=_opt_str(entry.get("resolution")),
                        uncovered_risks=_str_tuple(entry.get("uncovered_risks")),
                    )
                )

        candidates: list[Candidate] = []
        candidates_raw = data.get("candidates")
        if isinstance(candidates_raw, list):
            for entry in candidates_raw:
                if not isinstance(entry, Mapping):
                    continue
                candidates.append(
                    Candidate(
                        id=_opt_str(entry.get("id")) or "",
                        summary=_opt_str(entry.get("summary")) or "",
                        confidence=_opt_score(entry.get("confidence")),
                        goal_coverage=_opt_score(entry.get("goal_coverage")),
                        acceptance_coverage=_opt_score(entry.get("acceptance_coverage")),
                        project_fit=_opt_score(entry.get("project_fit")),
                        supporting_evidence=_str_tuple(entry.get("supporting_evidence")),
                        selected=entry.get("selected") is True,
                        risk_coverage=_opt_score(entry.get("risk_coverage")),
                        status=_opt_str(entry.get("status")),
                        rejection_reason=_opt_str(entry.get("rejection_reason")),
                    )
                )

        confirmations: dict[str, UserConfirmation] = {}
        confirmations_raw = data.get("user_confirmations")
        if isinstance(confirmations_raw, Mapping):
            for key in USER_CONFIRMATION_KEYS:
                entry = confirmations_raw.get(key)
                if not isinstance(entry, Mapping):
                    continue
                confirmations[key] = UserConfirmation(
                    confirmed=entry.get("confirmed") is True,
                    note=_opt_str(entry.get("note")),
                )

        attempt_raw = data.get("attempt_count", 1)
        if isinstance(attempt_raw, bool) or not isinstance(attempt_raw, int) or attempt_raw < 1:
            attempt_raw = 1

        attempts: list[InvestigationAttempt] = []
        attempts_raw = data.get("investigation_attempts")
        if isinstance(attempts_raw, list):
            for entry in attempts_raw:
                if not isinstance(entry, Mapping):
                    continue
                round_raw = entry.get("round")
                attempts.append(
                    InvestigationAttempt(
                        round=round_raw
                        if isinstance(round_raw, int) and not isinstance(round_raw, bool)
                        else 0,
                        candidate_id=_opt_str(entry.get("candidate_id")) or "",
                        added_evidence=_str_tuple(entry.get("added_evidence")),
                        score_before=_opt_score(entry.get("score_before")),
                        score_after=_opt_score(entry.get("score_after")),
                        provenance=_opt_str(entry.get("provenance")),
                    )
                )

        return cls(
            schema_version=_opt_str(data.get("schema_version")),
            project_root=_opt_str(data.get("project_root")),
            status=_opt_str(data.get("status")),
            previous_status=_opt_str(data.get("previous_status")),
            attempt_count=attempt_raw,
            goal=_opt_str(data.get("goal")),
            confidence=confidence,
            evidence=tuple(evidence),
            decisions=tuple(decisions),
            candidates=tuple(candidates),
            user_confirmations=confirmations,
            investigation_attempts=tuple(attempts),
        )


@dataclass(frozen=True)
class ValidationError:
    """一条门禁错误:稳定 code + JSON-path 位置 + 人类可读 message + next_action。"""

    code: str
    path: str
    message: str
    next_action: str

    def to_dict(self) -> dict[str, str]:
        return {
            "code": self.code,
            "path": self.path,
            "message": self.message,
            "next_action": self.next_action,
        }


@dataclass(frozen=True)
class Rejection:
    """一个被硬门禁丢弃的维度/决策/候选(rejected_low_confidence 等)。"""

    kind: str  # "dimension" | "decision" | "candidate"
    id: str
    reason: str
    next_action: str

    def to_dict(self) -> dict[str, str]:
        return {
            "kind": self.kind,
            "id": self.id,
            "reason": self.reason,
            "next_action": self.next_action,
        }


@dataclass(frozen=True)
class GateResult:
    """单个候选方案的门禁结论。"""

    candidate_id: str
    outcome: str  # selected | viable | needs_investigation | rejected_*
    failed_gates: tuple[str, ...]

    def to_dict(self) -> dict[str, Any]:
        return {
            "candidate_id": self.candidate_id,
            "outcome": self.outcome,
            "failed_gates": list(self.failed_gates),
        }


@dataclass(frozen=True)
class ValidationResult:
    """validator 的完整类型化输出。

    * ``valid``:brief 内部一致且声明状态与门禁结论不冲突(无错误)。
    * ``author_ready``:门禁认证结论——author_ready 充要条件全部满足。
    * ``recommended_status``:门禁推导出的状态建议。
    * ``handoff_block_reasons``:禁止 handoff 的原因清单(可交接时为空)。
    * ``missing_evidence``:被引用但证据台账中不存在的证据 id 清单。
    """

    valid: bool
    author_ready: bool
    recommended_status: str
    next_action: str
    errors: tuple[ValidationError, ...]
    rejections: tuple[Rejection, ...]
    candidate_gates: tuple[GateResult, ...]
    handoff_block_reasons: tuple[str, ...]
    missing_evidence: tuple[str, ...]
    brief: TaskBrief | None

    def to_dict(self) -> dict[str, Any]:
        return {
            "valid": self.valid,
            "author_ready": self.author_ready,
            "recommended_status": self.recommended_status,
            "next_action": self.next_action,
            "errors": [error.to_dict() for error in self.errors],
            "rejections": [rejection.to_dict() for rejection in self.rejections],
            "candidate_gates": [gate.to_dict() for gate in self.candidate_gates],
            "handoff_block_reasons": list(self.handoff_block_reasons),
            "missing_evidence": list(self.missing_evidence),
        }

    def to_json(self) -> str:
        return json.dumps(self.to_dict(), ensure_ascii=False, indent=2)

    def to_yaml(self) -> str:
        import yaml  # 延迟导入:保持模块导入期零副作用

        return yaml.safe_dump(self.to_dict(), allow_unicode=True)
