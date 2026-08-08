"""Stable result values for the plugin-owned Memory writer."""

from __future__ import annotations

import dataclasses
from typing import Any, Mapping


@dataclasses.dataclass(frozen=True)
class MemoryWriteResult:
    """A bounded, serialisable outcome of one writer attempt."""

    result: str
    reason: str
    memory_digest: str = ""
    memory_id: str | None = None
    policy_version: str = ""
    source: Mapping[str, Any] = dataclasses.field(default_factory=dict)
    retryable: bool = False

    def as_dict(self) -> dict[str, Any]:
        return {
            "result": self.result,
            "reason": self.reason,
            "memory_digest": self.memory_digest,
            "memory_id": self.memory_id,
            "policy_version": self.policy_version,
            "source": dict(self.source),
            "retryable": self.retryable,
        }
