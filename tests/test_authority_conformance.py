"""Conformance checks for authority.toml vs the agents-catalog (RD-006)."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent / "scripts"))

from check_authority_conformance import check, pattern_matches, valid_pattern

CATALOG = {
    "models": {
        "claude-sonnet-4-6": {"vendor": "anthropic", "status": "active"},
        "old-model": {"vendor": "x", "status": "retired"},
    },
    "harnesses": {"claude_code": {}, "codex_cli": {}},
    "agents": [
        {"harness": "claude_code", "model": "claude-sonnet-4-6", "routable": True},
        {"harness": "claude_code", "model": "old-model", "routable": False},
        {"harness": "codex_cli", "model": "claude-sonnet-4-6", "routable": False},
    ],
}


def authority(rules: list[dict]) -> dict:
    return {"version": 1, "unknown_context": "deny", "rules": rules}


def test_conformant_policy_is_clean() -> None:
    a = authority(
        [{"role": "implement", "phase": "execution", "agents": ["claude_code@*"]}]
    )
    assert check(a, CATALOG) == []


def test_unknown_harness_is_finding() -> None:
    a = authority(
        [{"role": "implement", "phase": "execution", "agents": ["gemini_cli@*"]}]
    )
    findings = check(a, CATALOG)
    assert any("gemini_cli" in f and "not in the agents-catalog" in f for f in findings)


def test_pattern_matching_no_routable_agent_is_finding() -> None:
    # codex_cli exists as a harness but has no routable agent in this catalog.
    a = authority(
        [{"role": "implement", "phase": "execution", "agents": ["codex_cli@*"]}]
    )
    findings = check(a, CATALOG)
    assert any("matches no routable agent" in f for f in findings)


def test_pattern_matching_retired_model_is_finding() -> None:
    a = authority(
        [{"role": "implement", "phase": "execution", "agents": ["claude_code@old-model"]}]
    )
    findings = check(a, CATALOG)
    assert any("retired model" in f for f in findings)


def test_arbitrary_glob_is_finding() -> None:
    a = authority(
        [{"role": "implement", "phase": "execution", "agents": ["claude_code@son*"]}]
    )
    findings = check(a, CATALOG)
    assert any("invalid pattern" in f for f in findings)


def test_unknown_role_and_phase_are_findings() -> None:
    a = authority([{"role": "root", "phase": "deploy", "agents": ["claude_code@*"]}])
    findings = check(a, CATALOG)
    assert any("unknown role 'root'" in f for f in findings)
    assert any("unknown phase 'deploy'" in f for f in findings)


def test_pattern_helpers() -> None:
    assert valid_pattern("claude_code@*")
    assert valid_pattern("claude_code@claude-sonnet-4-6")
    assert not valid_pattern("*")
    assert not valid_pattern("aider")
    assert pattern_matches("claude_code@*", "claude_code", "anything")
    assert not pattern_matches("claude_code@*", "codex_cli", "anything")
