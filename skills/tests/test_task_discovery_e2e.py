"""ralph-task-discovery discovery 工作流 e2e 测试(U2)。

被测对象:

* ``fixtures/transcripts/*.yml`` —— deterministic discovery transcript
  (项目事实 + 用户回答序列);
* ``scripts/discovery_transcript.py`` —— SKILL.md 工作流规则的确定性实现
  (transcript → task brief mapping),由 conftest 以 flat module 注册;
* U1 的 ``brief_validator`` —— 对生成的 brief 做硬门禁裁定;
* U4 的 ``author_handoff`` —— author 侧消费/停止裁定;
* ``SKILL.md`` / ``references/external-skill-adapters.md`` /
  ``agents/openai.yaml`` / ``references/task-brief-schema.md`` 的结构化契约。

U6 增补:端到端 pipeline 段把 transcript → discovery mapping → validator →
author handoff 串成一条确定性链(测试内组装,不新增生产模块),验证
低置信度/冲突/缺证据/外部不可用/三轮失败均不能穿透到 author 之后。

测试语义(HARD):断言的是状态、Evidence ID、决策记录与 next_action,
不做"markdown 包含某词"式的文案断言。仅有的结构化词汇断言针对稳定
agent-facing contract,且每处都在注释中说明契约原因。
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

import pytest
import yaml

from brief_validator import validate_brief_data, validate_brief_text
from task_brief import KEY_DIMENSIONS

SKILL_DIR = Path(__file__).resolve().parents[1] / "ralph-task-discovery"
TRANSCRIPTS = SKILL_DIR / "fixtures" / "transcripts"

TRANSCRIPT_FIXTURES = (
    "green.yml",
    "vague-success-criteria.yml",
    "conflicting-doc.yml",
    "bug-without-loop.yml",
    "unknown-project.yml",
    "unavailable-external-skill.yml",
    # U3:多候选淘汰 / 补证据重算
    "alternative-candidates.yml",
    "recompute-with-new-evidence.yml",
    # U6:bug 带 red-capable 回路(与 bug-without-loop.yml 成对照)
    "bug-with-red-loop.yml",
)


def _dt():
    """Import inside the test body so a missing interpreter module surfaces
    as per-test failure (assertion-level Red) instead of a collection error
    that would also swallow the structural tests below."""
    import discovery_transcript  # registered by conftest when the script exists

    return discovery_transcript


def _transcript(name: str) -> dict:
    return yaml.safe_load((TRANSCRIPTS / name).read_text(encoding="utf-8"))


def _build(name: str) -> dict:
    return _dt().build_brief(_transcript(name))


# --- fixtures 基础可用性 ----------------------------------------------------


@pytest.mark.parametrize("name", TRANSCRIPT_FIXTURES)
def test_transcript_fixture_exists_and_parses(name: str) -> None:
    data = _transcript(name)
    assert isinstance(data, dict)
    assert data.get("project_root"), "transcript 必须带 project_root provenance"
    assert data.get("goal_request"), "transcript 必须带目标请求"


# --- green transcript:事实齐全 + 逐题确认 → author_ready --------------------


def test_green_transcript_reaches_author_ready() -> None:
    brief = _build("green.yml")
    # 走 YAML 文本入口,真实经过 yaml.safe_load
    result = validate_brief_text(yaml.safe_dump(brief, allow_unicode=True))
    assert result.valid is True
    assert result.author_ready is True
    assert result.recommended_status == "author_ready"
    assert brief["status"] == "author_ready"
    assert result.next_action == "ready_for_handoff"
    assert result.handoff_block_reasons == ()
    assert result.missing_evidence == ()
    # 项目事实调查产生 E1-E3 证据
    ids = {entry["id"] for entry in brief["evidence"]}
    assert {"E1", "E2", "E3"} <= ids
    # 四个决策主题逐题显式确认,全部 resolved(blocking 决策不允许未决)
    assert {d["id"]: d["resolved"] for d in brief["decisions"]} == {
        "D1": True,
        "D2": True,
        "D3": True,
        "D4": True,
    }
    assert all(d["blocking"] is True for d in brief["decisions"])
    # 用户确认四项齐全
    assert all(
        entry["confirmed"] is True for entry in brief["user_confirmations"].values()
    )
    assert set(brief["user_confirmations"]) == {
        "goal",
        "scope",
        "completion_evidence",
        "failure_boundaries",
    }


def test_green_records_external_method_provenance() -> None:
    dt = _dt()
    brief = _build("green.yml")
    applied = [
        entry
        for entry in brief["evidence"]
        if entry["observation"].startswith(dt.MARKER_APPLIED)
    ]
    assert [entry["observation"] for entry in applied] == [
        dt.MARKER_APPLIED + "grilling"
    ]
    # 外部 skill 结果必须带 provenance(corpus 相对路径),等级为 E1(规则文件级)
    assert applied[0]["source"].endswith("skills/productivity/grilling/SKILL.md")
    assert applied[0]["level"] == "E1"


# --- 事实查环境,不反问用户 ---------------------------------------------------


def test_known_facts_are_not_reasked() -> None:
    # 契约原因:grilling 规则"fact can be found by exploring the environment →
    # look it up rather than asking"(provenance 见 external-skill-adapters.md
    # grilling 行),在 SKILL.md 工作流中稳定化为 open_questions() 的可验证行为:
    # 已知事实对应的问题必须从问题清单中消失,且事实类问题永远路由到环境调查,
    # 不得路由给用户。
    dt = _dt()
    transcript = _transcript("green.yml")
    known_topics = {fact["topic"] for fact in transcript["project_facts"]}
    assert {"package_manifest", "test_command", "ci_workflow"} <= known_topics

    questions = dt.open_questions(transcript)
    # 事实全部已知 + 决策全部显式回答 → 没有遗留问题
    assert questions == ()

    # 抽掉一个事实:对应事实问题回到清单,且只路由到环境(不反问用户)
    partial = dict(transcript)
    partial["project_facts"] = [
        fact
        for fact in transcript["project_facts"]
        if fact["topic"] != "ci_workflow"
    ]
    reopened = dt.open_questions(partial)
    fact_questions = [q for q in reopened if q.kind == "fact"]
    assert [q.topic for q in fact_questions] == ["ci_workflow"]
    assert all(q.routed_to == "environment" for q in fact_questions)
    # 决策类问题不受影响(用户已显式回答)
    assert [q for q in reopened if q.routed_to == "user"] == []


def test_unknown_project_produces_unknowns_list() -> None:
    dt = _dt()
    transcript = _transcript("unknown-project.yml")
    unknowns = dt.unknown_topics(transcript)
    assert unknowns == ("package_manifest", "test_command", "ci_workflow")

    brief = dt.build_brief(transcript)
    result = validate_brief_data(brief)
    assert result.valid is True
    assert result.author_ready is False
    assert result.recommended_status == "needs_investigation"
    assert result.next_action == "rerun_investigation"
    # 尚无事实与回答:证据台账为空,置信度不足维度被诚实丢弃(禁止平均值绕过)
    assert brief["evidence"] == []
    rejected_dims = {r.id for r in result.rejections if r.kind == "dimension"}
    assert "project_fact_coverage" in rejected_dims
    assert "goal_clarity" in rejected_dims


# --- 一次一问:模糊回答 → 恰好一个带推荐项的下一问 ---------------------------


def test_vague_success_criteria_produces_single_recommendation_question() -> None:
    dt = _dt()
    transcript = _transcript("vague-success-criteria.yml")
    questions = dt.open_questions(transcript)
    user_questions = [q for q in questions if q.routed_to == "user"]
    # 一次只问一个:恰好一个面向用户的开放问题
    assert len(user_questions) == 1
    question = user_questions[0]
    assert question.topic == "completion_evidence"
    assert question.recommendation, "澄清问题必须带推荐项"


def test_vague_answer_keeps_needs_user_decision() -> None:
    dt = _dt()
    brief = _build("vague-success-criteria.yml")
    result = validate_brief_data(brief)
    assert result.valid is True
    assert result.author_ready is False
    # 用户确认前 brief 状态保持 needs_user_decision(validator recommended 断言)
    assert brief["status"] == "needs_user_decision"
    assert result.recommended_status == "needs_user_decision"
    assert result.next_action == "confirm_with_user"
    assert result.handoff_block_reasons
    assert result.brief is not None
    assert result.brief.user_confirmations["completion_evidence"].confirmed is False
    # 模糊回答对应的决策必须保持未决(blocking + resolved=false)
    vague_decisions = [
        d
        for d in brief["decisions"]
        if d["question"] and not d["resolved"]
    ]
    assert len(vague_decisions) == 1
    assert vague_decisions[0]["blocking"] is True


def test_unconfirmed_brief_stays_needs_user_decision() -> None:
    # 契约原因:SKILL.md 交接边界——只有 author_ready 才交接;确认前必须停留。
    # 两个不同成因的 fixture(模糊回答 / 尚有未确认主题)都必须收敛到同一状态。
    for name in ("vague-success-criteria.yml", "unavailable-external-skill.yml"):
        brief = _build(name)
        result = validate_brief_data(brief)
        assert result.valid is True, name
        assert result.author_ready is False, name
        assert brief["status"] == "needs_user_decision", name
        assert result.recommended_status == "needs_user_decision", name
        assert result.next_action == "confirm_with_user", name
        assert result.handoff_block_reasons, name


# --- glossary 与代码冲突:记录证据,不自动覆盖 --------------------------------


def test_glossary_code_conflict_is_recorded_not_overridden() -> None:
    dt = _dt()
    brief = _build("conflicting-doc.yml")
    result = validate_brief_data(brief)
    assert result.valid is True
    assert brief["status"] == "needs_user_decision"
    assert result.recommended_status == "needs_user_decision"
    assert result.next_action == "confirm_with_user"

    # 冲突显式记录为证据(稳定 marker 前缀)
    conflict_evidence = [
        entry
        for entry in brief["evidence"]
        if entry["observation"].startswith(dt.MARKER_CONFLICT)
    ]
    assert len(conflict_evidence) == 1
    assert "cancellation" in conflict_evidence[0]["observation"]

    # 冲突对应未决的 blocking 决策,等待用户裁决,并引用冲突证据
    conflict_decisions = [d for d in brief["decisions"] if d["id"] == "DX1"]
    assert len(conflict_decisions) == 1
    assert conflict_decisions[0]["blocking"] is True
    assert conflict_decisions[0]["resolved"] is False
    assert conflict_evidence[0]["id"] in conflict_decisions[0]["supporting_evidence"]

    # 不自动覆盖 glossary:台账中不允许出现覆盖标记证据
    assert all(
        dt.FORBIDDEN_OVERRIDE_MARKER not in entry["observation"]
        for entry in brief["evidence"]
    )


# --- bug 任务无 red-capable 回路 → needs_investigation -----------------------


def test_bug_without_red_capable_loop_needs_investigation() -> None:
    # 契约原因:diagnosing-bugs Phase 1 完成判据——没有 red-capable 命令就没有
    # Phase 2(provenance 见 external-skill-adapters.md diagnosing-bugs 行)。
    dt = _dt()
    transcript = _transcript("bug-without-loop.yml")
    brief = dt.build_brief(transcript)
    result = validate_brief_data(brief)
    assert result.valid is True
    assert brief["status"] == "needs_investigation"
    assert result.recommended_status == "needs_investigation"
    assert result.next_action == "rerun_investigation"
    # 不产生已确认根因决策,不产生执行方案(候选为空)
    assert all("root_cause" not in d["id"].lower() for d in brief["decisions"])
    assert brief["candidates"] == []
    # red-capable 回路作为开放问题进入环境调查清单(路由到环境,不反问用户)
    questions = dt.open_questions(transcript)
    loop_questions = [q for q in questions if q.topic == "red_capable_loop"]
    assert len(loop_questions) == 1
    assert loop_questions[0].routed_to == "environment"


# --- 多候选 transcript:低覆盖候选被淘汰,高分不掩盖单维度 --------------------


def test_alternative_candidates_transcript_rejects_low_coverage() -> None:
    # 验收:两候选均 confidence 0.9,A acceptance_coverage=0.70、B 全达标
    # → A rejected_insufficient_coverage,B selected,author_ready 成立
    brief = _build("alternative-candidates.yml")
    result = validate_brief_data(brief)
    assert result.valid is True
    assert result.author_ready is True
    assert brief["status"] == "author_ready"
    assert result.recommended_status == "author_ready"
    assert result.next_action == "ready_for_handoff"
    gates = {g.candidate_id: g for g in result.candidate_gates}
    assert gates["C-A"].outcome == "rejected_insufficient_coverage"
    assert "acceptance_coverage" in gates["C-A"].failed_gates
    assert gates["C-B"].outcome == "selected"
    assert gates["C-B"].failed_gates == ()
    # 两个候选声明相同的高 confidence:单维度短板不能被平均分掩盖
    candidates = {c["id"]: c for c in brief["candidates"]}
    assert candidates["C-A"]["confidence"] == pytest.approx(0.90)
    assert candidates["C-B"]["confidence"] == pytest.approx(0.90)
    assert candidates["C-A"]["status"] == "rejected_insufficient_coverage"
    assert candidates["C-A"]["rejection_reason"]
    assert candidates["C-B"]["status"] == "selected"
    # 类型化视图同步携带候选状态
    assert result.brief is not None
    by_id = {c.id: c for c in result.brief.candidates}
    assert by_id["C-A"].status == "rejected_insufficient_coverage"
    assert by_id["C-B"].status == "selected"


# --- 重算 transcript:新增 E3/E4 证据后 0.78 → 0.87 --------------------------


def test_recompute_transcript_reaches_author_ready_with_audit_trail() -> None:
    # 验收:investigation_attempts 记录新增 E3/E4 后分数 0.78→0.87,
    # validator 确认重算一致且 author_ready
    brief = _build("recompute-with-new-evidence.yml")
    result = validate_brief_data(brief)
    assert result.valid is True
    assert result.author_ready is True
    assert brief["status"] == "author_ready"
    assert result.recommended_status == "author_ready"
    assert result.next_action == "ready_for_handoff"
    assert "score_inflation" not in {e.code for e in result.errors}

    attempts = brief["investigation_attempts"]
    assert [a["round"] for a in attempts] == [1, 2]
    assert [a["candidate_id"] for a in attempts] == ["C1", "C1"]
    assert attempts[0]["score_after"] == pytest.approx(0.78)
    assert attempts[1]["score_before"] == pytest.approx(0.78)
    assert attempts[1]["score_after"] == pytest.approx(0.87)
    assert brief["candidates"][0]["confidence"] == pytest.approx(0.87)

    # 第二轮新增的证据必须是 E3/E4 完成证据,且被候选引用(重算可回溯)
    ledger = {e["id"]: e["level"] for e in brief["evidence"]}
    added_round2 = attempts[1]["added_evidence"]
    assert added_round2, "第二轮必须引入新证据"
    assert {ledger[eid] for eid in added_round2} == {"E3", "E4"}
    assert set(added_round2) <= set(brief["candidates"][0]["supporting_evidence"])


# --- 外部 corpus 不可用:显式标记 + fallback provenance,不伪造执行 -----------


def test_unavailable_external_skill_records_marker_and_fallback() -> None:
    dt = _dt()
    brief = _build("unavailable-external-skill.yml")
    markers = [
        entry
        for entry in brief["evidence"]
        if entry["observation"].startswith(dt.MARKER_UNAVAILABLE)
    ]
    assert len(markers) == 1
    assert "grilling" in markers[0]["observation"]
    assert "domain-modeling" in markers[0]["observation"]
    # fallback provenance:source 显式描述替代规则与预期出处
    assert "fallback:" in markers[0]["source"]
    assert "grilling/SKILL.md" in markers[0]["source"]
    # 不伪装外部 skill 已执行:不允许出现 applied 标记
    assert not any(
        entry["observation"].startswith(dt.MARKER_APPLIED)
        for entry in brief["evidence"]
    )
    result = validate_brief_data(brief)
    assert result.valid is True
    assert result.recommended_status == "needs_user_decision"


# --- SKILL.md 结构化契约 ------------------------------------------------------


def test_skill_md_frontmatter_and_workflow_anchors() -> None:
    # 契约原因:SKILL.md 的 frontmatter 是 skill loader 契约(name/description);
    # 章节锚点是 external-skill-adapters.md 与本测试套件的交叉引用契约。
    skill_doc = SKILL_DIR / "SKILL.md"
    assert skill_doc.is_file(), f"missing {skill_doc}"
    text = skill_doc.read_text(encoding="utf-8")
    assert text.startswith("---\n"), "SKILL.md 必须以 YAML frontmatter 开头"
    frontmatter = yaml.safe_load(text.split("---", 2)[1])
    assert frontmatter["name"] == "ralph-task-discovery"
    assert str(frontmatter["description"]).strip()
    for anchor in (
        "## 边界",
        "## 工作流",
        "## 状态与停止条件",
        "## 外部 skill 方法边界",
        "## 交接边界",
    ):
        assert anchor in text, f"SKILL.md 缺少章节锚点 {anchor}"


def test_skill_md_relative_references_resolve() -> None:
    # 停止条件(U2):SKILL.md 不得引用不存在的文件路径。
    text = (SKILL_DIR / "SKILL.md").read_text(encoding="utf-8")
    targets = re.findall(r"\]\(([^)#\s]+?)(?:#[^)]*)?\)", text)
    assert targets, "SKILL.md 应至少引用 references/scripts"
    for target in targets:
        if target.startswith(("http://", "https://", "mailto:")):
            continue
        assert not target.startswith("/"), f"SKILL.md 引用必须是仓库内相对路径:{target}"
        path = SKILL_DIR / target
        assert path.exists(), f"SKILL.md 悬空引用:{target}"


# --- external-skill-adapters.md 结构化契约 ------------------------------------


def _adapters_block() -> dict:
    text = (
        SKILL_DIR / "references" / "external-skill-adapters.md"
    ).read_text(encoding="utf-8")
    match = re.search(
        r"<!-- adapters-yaml:start -->(.*?)<!-- adapters-yaml:end -->", text, re.S
    )
    assert match, "external-skill-adapters.md 缺少机器可读 adapters 块"
    # 该块在 markdown 中以 ``` 围栏呈现,解析前剥掉围栏行
    lines = [
        line
        for line in match.group(1).splitlines()
        if not line.strip().startswith("```")
    ]
    return yaml.safe_load("\n".join(lines))


# 8 个外部 skill 的调用分类与 corpus 相对路径:稳定 agent-facing contract。
# 契约原因:分类与外部 corpus README 的 disable-model-invocation 名单一致
# (user-invoked = triage / wayfinder / grill-with-docs / to-spec);路径后缀
# 与 U2 实际读取的外部 corpus 布局一致,是 provenance 的可验证部分。
MODEL_INVOKED = {
    "grilling": "skills/productivity/grilling/SKILL.md",
    "domain-modeling": "skills/engineering/domain-modeling/SKILL.md",
    "diagnosing-bugs": "skills/engineering/diagnosing-bugs/SKILL.md",
    "codebase-design": "skills/engineering/codebase-design/SKILL.md",
}
USER_INVOKED = {
    "triage": "skills/engineering/triage/SKILL.md",
    "wayfinder": "skills/engineering/wayfinder/SKILL.md",
    "grill-with-docs": "skills/engineering/grill-with-docs/SKILL.md",
    "to-spec": "skills/engineering/to-spec/SKILL.md",
}


def test_adapter_rows_cover_all_external_skills() -> None:
    data = _adapters_block()
    rows = {row["skill"]: row for row in data["adapters"]}
    assert set(rows) == set(MODEL_INVOKED) | set(USER_INVOKED)

    # adapter row 必备字段(触发条件/输入/输出/证据等级/fallback/停止条件/
    # provenance/调用限制)——U2 稳定契约。
    required_keys = {
        "skill",
        "invocation_mode",
        "sub_invocation_policy",
        "trigger",
        "inputs",
        "outputs",
        "bound_evidence_level",
        "fallback_if_unavailable",
        "stop_condition",
        "provenance",
    }
    for row in rows.values():
        missing = required_keys - set(row)
        assert not missing, f"adapter row {row.get('skill')} 缺字段 {missing}"

    model = {n for n, r in rows.items() if r["invocation_mode"] == "model_invoked"}
    user = {n for n, r in rows.items() if r["invocation_mode"] == "user_invoked"}
    assert model == set(MODEL_INVOKED)
    assert user == set(USER_INVOKED)
    # user-invoked 流程只吸收规则,禁止静默子调用
    for name in USER_INVOKED:
        assert rows[name]["sub_invocation_policy"] == "absorb-rules-only"
    for name in MODEL_INVOKED:
        assert rows[name]["sub_invocation_policy"] == "invoke-allowed"
    for name, suffix in {**MODEL_INVOKED, **USER_INVOKED}.items():
        assert rows[name]["provenance"].endswith(suffix), name
        assert rows[name]["fallback_if_unavailable"].strip(), name
        assert rows[name]["stop_condition"].strip(), name


# --- agents/openai.yaml 结构化契约 --------------------------------------------


def test_openai_yaml_agent_metadata_shape() -> None:
    # 契约原因:字段形状与 ralph-project-bootstrap/agents/openai.yaml 一致,
    # 是 agent metadata 的稳定加载契约。
    data = yaml.safe_load(
        (SKILL_DIR / "agents" / "openai.yaml").read_text(encoding="utf-8")
    )
    for key in (
        "name",
        "display_name",
        "description",
        "when_to_use",
        "inputs",
        "outputs",
        "boundaries",
        "verification_gates",
    ):
        assert key in data, f"openai.yaml 缺少字段 {key}"
    assert data["name"] == "ralph-task-discovery"
    assert data["inputs"] and data["outputs"]
    assert data["boundaries"] and data["verification_gates"]


# --- task-brief-schema.md 既有锚点 + 新增规则 ----------------------------------


def test_schema_doc_keeps_anchors_and_adds_rules() -> None:
    # 契约原因:task-brief-schema.md 是 validator 使用手册,既有顶层章节锚点
    # 被 SKILL.md 引用不得破坏;新增小节是 U2 Evidence/Decision 扩充规则的
    # 书面化(事实/决策/假设区分、事实不可覆盖、外部 skill provenance、
    # agent 不得替用户确认)。
    text = (SKILL_DIR / "references" / "task-brief-schema.md").read_text(
        encoding="utf-8"
    )
    for anchor in (
        "## 5. Evidence(证据台账)",
        "## 6. Decision Record(决策记录)",
        "## 9. author_ready 的充要条件(全部满足才可交接)",
        "## 11. 稳定错误码与 next_action 词表",
        "## 14. 如何运行验证",
    ):
        assert anchor in text, f"既有章节锚点被破坏:{anchor}"
    for rule_marker in (
        "### 5.1",
        "### 5.2",
        "### 5.3",
        "### 6.1",
        "external_skill_unavailable",
        "external_skill_applied",
        "terminology_conflict",
    ):
        assert rule_marker in text, f"缺少 U2 扩充规则标记:{rule_marker}"


# --- U4:author handoff contract(task brief → ralph-preset-author Workflow 0) ---
#
# 契约原因:ralph-preset-author 以 task brief 为 Workflow 0 的前置输入。
# 行为侧测试使用真实 fixtures + 真实 brief_validator + author_handoff
# (author 应执行的校验序列的确定性参考实现,conftest 注册的 flat module);
# anchor 侧仅断言稳定的 agent-facing contract anchor(task_brief_path 输入 /
# task_brief_invalid 停止输出 / author_ready 校验步骤的存在与顺序 /
# validator 集成指令),不做普通文案包含断言。

AUTHOR_SKILL = Path(__file__).resolve().parents[1] / "ralph-preset-author" / "SKILL.md"
AUTHOR_CHECKLIST = AUTHOR_SKILL.parent / "references" / "author-checklist.md"
HANDOFF_DOC = SKILL_DIR / "references" / "author-handoff.md"
BRIEF_FIXTURES = SKILL_DIR / "fixtures"


def _ah():
    """Import inside the test body so a missing helper surfaces as a
    per-test failure instead of a collection error."""
    import author_handoff  # registered by conftest when the script exists

    return author_handoff


def _author_skill_text() -> str:
    assert AUTHOR_SKILL.is_file(), f"missing {AUTHOR_SKILL}"
    return AUTHOR_SKILL.read_text(encoding="utf-8")


def _workflow0(text: str) -> str:
    """截取 author SKILL.md Workflow 步骤 0 段(Discovery gate,0d 小节之前)。"""
    start = text.index("## Workflow")
    end = text.index("0d.", start)
    return text[start:end]


def _load_brief_fixture(name: str) -> dict:
    return yaml.safe_load((BRIEF_FIXTURES / name).read_text(encoding="utf-8"))


# --- invalid brief 五种情形 → task_brief_invalid,不消费任何事实 ---------------


def test_handoff_missing_brief_file_is_task_brief_invalid(tmp_path) -> None:
    # 验收:缺文件 → task_brief_invalid + author 侧稳定 code;停止语义 = 无事实被消费
    ah = _ah()
    decision = ah.evaluate_task_brief(
        tmp_path / "no-such-brief.yml", "/workspace/demo-repo"
    )
    assert decision.verdict == ah.VERDICT_INVALID
    assert decision.errors[0].code == ah.CODE_FILE_NOT_FOUND
    assert decision.goal is None
    assert decision.selected_candidate_id is None


def test_handoff_invalid_yaml_is_task_brief_invalid(tmp_path) -> None:
    # 验收:坏 YAML → 透传 validator 稳定 code invalid_yaml(真实 yaml.safe_load)
    ah = _ah()
    broken = tmp_path / "broken.yml"
    broken.write_text("schema_version: [unclosed\n", encoding="utf-8")
    decision = ah.evaluate_task_brief(broken, "/workspace/demo-repo")
    assert decision.verdict == ah.VERDICT_INVALID
    assert decision.errors[0].code == "invalid_yaml"
    assert decision.selected_candidate_id is None


def test_handoff_schema_version_mismatch_is_task_brief_invalid(tmp_path) -> None:
    # 验收:schema_version 不受支持 → validator code schema_version_invalid
    ah = _ah()
    data = _load_brief_fixture("valid.yml")
    data["schema_version"] = "2.0"
    path = tmp_path / "schema-mismatch.yml"
    path.write_text(yaml.safe_dump(data, allow_unicode=True), encoding="utf-8")
    decision = ah.evaluate_task_brief(path, data["project_root"])
    assert decision.verdict == ah.VERDICT_INVALID
    assert "schema_version_invalid" in {e.code for e in decision.errors}
    assert decision.selected_candidate_id is None


def test_handoff_root_mismatch_is_task_brief_invalid() -> None:
    # 验收:brief 内部全绿但 project_root 与当前目标项目根不一致(stale)
    # → author 侧 code task_brief_root_mismatch;valid brief 也不得被消费
    ah = _ah()
    decision = ah.evaluate_task_brief(
        BRIEF_FIXTURES / "valid.yml", "/workspace/other-repo"
    )
    assert decision.verdict == ah.VERDICT_INVALID
    assert decision.errors[0].code == ah.CODE_ROOT_MISMATCH
    assert decision.goal is None
    assert decision.selected_candidate_id is None


def test_handoff_declared_author_ready_without_confirmation_is_invalid(tmp_path) -> None:
    # 验收:author_ready=false(声明 author_ready 但用户确认缺失)
    # → task_brief_invalid + validator code author_ready_gate_violation
    ah = _ah()
    data = _load_brief_fixture("valid.yml")
    data["user_confirmations"]["scope"]["confirmed"] = False
    path = tmp_path / "unconfirmed-scope.yml"
    path.write_text(yaml.safe_dump(data, allow_unicode=True), encoding="utf-8")
    decision = ah.evaluate_task_brief(path, data["project_root"])
    assert decision.verdict == ah.VERDICT_INVALID
    assert "author_ready_gate_violation" in {e.code for e in decision.errors}
    assert decision.selected_candidate_id is None


def test_handoff_blocked_brief_is_task_brief_invalid() -> None:
    # 验收:blocked brief(valid 但未认证)→ task_brief_invalid +
    # author 侧 code task_brief_not_author_ready,禁止 handoff 原因可见
    ah = _ah()
    decision = ah.evaluate_task_brief(
        BRIEF_FIXTURES / "blocked.yml", "/workspace/demo-repo"
    )
    assert decision.verdict == ah.VERDICT_INVALID
    assert decision.errors[0].code == ah.CODE_NOT_AUTHOR_READY
    assert "人工输入" in decision.errors[0].message
    assert decision.selected_candidate_id is None


def test_handoff_error_priority_follows_read_order(tmp_path) -> None:
    # 验收:错误按读取顺序汇报——validator 的 schema 错误先于 author 侧
    # root mismatch(schema 不受支持时 project_root 语义不可信)
    ah = _ah()
    data = _load_brief_fixture("valid.yml")
    data["schema_version"] = "2.0"
    path = tmp_path / "schema-and-root.yml"
    path.write_text(yaml.safe_dump(data, allow_unicode=True), encoding="utf-8")
    decision = ah.evaluate_task_brief(path, "/workspace/other-repo")
    codes = [e.code for e in decision.errors]
    assert "schema_version_invalid" in codes
    assert ah.CODE_ROOT_MISMATCH in codes
    assert codes.index("schema_version_invalid") < codes.index(ah.CODE_ROOT_MISMATCH)


# --- valid author_ready brief → 已确认输入进入 author 既有流程 ----------------


def test_handoff_valid_brief_enters_author_flow_with_confirmed_inputs() -> None:
    # 验收:valid.yml → handoff 成立,Intent Confirmation 所需的 goal / 成功条件 /
    # 阻塞条件 / scope / evidence refs 全部可从 brief 消费
    ah = _ah()
    decision = ah.evaluate_task_brief(
        BRIEF_FIXTURES / "valid.yml", "/workspace/demo-repo"
    )
    assert decision.verdict == ah.VERDICT_OK
    assert decision.errors == ()
    data = _load_brief_fixture("valid.yml")
    assert decision.goal == data["goal"]
    confirmations = data["user_confirmations"]
    assert decision.acceptance_note == confirmations["completion_evidence"]["note"]
    assert decision.failure_boundaries_note == confirmations["failure_boundaries"]["note"]
    assert decision.scope_note == confirmations["scope"]["note"]
    # evidence refs:完整证据台账 + selected 候选的支撑证据引用
    assert set(decision.evidence_ids) == {e["id"] for e in data["evidence"]}
    assert decision.selected_candidate_id == "C1"
    assert decision.selected_candidate_summary == data["candidates"][0]["summary"]
    assert decision.selected_candidate_evidence == tuple(
        data["candidates"][0]["supporting_evidence"]
    )
    # project_root 规范化匹配:尾随斜杠不影响一致性判定
    trailing = ah.evaluate_task_brief(
        BRIEF_FIXTURES / "valid.yml", "/workspace/demo-repo/"
    )
    assert trailing.verdict == ah.VERDICT_OK


def test_handoff_rejected_candidate_is_not_consumed_as_selected() -> None:
    # 验收:coverage-fail.yml 的 C1 被标 selected:true 但 validator 判
    # rejected_insufficient_coverage → 不得被当作 selected 消费;
    # 对照组 alternative.yml 中被 rejected 的 C-A 同样不可消费,只有 C-B 可消费
    ah = _ah()
    failing = ah.evaluate_task_brief(
        BRIEF_FIXTURES / "coverage-fail.yml", "/workspace/demo-repo"
    )
    assert failing.verdict == ah.VERDICT_INVALID
    assert failing.selected_candidate_id is None

    ok = ah.evaluate_task_brief(
        BRIEF_FIXTURES / "alternative.yml", "/workspace/demo-repo"
    )
    assert ok.verdict == ah.VERDICT_OK
    assert ok.selected_candidate_id == "C-B"
    assert ok.selected_candidate_id != "C-A"


# --- author 侧文档契约 anchor -------------------------------------------------


def test_author_skill_workflow0_declares_brief_consumption_contract() -> None:
    # 契约原因:task_brief_path(输入)/ task_brief_invalid(停止输出)/
    # author_ready(校验步骤)是 author Workflow 0 brief 消费协议的稳定
    # agent-facing anchor;顺序(输入 → validator 复核 → 停止判定)以及
    # "invalid 不生成 YAML" 的停止语义是跨 skill 契约,不是普通文案。
    text = _author_skill_text()
    section0 = _workflow0(text)
    for anchor in ("task_brief_path", "task_brief_invalid", "author_ready"):
        assert anchor in section0, (
            f"author SKILL.md Workflow 0 缺少 brief 消费 anchor {anchor}"
        )
    assert section0.index("task_brief_path") < section0.index("task_brief_invalid")
    # validator 集成指令:必须要求运行 brief_validator,不得信任 brief 自我声明
    assert "brief_validator" in section0 or "validate_brief_text" in section0
    # 停止语义:invalid brief 不生成任何 preset YAML
    assert re.search(r"不生成.{0,40}(preset\s*)?YAML", section0), (
        "author SKILL.md Workflow 0 必须声明 invalid brief 不生成 preset YAML"
    )
    # 交接书面协议引用
    assert "author-handoff.md" in section0


def test_author_skill_brief_input_does_not_exempt_existing_gates() -> None:
    # 契约原因:brief 只是已确认输入,不是门禁豁免许可;既有门禁 anchor 必须
    # 仍在原位,且 Workflow 0 必须显式声明 brief 不豁免任何既有门禁。
    text = _author_skill_text()
    section0 = _workflow0(text)
    assert re.search(r"brief\s*不豁免|不豁免.*既有门禁", section0), (
        "author SKILL.md Workflow 0 必须声明 brief 不豁免既有门禁"
    )
    for anchor in (
        "Discovery and user-confirmation gate",
        "Preset Intent Confirmation",
        "Payload Contract",
        "Pre-review gate",
        "ralph-preset-review",
    ):
        assert anchor in text, f"author SKILL.md 既有门禁 anchor 缺失:{anchor}"
    # Intent Confirmation 对 brief 已确认事实的引用项(成功/阻塞/scope/证据)
    for token in ("Goal", "acceptance", "failure boundaries", "scope", "Evidence refs"):
        assert token in section0, f"Intent Confirmation brief 引用项缺失 {token}"


def test_author_checklist_has_task_brief_ssot_reconciliation() -> None:
    # 契约原因:author-checklist.md 是 author 逐项对账清单;task brief SSOT
    # 对账项必须覆盖路径/validator 结论记录、rejected 候选禁消费、
    # 既有门禁不因 brief 跳过。
    text = AUTHOR_CHECKLIST.read_text(encoding="utf-8")
    for anchor in ("task_brief_path", "task_brief_invalid", "candidate_gates"):
        assert anchor in text, f"author-checklist.md 缺少 brief 对账 anchor {anchor}"
    assert re.search(r"rejected.{0,60}不得.{0,60}selected", text, re.S), (
        "author-checklist.md 必须禁止把 rejected 候选当作 selected 消费"
    )
    assert re.search(r"未因\s*brief|不因\s*brief", text), (
        "author-checklist.md 必须声明既有门禁未因 brief 跳过或削弱"
    )


def test_author_handoff_doc_exists_and_defines_contract() -> None:
    # 契约原因:author-handoff.md 是跨 skill 交接的唯一书面协议;读取顺序、
    # stale 判据、停止条件错误码与端到端映射表是 U6 e2e 的结构化基础。
    assert HANDOFF_DOC.is_file(), f"missing {HANDOFF_DOC}"
    text = HANDOFF_DOC.read_text(encoding="utf-8")
    # 读取顺序锚点必须在「消费顺序」段内按协议顺序首次出现
    order_section_start = text.index("消费顺序")
    section = text[order_section_start:]
    order = ("文件存在", "YAML", "schema_version", "project_root", "author_ready")
    positions = []
    for token in order:
        assert token in section, f"author-handoff.md 消费顺序段缺少锚点 {token}"
        positions.append(section.index(token))
    assert positions == sorted(positions), "author-handoff.md 读取顺序锚点顺序错误"
    # 停止条件错误码(task_brief_invalid + validator/author 侧稳定 code)
    for code in (
        "task_brief_invalid",
        "task_brief_file_not_found",
        "task_brief_root_mismatch",
        "task_brief_not_author_ready",
        "schema_version_invalid",
        "invalid_yaml",
        "author_ready_gate_violation",
    ):
        assert code in text, f"author-handoff.md 缺少停止条件 code {code}"
    # stale 判据必须显式定义
    assert "stale" in text.lower(), "author-handoff.md 必须定义 stale brief 判据"
    # 交接只传 brief 路径,不复制长文本进 prompt
    assert re.search(r"只传|不复制长文本", text), (
        "author-handoff.md 必须声明 handoff 只传 brief 路径"
    )
    # 端到端证据/错误映射表(U6 预留结构)
    assert "映射" in text and "U6" in text


# --- U6:端到端 pipeline(transcript → discovery → validator → author handoff) ---
#
# 把 SKILL.md 工作流的确定性实现串成一条链:transcript → build_brief →
# brief YAML 落盘 → validate_brief_text(真实 yaml.safe_load)→
# author_handoff.evaluate_task_brief(文件路径 + 目标项目根)。链在测试内
# 组装,不新增生产模块。断言语义:状态、分数、Evidence ID、候选状态、
# next action、handoff verdict;每一阶段的失败都必须阻断后续 author 消费
# (invalid 裁定断言「无任何已确认事实被输出」)。


@dataclass(frozen=True)
class PipelineRun:
    """一次端到端 pipeline 运行的完整产物(brief 数据 + 落盘路径 +
    validator 裁定 + author handoff 裁定)。"""

    label: str
    brief_path: Path
    brief_data: dict
    validation: object  # brief_validator.ValidationResult
    decision: object  # author_handoff.HandoffDecision


def _pipeline_finish(
    label: str, brief: dict, text: str, target_root: str, tmp_path: Path
) -> PipelineRun:
    """brief YAML 落盘后,validator 走文本入口、handoff 走文件路径入口。"""
    brief_path = tmp_path / f"{label.removesuffix('.yml')}-brief.yml"
    brief_path.write_text(text, encoding="utf-8")
    validation = validate_brief_text(brief_path.read_text(encoding="utf-8"))
    decision = _ah().evaluate_task_brief(brief_path, target_root)
    return PipelineRun(
        label=label,
        brief_path=brief_path,
        brief_data=brief,
        validation=validation,
        decision=decision,
    )


def _pipeline_run_from_transcript(name: str, tmp_path: Path) -> PipelineRun:
    """transcript → discovery mapping → brief 落盘 → validator → handoff。"""
    transcript = _transcript(name)
    brief = _dt().build_brief(transcript)
    text = yaml.safe_dump(brief, allow_unicode=True)
    return _pipeline_finish(name, brief, text, transcript["project_root"], tmp_path)


def _pipeline_run_from_brief_fixture(name: str, tmp_path: Path) -> PipelineRun:
    """brief fixture 原文 → 落盘 → validator → handoff(目标根取 brief 自身
    project_root,使裁定只取决于门禁语义而非 stale 根)。"""
    text = (BRIEF_FIXTURES / name).read_text(encoding="utf-8")
    brief = yaml.safe_load(text)
    return _pipeline_finish(name, brief, text, brief["project_root"], tmp_path)


def test_pipeline_green_flow_transcript_to_handoff_ok(tmp_path) -> None:
    # green transcript → discovery 收敛 author_ready → validator 认证
    # ready_for_handoff → author handoff 裁定可交接,已确认事实被消费。
    ah = _ah()
    run = _pipeline_run_from_transcript("green.yml", tmp_path)
    brief, validation, decision = run.brief_data, run.validation, run.decision

    # 阶段 1(discovery):状态收敛 + 事实/决策齐全
    assert brief["status"] == "author_ready"
    assert all(score == pytest.approx(0.85) for score in brief["confidence"].values())
    ledger = {entry["id"]: entry["level"] for entry in brief["evidence"]}
    assert {"E1", "E2", "E3"} <= set(ledger), "项目事实调查产生 E1-E3 证据"
    assert {d["id"]: d["resolved"] for d in brief["decisions"]} == {
        "D1": True,
        "D2": True,
        "D3": True,
        "D4": True,
    }

    # 阶段 2(validator):认证通过,next_action 可交接
    assert validation.valid is True
    assert validation.author_ready is True
    assert validation.recommended_status == "author_ready"
    assert validation.next_action == "ready_for_handoff"
    assert validation.errors == ()
    assert validation.handoff_block_reasons == ()
    gates = {gate.candidate_id: gate for gate in validation.candidate_gates}
    assert gates["C1"].outcome == "selected"
    assert gates["C1"].failed_gates == ()

    # 阶段 3(author handoff):可交接,消费的已确认事实与 brief 一致
    assert decision.verdict == ah.VERDICT_OK
    assert decision.errors == ()
    assert decision.goal == brief["goal"]
    confirmations = brief["user_confirmations"]
    assert decision.scope_note == confirmations["scope"]["note"]
    assert decision.acceptance_note == confirmations["completion_evidence"]["note"]
    assert decision.failure_boundaries_note == confirmations["failure_boundaries"]["note"]
    assert decision.selected_candidate_id == "C1"
    assert decision.selected_candidate_evidence == tuple(
        brief["candidates"][0]["supporting_evidence"]
    )
    assert set(decision.evidence_ids) == set(ledger)
    # selected 候选的支撑证据含 E3/E4 完成证据(真实执行级别)
    assert {
        ledger[eid] for eid in decision.selected_candidate_evidence
    } & {"E3", "E4"}


def test_pipeline_low_confidence_transcript_never_reaches_author(tmp_path) -> None:
    # 低置信度链:无项目事实/无回答 → 五维全部落入丢弃带 →
    # needs_investigation;validator 不认证;handoff 判 task_brief_invalid,
    # author 不启动(无任何已确认事实被消费)。
    ah = _ah()
    run = _pipeline_run_from_transcript("unknown-project.yml", tmp_path)
    brief, validation, decision = run.brief_data, run.validation, run.decision

    assert brief["status"] == "needs_investigation"
    assert brief["evidence"] == []
    assert brief["candidates"] == []
    assert all(score == pytest.approx(0.50) for score in brief["confidence"].values())

    assert validation.valid is True
    assert validation.author_ready is False
    assert validation.recommended_status == "needs_investigation"
    assert validation.next_action == "rerun_investigation"
    # 五维逐个诚实丢弃(rejected_low_confidence),禁止平均值绕过
    dim_rejections = [r for r in validation.rejections if r.kind == "dimension"]
    assert {r.id for r in dim_rejections} == set(KEY_DIMENSIONS)
    assert all(r.reason == "rejected_low_confidence" for r in dim_rejections)

    assert decision.verdict == ah.VERDICT_INVALID
    assert [e.code for e in decision.errors] == [ah.CODE_NOT_AUTHOR_READY]
    assert decision.goal is None
    assert decision.selected_candidate_id is None
    assert decision.evidence_ids == ()


def test_pipeline_medium_confidence_brief_stops_at_handoff(tmp_path) -> None:
    # 中置信度链:project_fact_coverage 0.84 落调查带 → needs_investigation;
    # brief 内部一致(valid)但未认证 → handoff 拒绝,author 不启动。
    ah = _ah()
    run = _pipeline_run_from_brief_fixture("medium-confidence.yml", tmp_path)
    brief, validation, decision = run.brief_data, run.validation, run.decision

    assert brief["status"] == "needs_investigation"
    assert brief["confidence"]["project_fact_coverage"] == pytest.approx(0.84)
    # 其余四维全部达标:单维度短板不被其它维度补偿
    others = {
        dim: score
        for dim, score in brief["confidence"].items()
        if dim != "project_fact_coverage"
    }
    assert all(score >= 0.85 for score in others.values())

    assert validation.valid is True
    assert validation.author_ready is False
    assert validation.recommended_status == "needs_investigation"
    assert validation.next_action == "rerun_investigation"

    assert decision.verdict == ah.VERDICT_INVALID
    assert [e.code for e in decision.errors] == [ah.CODE_NOT_AUTHOR_READY]
    assert decision.goal is None
    assert decision.selected_candidate_id is None
    assert decision.evidence_ids == ()


def test_pipeline_rejected_candidate_flow_only_selected_reaches_author(tmp_path) -> None:
    # 多候选链:C-A coverage 不足被 rejected(rejection reason 保留),
    # 替代候选 C-B 达标后才继续 → validator 认证 author_ready,
    # handoff 只消费 C-B,rejected 候选不得进入交接消费。
    ah = _ah()
    run = _pipeline_run_from_transcript("alternative-candidates.yml", tmp_path)
    brief, validation, decision = run.brief_data, run.validation, run.decision

    assert brief["status"] == "author_ready"
    candidates = {c["id"]: c for c in brief["candidates"]}
    assert candidates["C-A"]["status"] == "rejected_insufficient_coverage"
    assert candidates["C-A"]["rejection_reason"], "被淘汰候选的 rejection reason 必须保留"
    assert candidates["C-B"]["status"] == "selected"

    gates = {gate.candidate_id: gate for gate in validation.candidate_gates}
    assert gates["C-A"].outcome == "rejected_insufficient_coverage"
    assert gates["C-A"].failed_gates == ("acceptance_coverage",)
    assert gates["C-B"].outcome == "selected"
    assert gates["C-B"].failed_gates == ()
    assert validation.author_ready is True
    assert validation.next_action == "ready_for_handoff"

    assert decision.verdict == ah.VERDICT_OK
    assert decision.selected_candidate_id == "C-B"
    assert decision.selected_candidate_id != "C-A"
    assert decision.selected_candidate_summary == candidates["C-B"]["summary"]
    assert decision.selected_candidate_evidence == tuple(
        candidates["C-B"]["supporting_evidence"]
    )
    assert decision.goal == brief["goal"]


def test_pipeline_blocked_flow_reports_gaps_and_refuses_handoff(tmp_path) -> None:
    # 三轮失败链:blocked brief → validator 报告缺失维度/已尝试候选/人工动作,
    # 不建议第四轮自动调查;handoff 拒绝且不伪造任何 launch/preset 输入。
    ah = _ah()
    run = _pipeline_run_from_brief_fixture("blocked.yml", tmp_path)
    brief, validation, decision = run.brief_data, run.validation, run.decision

    assert brief["status"] == "blocked"
    assert brief["attempt_count"] == 3
    assert validation.valid is True
    assert validation.author_ready is False
    assert validation.recommended_status == "blocked"
    assert validation.next_action == "confirm_with_user"

    reasons = validation.handoff_block_reasons
    # 缺失证据/维度逐条列出(五维门禁未过 + 无达标候选)
    assert any("0.85" in reason for reason in reasons)
    # 已尝试候选及其结论可追溯
    assert any("C1=needs_investigation" in reason for reason in reasons)
    # 人工动作:只等人工输入,不建议第四轮自动调查
    assert any("人工输入" in reason for reason in reasons)
    assert any("不得自动开启第四轮调查" in reason for reason in reasons)

    assert decision.verdict == ah.VERDICT_INVALID
    assert [e.code for e in decision.errors] == [ah.CODE_NOT_AUTHOR_READY]
    assert "人工输入" in decision.errors[0].message
    assert "status=blocked" in decision.errors[0].message
    # 不伪造 launch/preset 输入:无任何可消费事实
    assert decision.goal is None
    assert decision.selected_candidate_id is None
    assert decision.selected_candidate_evidence == ()
    assert decision.evidence_ids == ()


def test_pipeline_external_unavailable_flow_records_fallback_and_refuses_handoff(
    tmp_path,
) -> None:
    # 外部 corpus 不可用链:static fallback + provenance 显式记录,不伪造已执行;
    # 置信度不被静默改动,阻断来自未确认主题 → validator 不认证 → handoff 拒绝。
    dt = _dt()
    ah = _ah()
    run = _pipeline_run_from_transcript("unavailable-external-skill.yml", tmp_path)
    brief, validation, decision = run.brief_data, run.validation, run.decision

    markers = [
        entry
        for entry in brief["evidence"]
        if entry["observation"].startswith(dt.MARKER_UNAVAILABLE)
    ]
    assert len(markers) == 1
    assert "grilling" in markers[0]["observation"]
    assert "domain-modeling" in markers[0]["observation"]
    # static fallback 与预期 provenance 同时出现;不伪装外部 skill 已执行
    assert markers[0]["source"].startswith("fallback:")
    assert "grilling/SKILL.md" in markers[0]["source"]
    assert not any(
        entry["observation"].startswith(dt.MARKER_APPLIED)
        for entry in brief["evidence"]
    )

    # confidence impact:五维保持达标带(未因 fallback 虚增或扣减),
    # 阻断原因是用户确认缺失而不是分数
    assert all(score == pytest.approx(0.85) for score in brief["confidence"].values())
    assert brief["status"] == "needs_user_decision"
    assert validation.valid is True
    assert validation.author_ready is False
    assert validation.recommended_status == "needs_user_decision"
    assert validation.next_action == "confirm_with_user"
    assert any("确认记录" in reason for reason in validation.handoff_block_reasons)

    assert decision.verdict == ah.VERDICT_INVALID
    assert [e.code for e in decision.errors] == [ah.CODE_NOT_AUTHOR_READY]
    assert decision.goal is None
    assert decision.evidence_ids == ()


def test_pipeline_bug_flow_requires_red_capable_e3_e4_before_author_ready(tmp_path) -> None:
    # bug 链对照:无 red-capable 回路 → needs_investigation、无候选、handoff 拒绝;
    # 补上 red-capable replay 事实(E4)后 acceptance 证据等级提升到 E3/E4,
    # selected 候选含完成证据才 author_ready → handoff 可交接。
    dt = _dt()
    ah = _ah()

    without = _pipeline_run_from_transcript("bug-without-loop.yml", tmp_path)
    assert without.brief_data["status"] == "needs_investigation"
    assert without.brief_data["candidates"] == []
    assert without.brief_data["confidence"]["acceptance_evidence"] == pytest.approx(0.75)
    assert without.validation.valid is True
    assert without.validation.author_ready is False
    assert without.validation.recommended_status == "needs_investigation"
    assert without.validation.next_action == "rerun_investigation"
    assert without.decision.verdict == ah.VERDICT_INVALID
    assert without.decision.selected_candidate_id is None
    assert without.decision.evidence_ids == ()

    with_loop = _pipeline_run_from_transcript("bug-with-red-loop.yml", tmp_path)
    brief, validation, decision = (
        with_loop.brief_data,
        with_loop.validation,
        with_loop.decision,
    )
    # red-capable 回路事实进入环境问题清单后,该 transcript 无遗留开放问题
    assert dt.open_questions(_transcript("bug-with-red-loop.yml")) == ()
    assert brief["status"] == "author_ready"
    assert brief["confidence"]["acceptance_evidence"] == pytest.approx(0.85)
    assert validation.valid is True
    assert validation.author_ready is True
    assert validation.recommended_status == "author_ready"
    assert validation.next_action == "ready_for_handoff"
    gates = {gate.candidate_id: gate for gate in validation.candidate_gates}
    assert gates["C1"].outcome == "selected"
    # selected 候选支撑证据必须含 E3/E4 完成证据(实际执行/独立验收级别)
    ledger = {entry["id"]: entry["level"] for entry in brief["evidence"]}
    selected = brief["candidates"][0]
    selected_levels = {ledger[eid] for eid in selected["supporting_evidence"]}
    assert selected_levels & {"E3", "E4"}
    assert "E4" in ledger.values(), "red-capable replay 事实以 E4 等级进入台账"

    assert decision.verdict == ah.VERDICT_OK
    assert decision.selected_candidate_id == "C1"
    assert decision.selected_candidate_evidence == tuple(selected["supporting_evidence"])
    assert decision.goal == brief["goal"]


# 每一阶段的失败都必须阻断后续 author 消费:validator 不认证 →
# handoff 判 invalid → invalid 裁定不携带任何已确认事实。
FAILING_PIPELINE_SOURCES = (
    ("transcript", "unknown-project.yml"),  # 低置信度:五维丢弃带
    ("transcript", "bug-without-loop.yml"),  # bug 无 red-capable 回路
    ("transcript", "unavailable-external-skill.yml"),  # 外部不可用 + 未确认主题
    ("transcript", "vague-success-criteria.yml"),  # 模糊回答 → 未决 blocking 决策
    ("transcript", "conflicting-doc.yml"),  # 术语冲突未裁决
    ("brief", "blocked.yml"),  # 三轮耗尽
    ("brief", "medium-confidence.yml"),  # 单维度调查带
    ("brief", "low-confidence.yml"),  # 单维度丢弃带
    ("brief", "coverage-fail.yml"),  # 覆盖门禁失败 + 虚假 author_ready 声明
    ("brief", "ambiguous-selected.yml"),  # 候选等分歧义
    ("brief", "conflicting-evidence.yml"),  # 矛盾证据未裁决
)


@pytest.mark.parametrize("kind, name", FAILING_PIPELINE_SOURCES)
def test_pipeline_failure_short_circuit_blocks_author_consumption(
    kind: str, name: str, tmp_path
) -> None:
    ah = _ah()
    run = (
        _pipeline_run_from_transcript(name, tmp_path)
        if kind == "transcript"
        else _pipeline_run_from_brief_fixture(name, tmp_path)
    )
    assert run.validation.author_ready is False, name
    assert run.decision.verdict == ah.VERDICT_INVALID, name
    assert run.decision.errors, name
    # invalid 裁定不输出任何已确认事实(停止语义)
    assert run.decision.goal is None, name
    assert run.decision.scope_note is None, name
    assert run.decision.acceptance_note is None, name
    assert run.decision.failure_boundaries_note is None, name
    assert run.decision.selected_candidate_id is None, name
    assert run.decision.selected_candidate_summary is None, name
    assert run.decision.selected_candidate_evidence == (), name
    assert run.decision.evidence_ids == (), name


def test_pipeline_validator_errors_propagate_through_handoff(tmp_path) -> None:
    # validator 结构错误必须原样透传到 handoff 裁定(不被吞、不降级):
    # 覆盖门禁失败 + 虚假 author_ready 声明;候选等分歧义。
    ah = _ah()
    failing = _pipeline_run_from_brief_fixture("coverage-fail.yml", tmp_path)
    codes = [error.code for error in failing.decision.errors]
    assert "candidate_coverage_gate_failed" in codes
    assert "author_ready_gate_violation" in codes
    assert failing.decision.verdict == ah.VERDICT_INVALID

    ambiguous = _pipeline_run_from_brief_fixture("ambiguous-selected.yml", tmp_path)
    codes = [error.code for error in ambiguous.decision.errors]
    assert "ambiguous_selected_candidates" in codes
    assert ambiguous.decision.verdict == ah.VERDICT_INVALID
