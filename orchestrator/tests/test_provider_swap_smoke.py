"""Provider-swap smoke test (PP-103 acceptance (c), ADR-ECO-003b last mile).

Observable guarantee: swapping the provider model is a catalog edit, not a
consumer change. The scenario retires active model X and promotes
replacement Y **only** by editing the user-config catalog (via explicit
``$ATP_CATALOG`` — the single resolution layer shared by all three
loaders), plus the sanctioned arbiter-side operator step:
``gen_agents_scaffold.py`` -> apply ``agents.toml`` -> restart.

Consumer invariant: the client (Maestro's vendored ``arbiter_client.py``)
is byte-identical between phases — same binary path, same config paths,
same ``route_task`` payload. Only the catalog content (and the
scaffold-applied ``agents.toml``) changes. Criterion: ``route_task``
chooses X before the edit and Y after it.

Also covers the fail-loud half of PP-103: a server started while
``agents.toml`` still references the retired model must refuse to start
with a conformance Check 5 diagnostic.
"""

from __future__ import annotations

import asyncio
import os
import subprocess
import sys
from pathlib import Path

import pytest

from orchestrator.arbiter_client import ArbiterClient, ArbiterClientConfig

PROJECT_ROOT = Path(__file__).resolve().parent.parent.parent

_BINARY_CANDIDATES = [
    PROJECT_ROOT / "target" / "release" / "arbiter-mcp",
    PROJECT_ROOT / "target" / "debug" / "arbiter-mcp",
]
SCAFFOLD_SCRIPT = PROJECT_ROOT / "scripts" / "gen_agents_scaffold.py"

# One fixed vendor per model id (works around the known Maestro
# models-duplicate-vendor-detection defect — PP-103 fixture constraint).
CATALOG_V1 = """\
[models."model-x"]
vendor = "acme"
status = "active"

[models."model-y"]
vendor = "bmce"
status = "active"

[harnesses.swapper]
kind = "cli"
shim = "shims/swapper.py"
routable = true

[[agents]]
harness = "swapper"
model = "model-x"
tested = true
routable = true
"""

# The provider swap: retire X, promote Y. Nothing else changes.
CATALOG_V2 = """\
[models."model-x"]
vendor = "acme"
status = "retired"

[models."model-y"]
vendor = "bmce"
status = "active"

[harnesses.swapper]
kind = "cli"
shim = "shims/swapper.py"
routable = true

[[agents]]
harness = "swapper"
model = "model-y"
tested = true
routable = true
"""

AGENT_POLICY_FIELDS = """\
display_name = "Swapper ({model})"
supports_languages = ["python"]
supports_types = ["bugfix"]
max_concurrent = 1
cost_per_hour = 0.10
avg_duration_min = 5.0
"""

# The consumer-side request: byte-identical in both phases.
TASK = {
    "type": "bugfix",
    "language": "python",
    "complexity": "simple",
    "priority": "normal",
}


def _find_binary() -> Path:
    for candidate in _BINARY_CANDIDATES:
        if candidate.exists():
            return candidate
    pytest.skip("Arbiter binary not found. Run: cargo build --bin arbiter-mcp")
    raise AssertionError("unreachable")  # for type checker


def _agent_section(agent_id: str) -> str:
    """Hand-author the policy fields for a scaffolded key (operator step)."""
    model = agent_id.split("@", 1)[1]
    return f'["{agent_id}"]\n{AGENT_POLICY_FIELDS.format(model=model)}'


def _write_config_dir(config_dir: Path, agents_toml: str) -> None:
    config_dir.mkdir(exist_ok=True)
    (config_dir / "agents.toml").write_text(agents_toml, encoding="utf-8")
    # Real invariant thresholds; no authority.toml (feature off) so plain
    # route_task calls are not rejected fail-closed.
    invariants = (PROJECT_ROOT / "config" / "invariants.toml").read_text(
        encoding="utf-8"
    )
    (config_dir / "invariants.toml").write_text(invariants, encoding="utf-8")


def _run_scaffold(catalog: Path, agents_toml: Path) -> tuple[str, str]:
    """Run gen_agents_scaffold.py; return (stdout scaffold, stderr report)."""
    proc = subprocess.run(
        [
            sys.executable,
            str(SCAFFOLD_SCRIPT),
            "--catalog",
            str(catalog),
            "--agents-toml",
            str(agents_toml),
        ],
        capture_output=True,
        text=True,
        timeout=60,
        check=True,
    )
    return proc.stdout, proc.stderr


async def _route_once(config: ArbiterClientConfig) -> str:
    """Start a fresh server, route TASK once, stop; return the chosen agent."""
    client = ArbiterClient(config)
    await client.start()
    try:
        decision = await client.route_task_typed("swap-smoke", dict(TASK))
        assert decision.action == "assign", decision
        return decision.chosen_agent
    finally:
        await client.stop()


@pytest.mark.asyncio
async def test_provider_swap_is_a_catalog_edit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Retire X + promote Y via catalog edit flips route_task X -> Y."""
    binary = _find_binary()

    catalog_path = tmp_path / "agents-catalog.toml"
    config_dir = tmp_path / "config"
    agents_toml = config_dir / "agents.toml"

    # Phase 1: catalog v1, agents.toml enrolls X only.
    catalog_path.write_text(CATALOG_V1, encoding="utf-8")
    _write_config_dir(config_dir, _agent_section("swapper@model-x"))
    monkeypatch.setenv("ATP_CATALOG", str(catalog_path))

    # Missing tree -> degraded round-robin; the smoke is about the
    # catalog/config plumbing, not DT internals.
    client_config = ArbiterClientConfig(
        binary_path=binary,
        tree_path=tmp_path / "no_tree.json",
        config_dir=config_dir,
        log_level="warn",
    )

    before = await _route_once(client_config)
    assert before == "swapper@model-x"

    # The provider swap: ONLY the catalog file content changes.
    catalog_path.write_text(CATALOG_V2, encoding="utf-8")

    # Sanctioned operator step: scaffold keys from the new catalog...
    scaffold, report = _run_scaffold(catalog_path, agents_toml)
    assert '["swapper@model-y"]' in scaffold, scaffold
    assert "swapper@model-y" in report and "new" in report, report
    assert "swapper@model-x" in report and "stale" in report, report

    # ...then hand-author policy for the NEW key and drop the STALE
    # section (policy fields are never generated, per ADR-ECO-003).
    agents_toml.write_text(_agent_section("swapper@model-y"), encoding="utf-8")

    # Restart (same client config, same request payload — the consumer
    # side is byte-identical between phases).
    after = await _route_once(client_config)
    assert after == "swapper@model-y"


def test_server_fails_loud_on_retired_reference(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """agents.toml referencing a retired catalog model refuses to start."""
    binary = _find_binary()

    catalog_path = tmp_path / "agents-catalog.toml"
    config_dir = tmp_path / "config"

    # Catalog already swapped to v2, but agents.toml still enrolls X.
    catalog_path.write_text(CATALOG_V2, encoding="utf-8")
    _write_config_dir(config_dir, _agent_section("swapper@model-x"))
    monkeypatch.setenv("ATP_CATALOG", str(catalog_path))

    proc = subprocess.run(
        [
            str(binary),
            "--tree",
            str(tmp_path / "no_tree.json"),
            "--config",
            str(config_dir),
            "--db",
            str(tmp_path / "smoke.db"),
            "--log-level",
            "warn",
        ],
        env=os.environ.copy(),
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert proc.returncode == 1, proc.stderr
    assert "retired model 'model-x'" in proc.stderr, proc.stderr
    assert "Check 5" in proc.stderr, proc.stderr


def test_consistent_pair_starts_normally(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The same server with a consistent pair completes the MCP handshake."""
    binary = _find_binary()

    catalog_path = tmp_path / "agents-catalog.toml"
    config_dir = tmp_path / "config"

    catalog_path.write_text(CATALOG_V1, encoding="utf-8")
    _write_config_dir(config_dir, _agent_section("swapper@model-x"))
    monkeypatch.setenv("ATP_CATALOG", str(catalog_path))

    config = ArbiterClientConfig(
        binary_path=binary,
        tree_path=tmp_path / "no_tree.json",
        config_dir=config_dir,
        log_level="warn",
    )

    async def handshake() -> None:
        client = ArbiterClient(config)
        result = await client.start()
        try:
            assert "protocolVersion" in result
        finally:
            await client.stop()

    asyncio.run(handshake())
