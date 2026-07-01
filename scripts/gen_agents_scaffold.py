"""Scaffold agents.toml section *keys* from the SSOT catalog (ADR-ECO-003 #5).

Reads the vendored `config/agents-catalog.toml` and the current
`config/agents.toml`, then prints a reconciled scaffold of the fused
`["<harness>@<model>"]` sections for every `routable = true` pair:

  - KEEP  — a routable key already present: its policy fields are preserved
            untouched (surrounding whitespace may be normalized; this tool
            never rewrites policy values).
  - NEW   — a routable key with no section yet: emitted as a header plus
            commented `# TODO(policy)` placeholders for a human to fill.
  - STALE — a fused section present but no longer routable in the catalog:
            reported (with a `# STALE` marker) so it can be removed by hand.

Keys-only by design: the catalog decides *which* agents are routable; arbiter
decides *how* to weight them. Policy values (cost_per_hour, supports_types, …)
stay hand-authored — see ADR-ECO-003 "Генерация по репам". Legacy bare `[aider]`
sections (pre-convention, not fused) are ignored, matching the devtools
`check-agent-id-conformance.py` rule.

READ-ONLY: prints the scaffold to stdout and a kept/new/stale report to stderr;
never edits any file. Drift (new/stale keys) is advisory, not an error — exit is
0 on success and non-zero only when the catalog cannot be read or parsed.

Usage:
    uv run python scripts/gen_agents_scaffold.py
    uv run python scripts/gen_agents_scaffold.py --catalog config/agents-catalog.toml \
        --agents-toml config/agents.toml
"""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CATALOG = REPO_ROOT / "config" / "agents-catalog.toml"
DEFAULT_AGENTS_TOML = REPO_ROOT / "config" / "agents.toml"

# A quoted section header ["<harness>@<model>"] — the fused routing key.
_FUSED_HEADER = re.compile(r'^\["([^"]+)"\]\s*$')
# Any TOML table/array-of-tables header start.
_ANY_HEADER = re.compile(r"^\s*\[")


@dataclass
class Section:
    """One TOML section: its verbatim text and the fused id it declares (if any)."""

    fused: str | None
    text: str


@dataclass
class Reconciled:
    """Outcome of matching routable ids against existing agents.toml sections."""

    blocks: list[str] = field(default_factory=list)
    kept: list[str] = field(default_factory=list)
    new: list[str] = field(default_factory=list)
    stale: list[str] = field(default_factory=list)


def load_routable_ids(catalog: dict[str, object]) -> list[str]:
    """Return fused ``harness@model`` ids with ``routable = true``, in catalog order."""
    agents = catalog.get("agents", [])
    if not isinstance(agents, list):
        return []
    return [
        f"{row['harness']}@{row['model']}"
        for row in agents
        if isinstance(row, dict) and row.get("routable")
    ]


def split_sections(agents_toml_text: str) -> list[Section]:
    """Split agents.toml into sections, tagging each with its fused id (or None).

    Text before the first header (preamble comments) is dropped — the scaffold
    is regenerated from sections, not preserved wholesale.
    """
    sections: list[Section] = []
    current: list[str] | None = None
    for line in agents_toml_text.splitlines():
        if _ANY_HEADER.match(line):
            if current is not None:
                sections.append(_make_section(current))
            current = [line]
        elif current is not None:
            current.append(line)
    if current is not None:
        sections.append(_make_section(current))
    return sections


def _make_section(lines: list[str]) -> Section:
    """Build a Section from its raw lines, extracting the fused id if quoted."""
    match = _FUSED_HEADER.match(lines[0].strip())
    return Section(fused=match.group(1) if match else None, text="\n".join(lines))


def render_stub(agent_id: str) -> str:
    """Render a new-key placeholder: header + commented TODO policy fields only."""
    return "\n".join(
        [
            f'["{agent_id}"]',
            "# TODO(policy): catalog marks this pair routable but arbiter has no",
            "# policy for it yet. Fill in and uncomment (values are hand-authored,",
            "# NOT generated — bench data is unrepresentative per ADR-ECO-003):",
            '# display_name = "..."',
            "# supports_languages = [...]",
            '# supports_types = ["review", ...]  # MUST include the routed task_type',
            "# max_concurrent = 1",
            "# cost_per_hour = 0.0",
            "# avg_duration_min = 0.0",
        ]
    )


def reconcile(routable_ids: list[str], sections: list[Section]) -> Reconciled:
    """Partition routable ids into kept/new and flag stale fused sections."""
    by_fused = {s.fused: s for s in sections if s.fused is not None}
    routable_set = set(routable_ids)
    result = Reconciled()
    for agent_id in routable_ids:
        section = by_fused.get(agent_id)
        if section is not None:
            result.kept.append(agent_id)
            result.blocks.append(section.text.rstrip())
        else:
            result.new.append(agent_id)
            result.blocks.append(render_stub(agent_id))
    result.stale = [
        s.fused for s in sections if s.fused is not None and s.fused not in routable_set
    ]
    return result


def render_scaffold(result: Reconciled) -> str:
    """Assemble the stdout scaffold: header + section blocks + stale notes."""
    parts = [
        "# GENERATED scaffold — agents.toml routable section keys (ADR-ECO-003 #5).",
        "# Source: config/agents-catalog.toml (routable=true). Keys only — policy",
        "# fields are hand-authored and preserved untouched for kept sections.",
        "",
    ]
    parts.append("\n\n".join(result.blocks))
    if result.stale:
        parts.append("")
        parts.append(
            "# STALE — present in agents.toml but not routable in catalog; "
            "remove by hand:"
        )
        parts.extend(f'#   ["{aid}"]' for aid in result.stale)
    return "\n".join(parts) + "\n"


def main() -> int:
    """Parse args, reconcile catalog vs agents.toml, print scaffold + report."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--catalog", type=Path, default=DEFAULT_CATALOG)
    parser.add_argument("--agents-toml", type=Path, default=DEFAULT_AGENTS_TOML)
    args = parser.parse_args()

    if not args.catalog.exists():
        print(f"catalog not found: {args.catalog}", file=sys.stderr)
        return 1
    try:
        catalog = tomllib.loads(args.catalog.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        print(f"cannot read catalog {args.catalog}: {exc}", file=sys.stderr)
        return 1
    routable_ids = load_routable_ids(catalog)
    agents_text = (
        args.agents_toml.read_text(encoding="utf-8")
        if args.agents_toml.exists()
        else ""
    )
    result = reconcile(routable_ids, split_sections(agents_text))

    print(render_scaffold(result), end="")

    print(
        f"\nscaffold: {len(result.kept)} kept, {len(result.new)} new, "
        f"{len(result.stale)} stale (of {len(routable_ids)} routable)",
        file=sys.stderr,
    )
    if result.new:
        print(f"  new (fill policy): {result.new}", file=sys.stderr)
    if result.stale:
        print(f"  stale (remove): {result.stale}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
