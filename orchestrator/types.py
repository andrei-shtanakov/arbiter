"""Typed DTOs for Arbiter route/outcome/status responses.

Frozen dataclasses that parse raw dicts returned by the MCP protocol
into strongly-typed Python objects.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass(frozen=True)
class InvariantCheck:
    """Single invariant rule check result."""

    rule: str
    severity: str
    passed: bool
    detail: str


@dataclass(frozen=True)
class RouteDecision:
    """Result of a route_task call."""

    task_id: str
    action: str
    chosen_agent: str
    confidence: float
    reasoning: str
    decision_path: list[str]
    invariant_checks: list[InvariantCheck]
    metadata: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RouteDecision:
        """Parse a raw route_task response dict."""
        checks = [
            InvariantCheck(
                rule=c["rule"],
                severity=c["severity"],
                passed=c["passed"],
                detail=c.get("detail", ""),
            )
            for c in data.get("invariant_checks", [])
        ]
        return cls(
            task_id=data["task_id"],
            action=data["action"],
            chosen_agent=data.get("chosen_agent", ""),
            confidence=data.get("confidence", 0.0),
            reasoning=data.get("reasoning", ""),
            decision_path=data.get("decision_path", []),
            invariant_checks=checks,
            metadata=data.get("metadata", {}),
        )


@dataclass(frozen=True)
class UpdatedStats:
    """Agent statistics returned after reporting an outcome."""

    agent_id: str
    total_tasks: int
    success_rate: float
    avg_duration_min: float
    avg_cost_usd: float


@dataclass(frozen=True)
class OutcomeResult:
    """Result of a report_outcome call."""

    task_id: str
    recorded: bool
    updated_stats: UpdatedStats
    retrain_suggested: bool
    warnings: list[str]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> OutcomeResult:
        """Parse a raw report_outcome response dict."""
        stats_raw = data.get("updated_stats", {})
        stats = UpdatedStats(
            agent_id=stats_raw.get("agent_id", ""),
            total_tasks=stats_raw.get("total_tasks", 0),
            success_rate=stats_raw.get("success_rate", 0.0),
            avg_duration_min=stats_raw.get("avg_duration_min", 0.0),
            avg_cost_usd=stats_raw.get("avg_cost_usd", 0.0),
        )
        return cls(
            task_id=data["task_id"],
            recorded=data.get("recorded", False),
            updated_stats=stats,
            retrain_suggested=data.get("retrain_suggested", False),
            warnings=data.get("warnings", []),
        )


@dataclass(frozen=True)
class AgentCapabilities:
    """Capabilities declared by an agent."""

    languages: list[str]
    task_types: list[str]
    max_concurrent: int
    cost_per_hour: float


@dataclass(frozen=True)
class AgentStatusInfo:
    """Status information for a single agent."""

    id: str
    display_name: str
    state: str
    capabilities: AgentCapabilities
    active_tasks: int
    total_completed: int
    success_rate: float
    avg_duration_min: float

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> AgentStatusInfo:
        """Parse a raw agent status dict."""
        caps_raw = data.get("capabilities", {})
        caps = AgentCapabilities(
            languages=caps_raw.get("languages", []),
            task_types=caps_raw.get("task_types", []),
            max_concurrent=caps_raw.get("max_concurrent", 0),
            cost_per_hour=caps_raw.get("cost_per_hour", 0.0),
        )
        return cls(
            id=data["id"],
            display_name=data.get("display_name", ""),
            state=data.get("state", "unknown"),
            capabilities=caps,
            active_tasks=data.get("active_tasks", 0),
            total_completed=data.get("total_completed", 0),
            success_rate=data.get("success_rate", 0.0),
            avg_duration_min=data.get("avg_duration_min", 0.0),
        )
