"""Python MCP client for the Arbiter policy engine.

Manages an Arbiter subprocess over stdio, implementing the JSON-RPC 2.0
protocol for task routing, outcome reporting, and agent status queries.
"""

from __future__ import annotations

import asyncio
import json
import logging
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)


class ArbiterError(Exception):
    """Base error for Arbiter client operations."""


class ArbiterConnectionError(ArbiterError):
    """Raised when the connection to the Arbiter subprocess is broken."""


class ArbiterProtocolError(ArbiterError):
    """Raised when a JSON-RPC protocol error occurs."""

    def __init__(self, code: int, message: str, data: Any = None) -> None:
        self.code = code
        self.rpc_message = message
        self.data = data
        super().__init__(f"JSON-RPC error {code}: {message}")


@dataclass
class ArbiterClientConfig:
    """Configuration for ArbiterClient."""

    binary_path: str | Path = "target/release/arbiter-mcp"
    tree_path: str | Path = "models/agent_policy_tree.json"
    config_dir: str | Path = "config/"
    db_path: str | Path | None = None
    log_level: str = "warn"
    reconnect_delay: float = 1.0
    max_reconnect_attempts: int = 3


class ArbiterClient:
    """MCP client that manages an Arbiter subprocess.

    Communicates over stdin/stdout using JSON-RPC 2.0. One JSON object
    per line. Supports automatic reconnection on broken pipe.

    Usage::

        client = ArbiterClient(ArbiterClientConfig(binary_path="..."))
        await client.start()
        decision = await client.route_task("task-1", {
            "type": "bugfix", "language": "python",
            "complexity": "simple", "priority": "normal",
        })
        await client.stop()
    """

    def __init__(self, config: ArbiterClientConfig | None = None) -> None:
        self._config = config or ArbiterClientConfig()
        self._process: asyncio.subprocess.Process | None = None
        self._request_id: int = 0
        self._started: bool = False
        self._db_path: Path | None = None
        self._temp_db: tempfile.NamedTemporaryFile | None = None

    @property
    def is_running(self) -> bool:
        """Check if the subprocess is currently running."""
        return self._process is not None and self._process.returncode is None

    async def start(self) -> dict[str, Any]:
        """Start the Arbiter subprocess and perform MCP handshake.

        Returns:
            Server capabilities from the initialize response.

        Raises:
            ArbiterConnectionError: If the subprocess fails to start.
            ArbiterProtocolError: If the handshake fails.
        """
        if self._started and self.is_running:
            raise ArbiterError("Client already started")

        await self._spawn_process()
        result = await self._handshake()
        self._started = True
        return result

    async def stop(self) -> None:
        """Gracefully shut down the Arbiter subprocess.

        Closes stdin to signal EOF, waits for the process to exit.
        """
        if self._process is None:
            return

        proc = self._process
        self._process = None
        self._started = False

        try:
            if proc.stdin is not None:
                proc.stdin.close()
                await proc.stdin.wait_closed()
        except (BrokenPipeError, ConnectionResetError, OSError):
            pass

        try:
            await asyncio.wait_for(proc.wait(), timeout=5.0)
        except asyncio.TimeoutError:
            logger.warning("Arbiter process did not exit, killing")
            proc.kill()
            await proc.wait()
        finally:
            if self._temp_db is not None:
                try:
                    self._temp_db.close()
                except OSError:
                    pass
                self._temp_db = None

    async def route_task(
        self,
        task_id: str,
        task: dict[str, Any],
        constraints: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Route a coding task to the best agent.

        Args:
            task_id: Unique task identifier.
            task: Task description with type, language, complexity, priority.
            constraints: Optional routing constraints.

        Returns:
            Decision dict with chosen_agent, confidence, invariant_checks.

        Raises:
            ArbiterProtocolError: On JSON-RPC error response.
            ArbiterConnectionError: On broken pipe (after retry).
        """
        arguments: dict[str, Any] = {"task_id": task_id, "task": task}
        if constraints is not None:
            arguments["constraints"] = constraints
        return await self._call_tool("route_task", arguments)

    async def report_outcome(
        self,
        task_id: str,
        agent_id: str,
        status: str,
        **kwargs: Any,
    ) -> dict[str, Any]:
        """Report the outcome of a task execution.

        Args:
            task_id: Task identifier from route_task.
            agent_id: Agent that executed the task.
            status: One of success, failure, timeout, cancelled.
            **kwargs: Optional fields (duration_min, tokens_used, etc.).

        Returns:
            Outcome result with updated_stats.
        """
        arguments: dict[str, Any] = {
            "task_id": task_id,
            "agent_id": agent_id,
            "status": status,
            **kwargs,
        }
        return await self._call_tool("report_outcome", arguments)

    async def get_agent_status(
        self,
        agent_id: str | None = None,
    ) -> dict[str, Any]:
        """Query agent capabilities, load, and performance.

        Args:
            agent_id: Specific agent to query, or None for all.

        Returns:
            Status dict with agents list.
        """
        arguments: dict[str, Any] = {}
        if agent_id is not None:
            arguments["agent_id"] = agent_id
        return await self._call_tool("get_agent_status", arguments)

    # ------------------------------------------------------------------
    # Internal methods
    # ------------------------------------------------------------------

    async def _spawn_process(self) -> None:
        """Spawn the Arbiter subprocess with pipes."""
        binary = str(self._config.binary_path)
        if not Path(binary).is_absolute():
            binary = str(Path.cwd() / binary)

        if not Path(binary).exists():
            raise ArbiterConnectionError(f"Binary not found: {binary}")

        # Use configured db_path or create a temp file
        if self._config.db_path is not None:
            db_path = str(self._config.db_path)
        else:
            self._temp_db = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
            db_path = self._temp_db.name
        self._db_path = Path(db_path)

        cmd = [
            binary,
            "--tree",
            str(self._config.tree_path),
            "--config",
            str(self._config.config_dir),
            "--db",
            db_path,
            "--log-level",
            self._config.log_level,
        ]

        logger.debug("Spawning: %s", " ".join(cmd))

        try:
            self._process = await asyncio.create_subprocess_exec(
                *cmd,
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
        except (FileNotFoundError, PermissionError) as e:
            raise ArbiterConnectionError(f"Failed to start Arbiter: {e}") from e

    async def _handshake(self) -> dict[str, Any]:
        """Perform MCP initialize + initialized handshake."""
        result = await self._send_request("initialize", {})
        await self._send_notification("initialized")
        return result

    async def _call_tool(
        self,
        name: str,
        arguments: dict[str, Any],
    ) -> dict[str, Any]:
        """Call an MCP tool with automatic reconnection on broken pipe.

        Returns the parsed tool result (inner JSON from content[0].text).
        """
        try:
            return await self._call_tool_once(name, arguments)
        except ArbiterConnectionError:
            logger.warning(
                "Broken pipe, reconnecting in %.1fs",
                self._config.reconnect_delay,
            )
            await self._reconnect()
            return await self._call_tool_once(name, arguments)

    async def _call_tool_once(
        self,
        name: str,
        arguments: dict[str, Any],
    ) -> dict[str, Any]:
        """Single attempt to call an MCP tool."""
        raw = await self._send_request(
            "tools/call",
            {"name": name, "arguments": arguments},
        )

        # Extract inner JSON from MCP content wrapper
        if "content" in raw and isinstance(raw["content"], list):
            text = raw["content"][0].get("text", "{}")
            return json.loads(text)
        return raw

    async def _send_request(
        self,
        method: str,
        params: dict[str, Any],
    ) -> dict[str, Any]:
        """Send a JSON-RPC request and wait for the response."""
        self._request_id += 1
        msg = {
            "jsonrpc": "2.0",
            "id": self._request_id,
            "method": method,
            "params": params,
        }
        return await self._send_and_receive(msg)

    async def _send_notification(self, method: str) -> None:
        """Send a JSON-RPC notification (no response expected)."""
        msg = {
            "jsonrpc": "2.0",
            "method": method,
        }
        await self._write_message(msg)

    async def _send_and_receive(
        self,
        msg: dict[str, Any],
    ) -> dict[str, Any]:
        """Write a message and read the response."""
        await self._write_message(msg)
        response = await self._read_response()

        if "error" in response and response["error"] is not None:
            err = response["error"]
            raise ArbiterProtocolError(
                code=err.get("code", -32000),
                message=err.get("message", "Unknown error"),
                data=err.get("data"),
            )

        return response.get("result", {})

    async def _write_message(self, msg: dict[str, Any]) -> None:
        """Write a JSON message as a single line to the subprocess stdin."""
        if self._process is None or self._process.stdin is None:
            raise ArbiterConnectionError("Not connected")

        line = json.dumps(msg, separators=(",", ":")) + "\n"
        try:
            self._process.stdin.write(line.encode())
            await self._process.stdin.drain()
        except (BrokenPipeError, ConnectionResetError, OSError) as e:
            raise ArbiterConnectionError(f"Write failed: {e}") from e

    async def _read_response(self) -> dict[str, Any]:
        """Read a single JSON-RPC response line from stdout."""
        if self._process is None or self._process.stdout is None:
            raise ArbiterConnectionError("Not connected")

        try:
            line = await asyncio.wait_for(
                self._process.stdout.readline(),
                timeout=30.0,
            )
        except asyncio.TimeoutError as e:
            raise ArbiterConnectionError("Read timeout") from e

        if not line:
            raise ArbiterConnectionError("Process exited unexpectedly")

        try:
            return json.loads(line.decode())
        except json.JSONDecodeError as e:
            raise ArbiterProtocolError(
                code=-32700,
                message=f"Invalid JSON response: {e}",
            ) from e

    async def _reconnect(self) -> None:
        """Reconnect to the Arbiter subprocess after a broken pipe."""
        attempts = 0
        while attempts < self._config.max_reconnect_attempts:
            attempts += 1
            await asyncio.sleep(self._config.reconnect_delay)
            logger.info(
                "Reconnect attempt %d/%d",
                attempts,
                self._config.max_reconnect_attempts,
            )
            try:
                # Kill old process if still alive
                if self._process is not None:
                    try:
                        self._process.kill()
                        await self._process.wait()
                    except (ProcessLookupError, OSError):
                        pass
                    self._process = None

                await self._spawn_process()
                await self._handshake()
                logger.info("Reconnected successfully")
                return
            except (ArbiterConnectionError, ArbiterProtocolError) as e:
                logger.warning("Reconnect attempt %d failed: %s", attempts, e)

        raise ArbiterConnectionError(
            f"Failed to reconnect after {self._config.max_reconnect_attempts} attempts"
        )


@dataclass
class FallbackScheduler:
    """Round-robin fallback scheduler for when the Arbiter server
    is unavailable.

    Cycles through a fixed list of agents, assigning tasks in order.
    Useful as a degraded-mode fallback when the decision tree or
    database is unavailable.

    Usage::

        scheduler = FallbackScheduler()
        agent = scheduler.next_agent("task-1")
        # "claude_code"
        agent = scheduler.next_agent("task-2")
        # "codex_cli"
    """

    agents: list[str] = field(
        default_factory=lambda: ["claude_code", "codex_cli", "aider"]
    )
    _index: int = field(default=0, init=False, repr=False)

    def next_agent(self, task_id: str) -> str:
        """Select the next agent in round-robin order.

        Args:
            task_id: Task identifier (logged for traceability).

        Returns:
            Agent ID string.
        """
        if not self.agents:
            raise ArbiterError("No agents configured for fallback")
        agent = self.agents[self._index % len(self.agents)]
        logger.debug("Fallback: task=%s -> agent=%s", task_id, agent)
        self._index += 1
        return agent

    def reset(self) -> None:
        """Reset the round-robin index."""
        self._index = 0
