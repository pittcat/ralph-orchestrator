"""ralph-preset-author 消费 task brief 的交接协议(author handoff)确定性参考实现。

本模块是 `ralph-preset-author` Workflow 0 在消费 task brief 时**应执行的
校验序列**的确定性参考实现;书面协议见本 skill 目录的
`references/author-handoff.md`,两者必须保持一致。

语义约定(首次出现即解释):

* handoff(交接):`ralph-task-discovery` 只把 **task brief 的文件路径**
  交给 `ralph-preset-author`,不复制长文本;author 自己读取并复核。
* verdict(裁定):``author_handoff_ok`` = brief 通过全部复核,其已确认
  事实可进入 author 的既有流程;``task_brief_invalid`` = 任一复核失败,
  author 停在 Discovery gate,不生成任何 preset YAML。
* stale(陈旧)brief:brief 的 provenance(出处)已不对应当前 authoring
  对象——`project_root` 与当前目标项目根不一致,或 validator 报
  provenance 错误(`schema_version_invalid` / `root_provenance_missing`),
  或 goal/目标与当前请求不符。stale brief 一律 `task_brief_invalid`。

本模块只依赖同目录 flat module ``brief_validator``(进而依赖
``task_brief``);只读取调用者给定的 brief 文件,不写盘、不访问其它
文件系统状态、不依赖 Ralph runtime。**它不替代 author 的既有门禁**:
brief 通过复核只是「已确认输入」,Discovery / Intent Confirmation / AAF /
Payload Contract / review handoff 等门禁全部照常执行。
"""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Union

import brief_validator

# --- 稳定裁定与错误 code(冻结契约) ------------------------------------------

#: brief 通过全部复核,已确认事实可进入 author 既有流程。
VERDICT_OK = "author_handoff_ok"
#: 任一复核失败:author 停在 Discovery gate,不生成任何 preset YAML。
VERDICT_INVALID = "task_brief_invalid"

#: author 侧稳定错误 code。文件存在性与目标项目根匹配是 validator 视野之外
#: 的检查(validator 只校验 brief 内部一致性),由 author 侧给出确定性 code;
#: 其余错误透传 brief_validator 的稳定 code(schema §11)。
CODE_FILE_NOT_FOUND = "task_brief_file_not_found"
CODE_ROOT_MISMATCH = "task_brief_root_mismatch"
CODE_NOT_AUTHOR_READY = "task_brief_not_author_ready"


# --- 纯数据结构 ---------------------------------------------------------------


@dataclass(frozen=True)
class HandoffError:
    """一条复核错误:稳定 code + JSON-path 位置 + 人类可读 message。"""

    code: str
    path: str
    message: str

    def to_dict(self) -> dict[str, str]:
        return {"code": self.code, "path": self.path, "message": self.message}


@dataclass(frozen=True)
class HandoffDecision:
    """author 侧对一份 task brief 的完整消费裁定。

    * ``verdict``:``author_handoff_ok`` / ``task_brief_invalid``;
    * ``errors``:复核错误清单(按读取顺序汇报,可交接时为空);
    * 其余字段为 brief 通过复核后供 author 消费的**已确认事实**;
      ``task_brief_invalid`` 时全部为 None / 空(停止语义:invalid brief
      不提供任何可消费事实,author 不得据此生成 preset YAML)。

    字段与 author 用途对照:

    * ``goal`` → Preset Intent Confirmation 的目标;
    * ``acceptance_note`` → 成功条件(来自用户确认 ``completion_evidence``);
    * ``failure_boundaries_note`` → 阻塞条件(来自用户确认 ``failure_boundaries``);
    * ``scope_note`` → 范围(来自用户确认 ``scope``);
    * ``selected_candidate_*`` → 方案输入(仅 validator ``candidate_gates``
      结论为 selected 的候选;被 rejected 的候选不得被当作 selected 使用);
    * ``evidence_ids`` → Evidence refs(证据台账全量 id)。
    """

    verdict: str
    errors: tuple[HandoffError, ...]
    goal: str | None
    scope_note: str | None
    acceptance_note: str | None
    failure_boundaries_note: str | None
    selected_candidate_id: str | None
    selected_candidate_summary: str | None
    selected_candidate_evidence: tuple[str, ...]
    evidence_ids: tuple[str, ...]

    def to_dict(self) -> dict[str, object]:
        return {
            "verdict": self.verdict,
            "errors": [error.to_dict() for error in self.errors],
            "goal": self.goal,
            "scope_note": self.scope_note,
            "acceptance_note": self.acceptance_note,
            "failure_boundaries_note": self.failure_boundaries_note,
            "selected_candidate_id": self.selected_candidate_id,
            "selected_candidate_summary": self.selected_candidate_summary,
            "selected_candidate_evidence": list(self.selected_candidate_evidence),
            "evidence_ids": list(self.evidence_ids),
        }


# --- 确定性实现 ---------------------------------------------------------------


def _normalize_root(value: str) -> str:
    """规范化项目根用于一致性比较:去首尾空白与尾随斜杠(根 "/" 保留)。"""
    cleaned = value.strip()
    while len(cleaned) > 1 and cleaned.endswith("/"):
        cleaned = cleaned[:-1]
    return cleaned


def consumable_selected_candidate(result: "brief_validator.ValidationResult") -> str | None:
    """可被 author 当作 selected 消费的候选 id。

    只认 validator ``candidate_gates`` 结论为 ``selected`` 的候选;brief
    里标了 ``selected: true`` 但被门禁判定 rejected(覆盖不足 / 低置信度)
    的候选**不得**被当作 selected 使用。歧义(无或多个 selected 结论)
    返回 None——author_ready 认证本身已排除歧义,这里是防御性 SSOT。
    """
    selected = [
        gate.candidate_id for gate in result.candidate_gates if gate.outcome == "selected"
    ]
    return selected[0] if len(selected) == 1 else None


def _invalid(errors: tuple[HandoffError, ...]) -> HandoffDecision:
    """构造停止裁定:不消费 brief 的任何事实。"""
    return HandoffDecision(
        verdict=VERDICT_INVALID,
        errors=errors,
        goal=None,
        scope_note=None,
        acceptance_note=None,
        failure_boundaries_note=None,
        selected_candidate_id=None,
        selected_candidate_summary=None,
        selected_candidate_evidence=(),
        evidence_ids=(),
    )


def evaluate_task_brief(
    brief_path: Union[str, Path], target_project_root: str
) -> HandoffDecision:
    """按 author-handoff.md 的读取顺序复核一份 task brief。

    顺序(与书面协议一致):

    1. 文件存在 → 缺失则 ``task_brief_file_not_found``(短路);
    2. 运行 ``brief_validator.validate_brief_text``(真实 ``yaml.safe_load``;
       YAML 可解析性、``schema_version``、``project_root`` provenance、
       全部硬门禁与 author_ready 充要条件都在这里判定),错误按 validator
       自身顺序透传;
    3. ``project_root`` 与 ``target_project_root``(当前目标项目根)规范化
       比较 → 不一致则 ``task_brief_root_mismatch``(stale brief);
       brief 未提供 project_root 时不重复报错(validator 已报
       ``root_provenance_missing``);
    4. author_ready 认证复核 → validator 无错误但未认证(诚实声明
       blocked / needs_user_decision / needs_investigation 的 brief)时,
       补 ``task_brief_not_author_ready``,message 携带 validator 给出的
       禁止 handoff 原因清单。

    ``brief_path`` 由调用者解析(author 收到的是 repo-relative 路径,
    相对当前仓库根解析后传入);本函数不做路径猜测。
    """
    path = Path(brief_path)
    if not path.is_file():
        return _invalid(
            (
                HandoffError(
                    code=CODE_FILE_NOT_FOUND,
                    path="$",
                    message=f"task brief 文件不存在:{brief_path}",
                ),
            )
        )

    text = path.read_text(encoding="utf-8")
    result = brief_validator.validate_brief_text(text)
    errors: list[HandoffError] = [
        HandoffError(code=e.code, path=e.path, message=e.message) for e in result.errors
    ]

    brief = result.brief
    root_value = brief.project_root if brief is not None else None
    if root_value and _normalize_root(root_value) != _normalize_root(target_project_root):
        errors.append(
            HandoffError(
                code=CODE_ROOT_MISMATCH,
                path="$.project_root",
                message=(
                    f"brief 的 project_root {root_value!r} 与当前目标项目根 "
                    f"{target_project_root!r} 不一致:stale brief,不得消费,"
                    "需要对当前项目重新走 discovery"
                ),
            )
        )

    if not errors and not result.author_ready:
        reasons = "; ".join(result.handoff_block_reasons) or (
            f"recommended_status={result.recommended_status}"
        )
        errors.append(
            HandoffError(
                code=CODE_NOT_AUTHOR_READY,
                path="$",
                message=(
                    f"brief 未通过 author_ready 认证"
                    f"(status={brief.status if brief else '未知'},"
                    f"recommended_status={result.recommended_status}):{reasons}"
                ),
            )
        )

    if errors:
        return _invalid(tuple(errors))

    # --- 通过全部复核:消费已确认事实 -----------------------------------------
    selected_id = consumable_selected_candidate(result)
    assert brief is not None  # author_ready 认证蕴含 brief 已成功解析
    selected_candidate = next(
        (candidate for candidate in brief.candidates if candidate.id == selected_id),
        None,
    )
    confirmations = brief.user_confirmations
    return HandoffDecision(
        verdict=VERDICT_OK,
        errors=(),
        goal=brief.goal,
        scope_note=confirmations["scope"].note if "scope" in confirmations else None,
        acceptance_note=(
            confirmations["completion_evidence"].note
            if "completion_evidence" in confirmations
            else None
        ),
        failure_boundaries_note=(
            confirmations["failure_boundaries"].note
            if "failure_boundaries" in confirmations
            else None
        ),
        selected_candidate_id=selected_id,
        selected_candidate_summary=(
            selected_candidate.summary if selected_candidate else None
        ),
        selected_candidate_evidence=(
            selected_candidate.supporting_evidence if selected_candidate else ()
        ),
        evidence_ids=tuple(evidence.id for evidence in brief.evidence),
    )
