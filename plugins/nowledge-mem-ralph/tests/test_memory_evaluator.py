"""U05 contract test for the evaluator subagent boundary."""

from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
EVALUATOR = ROOT / "plugins/nowledge-mem-ralph/agents/memory-evaluator.md"


def test_evaluator_contract_is_structured_and_write_free():
    text = EVALUATOR.read_text(encoding="utf-8")
    assert '"verdict": "ACCEPTED|REJECTED|NEEDS_REWRITE"' in text
    assert "不要执行 nmem" in text
    assert "不要读取 transcript" in text
    assert "不要写文件" in text
