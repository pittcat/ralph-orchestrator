"""ralph-task-discovery discovery transcript 解释器(U2)。

把 SKILL.md 定义的 discovery 工作流规则实现为确定性纯函数
(无 I/O、无随机、无 LLM):transcript(项目事实 + 用户回答序列)→
可直接提交给 ``brief_validator`` 的 task brief mapping。

工作流规则(与 SKILL.md 一一对应,规则出处见
``references/external-skill-adapters.md``):

* **事实查环境,不反问用户**:已知项目事实会让对应的事实类问题从
  开放问题清单中消失;事实类问题永远路由到环境调查(``environment``),
  不会路由给用户。
* **决策一次一问**:决策类问题路由给用户(``user``);显式回答使问题
  关闭;模糊回答产生**恰好一个**带推荐项的澄清问题。
* **术语/边界冲突显式记录**:冲突写入证据台账(``terminology_conflict:``
  前缀)并挂一条未决 blocking 决策,等待用户裁决;**绝不**自动覆盖
  glossary 或代码。
* **bug 任务必须先有 red-capable 反馈回路**:缺少 ``red_capable_loop``
  事实时状态收敛到 ``needs_investigation``,不产生已确认根因决策,
  不产生执行方案(候选为空)。
* **三轮调查/替代仍不达标 → blocked**:``attempt_count >= 3`` 且未收敛
  到 ``author_ready`` 时状态为 ``blocked``,等待人工输入,不得开启第四轮
  自动调查(与 ``brief_validator`` 的 recommended_status 及
  ``references/author-handoff.md`` 阶段 1 失败形态一致)。
* **外部 skill 必须带 provenance**:corpus 可用且方法被应用 →
  ``external_skill_applied:<name>`` + 出处路径(E1);corpus 不可用 →
  ``external_skill_unavailable:<names>`` + ``fallback:`` 替代规则与预期
  出处;绝不伪造外部 skill 已执行。

输出 brief 的状态由规则推导,与 ``brief_validator`` 的
``recommended_status`` 一致(valid=true 是 e2e 测试的验收条件)。
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Mapping

from task_brief import ATTEMPT_LIMIT, SUPPORTED_SCHEMA_VERSION, USER_CONFIRMATION_KEYS

# --- 稳定标记(agent-facing contract) ----------------------------------------

#: 外部 skill 方法被应用(仅当 corpus 可用时允许出现)。
MARKER_APPLIED = "external_skill_applied:"
#: 外部 skill corpus 不可用(必须与 fallback provenance 同时出现)。
MARKER_UNAVAILABLE = "external_skill_unavailable:"
#: 术语/边界冲突证据的观察前缀。
MARKER_CONFLICT = "terminology_conflict:"
#: 禁止标记:出现即表示 glossary 被自动覆盖(discovery 阶段绝不允许)。
FORBIDDEN_OVERRIDE_MARKER = "glossary_overridden"

# --- 任务类型与必需事实主题 ----------------------------------------------------

DEFAULT_TASK_TYPE = "feature"

#: 每种任务类型必须通过环境调查获得的事实主题。
#: 缺失主题进入 unknowns 清单与开放问题清单(路由 environment)。
REQUIRED_FACT_TOPICS: Mapping[str, tuple[str, ...]] = {
    "feature": ("package_manifest", "test_command", "ci_workflow"),
    # bug 任务额外要求 red-capable 反馈回路(diagnosing-bugs Phase 1 判据)。
    "bug": ("entry_point", "test_command", "red_capable_loop"),
}

# --- 外部 skill provenance(corpus 相对路径 SSOT) -----------------------------

EXTERNAL_SKILL_PROVENANCE: Mapping[str, str] = {
    "grilling": "skills/productivity/grilling/SKILL.md",
    "domain-modeling": "skills/engineering/domain-modeling/SKILL.md",
    "diagnosing-bugs": "skills/engineering/diagnosing-bugs/SKILL.md",
    "codebase-design": "skills/engineering/codebase-design/SKILL.md",
    "triage": "skills/engineering/triage/SKILL.md",
    "wayfinder": "skills/engineering/wayfinder/SKILL.md",
    "grill-with-docs": "skills/engineering/grill-with-docs/SKILL.md",
    "to-spec": "skills/engineering/to-spec/SKILL.md",
}

#: corpus 不可用时的最小 fallback(与 external-skill-adapters.md 一致)。
EXTERNAL_SKILL_FALLBACKS: Mapping[str, str] = {
    "grilling": "内置四主题逐题确认问题列表(goal/scope/completion_evidence/failure_boundaries)",
    "domain-modeling": "内置 glossary/代码交叉核对清单(冲突记录为 terminology_conflict 证据)",
    "diagnosing-bugs": "内置 red-capable 反馈回路判据清单(无回路不确认根因)",
    "codebase-design": "内置 module/interface/seam/depth 词汇对照表",
    "triage": "内置重复实现/历史拒绝两项前置检查",
    "wayfinder": "内置 unknowns 清单(fog 保留,不强行拆问题)",
    "grill-with-docs": "内置逐题确认 + 术语记录组合流程",
    "to-spec": "内置 brief 综合(author_ready 输出作为交接材料,不再面谈)",
}

# --- 开放问题 ------------------------------------------------------------------


@dataclass(frozen=True)
class OpenQuestion:
    """一个开放问题。

    ``routed_to``:``environment`` = 通过项目调查获得(不反问用户);
    ``user`` = 必须逐题问用户的业务决策。
    """

    id: str
    topic: str
    kind: str  # "fact" | "decision"
    routed_to: str  # "environment" | "user"
    text: str
    recommendation: str | None = None


#: 事实类问题模板(topic → 问题)。全部路由 environment。
FACT_QUESTION_TEMPLATES: Mapping[str, tuple[str, str]] = {
    "package_manifest": (
        "q_package_layout",
        "项目包结构与清单(依赖、workspace、入口)是什么?",
    ),
    "test_command": (
        "q_test_command",
        "哪个命令能真实运行本项目的测试入口?",
    ),
    "ci_workflow": ("q_ci_entry", "CI 门禁在哪里、跑什么?"),
    "entry_point": ("q_entry_point", "症状对应的代码入口在哪里?"),
    "red_capable_loop": (
        "q_red_capable_loop",
        "哪个命令能针对该真实症状变红(red-capable 反馈回路)?",
    ),
}

#: 决策类问题模板(topic → 问题 + 推荐项)。全部路由 user,且必须带推荐项。
DECISION_QUESTION_TEMPLATES: Mapping[str, tuple[str, str, str]] = {
    "goal": (
        "q_goal_success_criteria",
        "任务的成功条件(目标)是什么?",
        "以用户目标请求为准,补全可验证的完成标准",
    ),
    "scope": (
        "q_scope_boundaries",
        "范围边界是什么(做什么 / 明确不做什么)?",
        "只包含与目标直接相关的模块,其余显式列为非目标",
    ),
    "completion_evidence": (
        "q_completion_evidence",
        "完成证据是什么(什么算做完、如何验证)?",
        "以已知测试入口为准:新增/修改的验证命令由红转绿即算完成",
    ),
    "failure_boundaries": (
        "q_failure_boundaries",
        "关键失败边界是什么(哪些失败必须停下来)?",
        "任一硬门禁失败、数据损坏风险或不可回滚操作即停止",
    ),
}


def _task_type(transcript: Mapping[str, Any]) -> str:
    value = transcript.get("task_type")
    if isinstance(value, str) and value in REQUIRED_FACT_TOPICS:
        return value
    return DEFAULT_TASK_TYPE


def _required_topics(transcript: Mapping[str, Any]) -> tuple[str, ...]:
    return REQUIRED_FACT_TOPICS[_task_type(transcript)]


def _facts_by_topic(transcript: Mapping[str, Any]) -> dict[str, Mapping[str, Any]]:
    facts = transcript.get("project_facts") or ()
    result: dict[str, Mapping[str, Any]] = {}
    for fact in facts:
        if isinstance(fact, Mapping) and isinstance(fact.get("topic"), str):
            result[fact["topic"]] = fact
    return result


def _answers_by_topic(transcript: Mapping[str, Any]) -> dict[str, Mapping[str, Any]]:
    answers = transcript.get("user_answers") or ()
    result: dict[str, Mapping[str, Any]] = {}
    for answer in answers:
        if isinstance(answer, Mapping) and isinstance(answer.get("topic"), str):
            result[answer["topic"]] = answer
    return result


def unknown_topics(transcript: Mapping[str, Any]) -> tuple[str, ...]:
    """尚未通过环境调查获得的必需事实主题(unknowns 清单)。"""
    facts = _facts_by_topic(transcript)
    return tuple(topic for topic in _required_topics(transcript) if topic not in facts)


def open_questions(transcript: Mapping[str, Any]) -> tuple[OpenQuestion, ...]:
    """当前开放问题清单(确定性)。

    * 已知事实对应的事实问题被移除;剩余事实问题路由 environment;
    * 显式回答的决策问题被关闭;模糊回答替换为恰好一个带推荐项的澄清问题;
      未回答的决策问题保持路由 user。
    """
    facts = _facts_by_topic(transcript)
    answers = _answers_by_topic(transcript)
    questions: list[OpenQuestion] = []

    for topic in unknown_topics(transcript):
        question_id, text = FACT_QUESTION_TEMPLATES[topic]
        questions.append(
            OpenQuestion(
                id=question_id,
                topic=topic,
                kind="fact",
                routed_to="environment",
                text=text,
            )
        )

    for topic in USER_CONFIRMATION_KEYS:
        question_id, text, recommendation = DECISION_QUESTION_TEMPLATES[topic]
        answer = answers.get(topic)
        if answer is None:
            questions.append(
                OpenQuestion(
                    id=question_id,
                    topic=topic,
                    kind="decision",
                    routed_to="user",
                    text=text,
                    recommendation=recommendation,
                )
            )
        elif answer.get("clarity") == "vague":
            questions.append(
                OpenQuestion(
                    id=f"q_clarify_{topic}",
                    topic=topic,
                    kind="decision",
                    routed_to="user",
                    text=f"就「{text}」给出一个可验证的明确回答(上一轮回答过于模糊)。",
                    recommendation=recommendation,
                )
            )
        # explicit → 关闭,不再询问

    return tuple(questions)


# --- brief 构建 -----------------------------------------------------------------


def build_brief(transcript: Mapping[str, Any]) -> dict[str, Any]:
    """按 SKILL.md 工作流规则把 transcript 收敛为 task brief mapping。"""
    task_type = _task_type(transcript)
    facts = _facts_by_topic(transcript)
    answers = _answers_by_topic(transcript)
    conflicts = list(transcript.get("conflicts") or ())
    external = transcript.get("external_skills") or {}
    corpus_available = external.get("corpus_available") is True
    corpus_root = external.get("corpus_root") or "external-corpus"

    evidence: list[dict[str, str]] = []

    def add_evidence(source: str, observation: str, level: str) -> str:
        entry_id = f"E{len(evidence) + 1}"
        evidence.append(
            {"id": entry_id, "source": source, "observation": observation, "level": level}
        )
        return entry_id

    # 1) 项目事实(E1-E3,顺序 = transcript 中的主题出现顺序)
    fact_order = [
        fact["topic"]
        for fact in (transcript.get("project_facts") or ())
        if isinstance(fact, Mapping) and isinstance(fact.get("topic"), str)
    ]
    fact_evidence_ids: list[str] = []
    for topic in fact_order:
        fact = facts[topic]
        fact_evidence_ids.append(
            add_evidence(
                str(fact.get("source", "")),
                str(fact.get("observation", "")),
                str(fact.get("level", "E1")),
            )
        )

    # 2) 术语/边界冲突(显式记录,绝不自动覆盖)
    conflict_pairs: list[tuple[Mapping[str, Any], str]] = []
    for conflict in conflicts:
        if not isinstance(conflict, Mapping):
            continue
        observation = (
            f"{MARKER_CONFLICT}{conflict.get('term', '?')}: "
            f"glossary={conflict.get('glossary_statement', '')}; "
            f"code={conflict.get('code_statement', '')}"
        )
        source = (
            f"{conflict.get('glossary_source', '?')} <-> {conflict.get('code_source', '?')}"
        )
        conflict_pairs.append((conflict, add_evidence(source, observation, "E2")))

    # 3) 外部 skill provenance(可用 → applied;不可用 → unavailable + fallback)
    if corpus_available:
        for name in external.get("applied_methods") or ():
            provenance = EXTERNAL_SKILL_PROVENANCE.get(name)
            if provenance is None:
                continue
            add_evidence(f"{corpus_root}:{provenance}", f"{MARKER_APPLIED}{name}", "E1")
    else:
        needed = tuple(external.get("needed_methods") or ())
        known_needed = tuple(
            name for name in needed if name in EXTERNAL_SKILL_PROVENANCE
        )
        if known_needed:
            fallback_bits = "; ".join(
                f"{name} -> {EXTERNAL_SKILL_FALLBACKS[name]}" for name in known_needed
            )
            provenance_bits = "; ".join(
                f"{corpus_root}:{EXTERNAL_SKILL_PROVENANCE[name]}"
                for name in known_needed
            )
            add_evidence(
                f"fallback: {fallback_bits} | expected provenance: {provenance_bits}",
                f"{MARKER_UNAVAILABLE}{','.join(known_needed)}",
                "E1",
            )

    # 4) 用户回答(E0,按四个确认主题顺序)
    answer_evidence_ids: dict[str, str] = {}
    for topic in USER_CONFIRMATION_KEYS:
        answer = answers.get(topic)
        if answer is None:
            continue
        answer_evidence_ids[topic] = add_evidence(
            "与用户的对话记录", str(answer.get("answer", "")), "E0"
        )

    # 5) 决策记录(逐题确认;模糊回答保持未决)
    decisions: list[dict[str, Any]] = []
    for topic in USER_CONFIRMATION_KEYS:
        answer = answers.get(topic)
        if answer is None:
            continue
        explicit = answer.get("clarity", "explicit") == "explicit"
        _, text, _ = DECISION_QUESTION_TEMPLATES[topic]
        entry: dict[str, Any] = {
            "id": f"D{len(decisions) + 1}",
            "question": text,
            "confidence": 0.88 if explicit else 0.75,
            "supporting_evidence": [answer_evidence_ids[topic]],
            "blocking": True,
            "resolved": explicit,
        }
        if explicit:
            entry["resolution"] = str(answer.get("answer", ""))
            entry["uncovered_risks"] = []
        decisions.append(entry)

    for conflict, evidence_id in conflict_pairs:
        decisions.append(
            {
                "id": f"D{conflict.get('id', 'X')}",
                "question": (
                    f"术语「{conflict.get('term', '?')}」在 glossary 与代码中定义冲突,"
                    "以哪一侧为准?(裁决前不覆盖任何一侧)"
                ),
                "confidence": 0.85,
                "supporting_evidence": [evidence_id],
                "blocking": True,
                "resolved": False,
            }
        )

    # 6) 候选方案(bug 无 red-capable 回路 → 不产生执行方案)
    #    transcript 可携带 `candidates`(多候选淘汰流)与
    #    `investigation_attempts`(补证据重算流);缺省时保持单候选 C1 行为。
    red_loop_known = "red_capable_loop" in facts
    fact_topic_evidence = dict(zip(fact_order, fact_evidence_ids))
    attempts_raw = transcript.get("investigation_attempts") or ()

    def _topic_ids(topics: Any) -> list[str]:
        """事实主题 → 证据 id(去重保序;未知主题忽略)。"""
        ids: list[str] = []
        for topic in topics or ():
            evidence_id = fact_topic_evidence.get(topic)
            if evidence_id is not None and evidence_id not in ids:
                ids.append(evidence_id)
        return ids

    # 调查尝试新增的主题并入对应候选的 supporting_evidence(重算可回溯)
    attempt_topics_by_candidate: dict[str, list[str]] = {}
    for attempt in attempts_raw:
        if isinstance(attempt, Mapping) and isinstance(attempt.get("candidate_id"), str):
            attempt_topics_by_candidate.setdefault(
                attempt["candidate_id"], []
            ).extend(attempt.get("added_topics") or ())

    transcript_candidates = transcript.get("candidates")
    candidates: list[dict[str, Any]] = []
    if task_type == "bug" and not red_loop_known:
        candidates = []
    elif isinstance(transcript_candidates, list) and transcript_candidates:
        for index, cand in enumerate(transcript_candidates):
            if not isinstance(cand, Mapping):
                continue
            cand_id = (
                cand.get("id")
                if isinstance(cand.get("id"), str) and cand.get("id")
                else f"C{index + 1}"
            )
            topics = list(cand.get("evidence_topics") or ())
            topics.extend(attempt_topics_by_candidate.get(cand_id, ()))
            cand_entry: dict[str, Any] = {
                "id": cand_id,
                "summary": str(cand.get("summary", "")),
                "confidence": cand.get("confidence"),
                "goal_coverage": cand.get("goal_coverage"),
                "acceptance_coverage": cand.get("acceptance_coverage"),
                "project_fit": cand.get("project_fit"),
                "supporting_evidence": _topic_ids(topics),
                "selected": cand.get("selected") is True,
            }
            for optional_key in ("risk_coverage", "status", "rejection_reason"):
                if cand.get(optional_key) is not None:
                    cand_entry[optional_key] = cand[optional_key]
            candidates.append(cand_entry)
    elif fact_evidence_ids:
        candidates.append(
            {
                "id": "C1",
                "summary": "按项目现有模式实现:复用既有入口与验证命令,不引入新依赖",
                "confidence": 0.88,
                "goal_coverage": 0.86,
                "acceptance_coverage": 0.86,
                "project_fit": 0.80,
                "supporting_evidence": fact_evidence_ids[:2],
                "selected": True,
            }
        )

    # 7) 用户确认记录(只有显式回答记为 confirmed)
    user_confirmations: dict[str, dict[str, Any]] = {}
    for topic in USER_CONFIRMATION_KEYS:
        answer = answers.get(topic)
        confirmed = answer is not None and answer.get("clarity", "explicit") == "explicit"
        entry: dict[str, Any] = {"confirmed": confirmed}
        if confirmed and answer is not None:
            entry["note"] = str(answer.get("answer", ""))
        user_confirmations[topic] = entry

    # 8) 五维置信度(确定性分带,禁止平均值)
    required = _required_topics(transcript)
    known_required = [topic for topic in required if topic in facts]
    if len(known_required) == len(required):
        fact_band = 0.85
    elif known_required:
        fact_band = 0.75
    else:
        fact_band = 0.50

    goal_request = transcript.get("goal_request")
    has_goal_request = isinstance(goal_request, str) and bool(goal_request.strip())
    goal_clarity = 0.85 if (has_goal_request and (facts or answers)) else 0.50

    if task_type == "bug":
        acceptance = 0.85 if red_loop_known else (0.75 if facts else 0.50)
    else:
        acceptance = 0.85 if "test_command" in facts else (0.75 if facts else 0.50)

    execution_feasibility = 0.85 if candidates else (0.75 if facts else 0.50)

    confidence = {
        "goal_clarity": goal_clarity,
        "project_fact_coverage": fact_band,
        "acceptance_evidence": acceptance,
        "execution_feasibility": execution_feasibility,
        "risk_coverage": fact_band,
    }

    # 9) 状态收敛(与 brief_validator 的 recommended_status 一致)
    unresolved_conflicts = bool(conflict_pairs)
    all_confirmed = all(
        entry["confirmed"] for entry in user_confirmations.values()
    )
    attempt_count = transcript.get("attempt_count", 1)
    # attempt_count 类型收敛(与 brief_validator 同字段校验契约对称):
    # 整数浮点归一为 int(3.0 → 3),bool / 非整数 / <1 抛带稳定消息的
    # ValueError —— 不抛裸 TypeError,也不让非法值透传进 brief 被 validator
    # 判 invalid_score。合法 int >= 1 原样使用,后续收敛逻辑不变。
    if isinstance(attempt_count, float) and attempt_count.is_integer():
        attempt_count = int(attempt_count)
    if (
        isinstance(attempt_count, bool)
        or not isinstance(attempt_count, int)
        or attempt_count < 1
    ):
        raise ValueError(
            f"attempt_count 必须是 >= 1 的整数,当前为 {attempt_count!r}"
        )
    if task_type == "bug" and not red_loop_known:
        status = "needs_investigation"
    elif any(score < 0.85 for score in confidence.values()) or not candidates:
        status = "needs_investigation"
    elif unresolved_conflicts or not all_confirmed:
        status = "needs_user_decision"
    else:
        status = "author_ready"
    if attempt_count >= ATTEMPT_LIMIT and status != "author_ready":
        # 三轮调查/替代后仍不达标 → blocked(validator 对 attempt_count >= 3
        # 且 failing 的 brief 强制 recommended=blocked;author-handoff.md
        # 阶段 1 失败形态)。不得开启第四轮自动调查。
        status = "blocked"

    # 10) 目标陈述(显式 goal 回答优先于原始请求)
    goal_answer = answers.get("goal")
    if goal_answer is not None and goal_answer.get("clarity", "explicit") == "explicit":
        goal = str(goal_answer.get("answer", "")) or goal_request
    else:
        goal = goal_request

    # 11) 调查/重算尝试透传(added_topics → 证据 id)
    attempts_out: list[dict[str, Any]] = []
    for attempt in attempts_raw:
        if not isinstance(attempt, Mapping):
            continue
        attempts_out.append(
            {
                "round": attempt.get("round"),
                "candidate_id": attempt.get("candidate_id", ""),
                "added_evidence": _topic_ids(attempt.get("added_topics")),
                "score_before": attempt.get("score_before"),
                "score_after": attempt.get("score_after"),
                "provenance": attempt.get("provenance"),
            }
        )

    brief: dict[str, Any] = {
        "schema_version": SUPPORTED_SCHEMA_VERSION,
        "project_root": transcript.get("project_root", ""),
        "status": status,
        "attempt_count": attempt_count,
        "goal": goal,
        "confidence": confidence,
        "evidence": evidence,
        "decisions": decisions,
        "candidates": candidates,
        "user_confirmations": user_confirmations,
    }
    if attempts_out:
        brief["investigation_attempts"] = attempts_out
    previous_status = transcript.get("previous_status")
    if previous_status:
        brief["previous_status"] = previous_status
    return brief
