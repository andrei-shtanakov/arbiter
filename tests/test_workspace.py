"""Tests that verify the Rust workspace infrastructure is set up correctly."""

import subprocess


def test_cargo_build() -> None:
    """Verify that the Rust workspace compiles."""
    result = subprocess.run(
        ["cargo", "build", "--workspace"],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert result.returncode == 0, f"cargo build failed:\n{result.stderr}"


def test_cargo_test() -> None:
    """Verify that all Rust tests pass."""
    result = subprocess.run(
        ["cargo", "test", "--workspace"],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert result.returncode == 0, f"cargo test failed:\n{result.stderr}"


def test_cargo_clippy() -> None:
    """Verify that clippy passes with no warnings."""
    result = subprocess.run(
        ["cargo", "clippy", "--workspace", "--", "-D", "warnings"],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert result.returncode == 0, f"cargo clippy failed:\n{result.stderr}"


def test_cargo_fmt() -> None:
    """Verify that code is properly formatted."""
    result = subprocess.run(
        ["cargo", "fmt", "--all", "--", "--check"],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode == 0, f"cargo fmt check failed:\n{result.stdout}"
