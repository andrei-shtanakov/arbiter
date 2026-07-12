"""Authority policy conformance check (RD-006, design §4).

Validates an ``authority.toml`` allowlist against the vendored agents-catalog:

- roles/phases come from the closed v1 vocabularies;
- agent patterns are exactly ``harness@model`` or ``harness@*``;
- every harness named by a pattern exists in the catalog;
- every pattern matches at least one ``routable = true`` agent;
- no exact pattern pins a ``retired`` model (wildcards are covered by the
  routable requirement: the catalog itself CI-fails agents referencing
  retired models).

stdlib-only (tomllib), mirrors scripts/check_routable_gate.py conventions.
Exit codes: 0 clean, 1 findings, 2 usage/config error.

Usage:
    python scripts/check_authority_conformance.py \
        --authority config/authority.toml \
        --catalog config/agents-catalog.toml
"""

from __future__ import annotations

import argparse
import sys
import tomllib
from pathlib import Path

ROLES = ("decompose", "implement", "review", "benchmark")
PHASES = ("authoring", "execution", "merge", "pr")
UNKNOWN_CONTEXT = ("deny", "allow")


def load_toml(path: Path) -> dict:
    """Parse a TOML file or exit 2 (config error)."""
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        print(f"config error: cannot read {path}: {exc}", file=sys.stderr)
        raise SystemExit(2) from exc


def valid_pattern(pattern: str) -> bool:
    """Exactly two forms: exact harness@model or harness@* (design §4)."""
    if "@" not in pattern:
        return False
    harness, _, model = pattern.partition("@")
    if not harness or "*" in harness or not model:
        return False
    return model == "*" or "*" not in model


def pattern_matches(pattern: str, harness: str, model: str) -> bool:
    p_harness, _, p_model = pattern.partition("@")
    if p_harness != harness:
        return False
    return p_model == "*" or p_model == model


def check(authority: dict, catalog: dict) -> list[str]:
    """Return findings; empty means conformant."""
    findings: list[str] = []

    if not isinstance(authority.get("version"), int):
        findings.append("authority: 'version' must be an integer")
    unknown = authority.get("unknown_context", "deny")
    if unknown not in UNKNOWN_CONTEXT:
        findings.append(
            f"authority: unknown_context '{unknown}' (expected one of {UNKNOWN_CONTEXT})"
        )

    harnesses = set(catalog.get("harnesses", {}))
    model_status = {
        name: entry.get("status", "active")
        for name, entry in catalog.get("models", {}).items()
    }
    agents = [
        (a.get("harness", ""), a.get("model", ""), bool(a.get("routable", False)))
        for a in catalog.get("agents", [])
    ]

    for i, rule in enumerate(authority.get("rules", [])):
        where = f"rules[{i}]"
        role, phase = rule.get("role"), rule.get("phase")
        if role not in ROLES:
            findings.append(f"{where}: unknown role '{role}' (expected one of {ROLES})")
        if phase not in PHASES:
            findings.append(f"{where}: unknown phase '{phase}' (expected one of {PHASES})")
        patterns = rule.get("agents", [])
        if not patterns:
            findings.append(f"{where}: empty agents list")
        for pattern in patterns:
            if not valid_pattern(pattern):
                findings.append(
                    f"{where}: invalid pattern '{pattern}' "
                    f"(only exact 'harness@model' or 'harness@*')"
                )
                continue
            harness = pattern.partition("@")[0]
            if harness not in harnesses:
                findings.append(
                    f"{where}: pattern '{pattern}': harness '{harness}' "
                    f"is not in the agents-catalog"
                )
                continue
            matched = [
                (h, m) for (h, m, routable) in agents
                if routable and pattern_matches(pattern, h, m)
            ]
            if not matched:
                findings.append(
                    f"{where}: pattern '{pattern}' matches no routable agent"
                )
            # Retired check applies to exact pins only: a wildcard's safety
            # is covered by the routable requirement (the catalog's own
            # invariant already CI-fails agents referencing retired models).
            p_model = pattern.partition("@")[2]
            if p_model != "*" and model_status.get(p_model) == "retired":
                findings.append(
                    f"{where}: pattern '{pattern}' pins retired model '{p_model}'"
                )
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--authority", type=Path, required=True)
    parser.add_argument("--catalog", type=Path, required=True)
    args = parser.parse_args(argv)

    findings = check(load_toml(args.authority), load_toml(args.catalog))
    for finding in findings:
        print(f"error: {finding}")
    if findings:
        return 1
    print("ok: authority policy conforms to the catalog")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
