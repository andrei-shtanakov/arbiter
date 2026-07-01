"""Tests for scripts/gen_agents_scaffold.py (ADR-ECO-003 #5).

The scaffold generator projects the catalog's routable=true set onto
agents.toml *section keys only* — it must never invent policy values.
"""

from __future__ import annotations

import subprocess
import sys
import tomllib
from pathlib import Path

import pytest

from scripts.gen_agents_scaffold import (
    load_routable_ids,
    reconcile,
    render_scaffold,
    render_stub,
    split_sections,
)

CATALOG = """
[models."m1"]
vendor = "v"
status = "active"

[models."m2"]
vendor = "v"
status = "active"

[models."mnew"]
vendor = "v"
status = "active"

[harnesses.h1]
routable = true

[[agents]]
harness = "h1"
model = "m1"
tested = true
routable = true

[[agents]]
harness = "h1"
model = "mnew"
tested = true
routable = true

[[agents]]
harness = "h1"
model = "m2"
tested = true
routable = false
"""

AGENTS_TOML = """# preamble comment
["h1@m1"]
display_name = "Existing One"
cost_per_hour = 0.30
max_concurrent = 2

["h1@mstale"]
display_name = "No longer routable"
cost_per_hour = 0.99

[aider]
display_name = "legacy bare id"
"""


def test_load_routable_ids_ordered_and_filtered() -> None:
    """Only routable=true pairs, in catalog order, as fused ids."""
    catalog = tomllib.loads(CATALOG)
    assert load_routable_ids(catalog) == ["h1@m1", "h1@mnew"]


def test_split_sections_marks_fused_ignores_bare() -> None:
    """Quoted ["h@m"] sections carry a fused id; bare [aider] does not."""
    sections = split_sections(AGENTS_TOML)
    fused = [s.fused for s in sections]
    assert "h1@m1" in fused
    assert "h1@mstale" in fused
    # bare [aider] header present but not a fused id
    assert any(s.fused is None and "[aider]" in s.text for s in sections)


def test_render_stub_is_keys_only_no_invented_policy() -> None:
    """A new-key stub is a header plus commented TODOs — never live values."""
    stub = render_stub("h1@mnew")
    lines = [ln for ln in stub.splitlines() if ln.strip()]
    assert lines[0] == '["h1@mnew"]'
    assert all(ln.lstrip().startswith("#") for ln in lines[1:]), (
        "stub must not emit any uncommented policy value"
    )


def test_reconcile_partitions_kept_new_stale() -> None:
    """Known→kept, catalog-only→new, present-but-not-routable→stale."""
    routable = ["h1@m1", "h1@mnew"]
    sections = split_sections(AGENTS_TOML)
    result = reconcile(routable, sections)
    assert result.kept == ["h1@m1"]
    assert result.new == ["h1@mnew"]
    assert result.stale == ["h1@mstale"]


def test_reconcile_preserves_known_section_verbatim() -> None:
    """A kept key re-emits its existing policy fields untouched."""
    routable = ["h1@m1", "h1@mnew"]
    sections = split_sections(AGENTS_TOML)
    scaffold = render_scaffold(reconcile(routable, sections))
    assert "cost_per_hour = 0.30" in scaffold
    assert "max_concurrent = 2" in scaffold
    # the new key appears as a stub with a TODO marker
    assert '["h1@mnew"]' in scaffold
    assert "TODO" in scaffold


def test_script_runs_against_vendored_catalog() -> None:
    """End-to-end: script exits 0 and prints both live routable keys."""
    repo = Path(__file__).resolve().parent.parent
    if not (repo / "config" / "agents-catalog.toml").exists():
        pytest.skip("vendored config/agents-catalog.toml not present in this checkout")
    result = subprocess.run(
        [sys.executable, "scripts/gen_agents_scaffold.py"],
        capture_output=True,
        text=True,
        timeout=60,
        cwd=repo,
    )
    assert result.returncode == 0, f"scaffold script failed:\n{result.stderr}"
    assert "claude_code@claude-sonnet-4-6" in result.stdout
    assert "codex_cli@gpt-5.5" in result.stdout
