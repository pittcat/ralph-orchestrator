"""ralph-task-discovery discovery 工作流 e2e 测试(U2)。

被测对象:

* ``fixtures/transcripts/*.yml`` —— deterministic discovery transcript
  (项目事实 + 用户回答序列);
* ``scripts/discovery_transcript.py`` —— SKILL.md 工作流规则的确定性实现
  (transcript → task brief mapping),由 conftest 以 flat module 注册;
* U1 的 ``brief_validator`` —— 对生成的 brief 做硬门禁裁定;
* ``SKILL.md`` / ``references/external-skill-adapters.md`` /
  ``agents/openai.yaml`` / ``references/task-brief-schema.md`` 的结构化契约。

测试语义(HARD):断言的是状态、Evidence ID、决策记录与 next_action,
不做"markdown 包含某词"式的文案断言。仅有的结构化词汇断言针对稳定
agent-facing contract,且每处都在注释中说明契约原因。
"""
from __future__ import annotations

import re
from pathlib import Path

import pytest
import yaml

from brief_validator import validate_brief_data, validate_brief_text

SKILL_DIR = Path(__file__).resolve().parents[1] / "ralph-task-discovery"
TRANSCRIPTS = SKILL_DIR / "fixtures" / "transcripts"

TRANSCRIPT_FIXTURES = (
    "green.yml",
    "vague-success-criteria.yml",
    "conflicting-doc.yml",
    "bug-without-loop.yml",
    "unknown-project.yml",
    "unavailable-external-skill.yml",
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
