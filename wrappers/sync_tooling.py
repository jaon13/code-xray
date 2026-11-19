#!/usr/bin/env python3
"""Single-source-of-truth tooling path sync from ~/.agents/tooling.json.

Updates (never touching ~/.gemini/config/config.json):
  1. ~/.agents/tooling.json (when CLI override flags are passed)
  2. ~/.gemini/config/mcp_config.json & ~/.cursor/mcp.json (code-xray server entry)
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any

HOME = Path.home()
AGENTS_DIR = HOME / ".agents"
TOOLING_JSON_PATH = AGENTS_DIR / "tooling.json"
GEMINI_MCP_PATH = HOME / ".gemini" / "config" / "mcp_config.json"
CURSOR_MCP_PATH = HOME / ".cursor" / "mcp.json"

HERE = Path(__file__).resolve().parent
BIN_DIR = HERE / "bin"
for entry in (str(BIN_DIR), str(HERE)):
    if entry not in sys.path:
        sys.path.insert(0, entry)

from xray_run import resolve_code_xray_mcp_exe  # noqa: E402


def load_registry(path: Path = TOOLING_JSON_PATH) -> dict[str, Any]:
    if not path.is_file():
        return {}
    data = json.loads(path.read_text(encoding="utf-8-sig"))
    if not isinstance(data, dict):
        raise ValueError(f"Expected JSON object in {path}")
    return {str(k): v for k, v in data.items()}


def save_registry(data: dict[str, Any], path: Path = TOOLING_JSON_PATH) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")


def _build_mcp_command(target: Path) -> tuple[str, list[str]]:
    if target.suffix.lower() == ".dll":
        return "dotnet", [str(target)]
    return str(target), []


def sync_mcp_file(
    cfg_path: Path,
    code_xray_exe: Path,
    *,
    include_env: bool,
    check_only: bool,
) -> bool:
    """Sync code-xray command in an MCP config file. Returns True if drifted/changed."""
    if not cfg_path.is_file():
        return False

    data: dict[str, Any] = json.loads(cfg_path.read_text(encoding="utf-8-sig"))
    servers = data.setdefault("mcpServers", {})
    if not isinstance(servers, dict):
        return False

    changed = False
    if "roslyn-graph" in servers:
        if not check_only:
            servers.pop("roslyn-graph", None)
        changed = True

    cmd, args = _build_mcp_command(code_xray_exe)
    current = servers.get("code-xray")
    if not isinstance(current, dict):
        current = {}
        servers["code-xray"] = current
        changed = True
    if current.get("command") != cmd or current.get("args", []) != args:
        current["command"] = cmd
        current["args"] = args
        if include_env and "env" not in current:
            current["env"] = {}
        changed = True

    if changed and not check_only:
        cfg_path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")
    return changed


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Sync ~/.agents/tooling.json paths to MCP configs.")
    parser.add_argument("--codexray-root", help="Override codeXrayRoot in tooling.json")
    parser.add_argument("--codexray-mcp-exe", help="Override codeXrayMcpExe in tooling.json")
    parser.add_argument("--check", action="store_true", help="Verify paths and sync status without modifying files")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    reg = load_registry()

    updates = {
        "codeXrayRoot": args.codexray_root,
        "codeXrayMcpExe": args.codexray_mcp_exe,
    }
    reg_changed = False
    for key, value in updates.items():
        if value and reg.get(key) != value:
            reg[key] = value
            reg_changed = True

    if reg_changed and not args.check:
        save_registry(reg)
        print(f"[tooling] Updated {TOOLING_JSON_PATH}")

    os.environ["TOOLING_JSON"] = str(TOOLING_JSON_PATH)
    code_xray_exe = (
        Path(str(reg["codeXrayMcpExe"]))
        if isinstance(reg.get("codeXrayMcpExe"), str) and reg["codeXrayMcpExe"]
        else resolve_code_xray_mcp_exe()
    )

    gemini_drifted = sync_mcp_file(GEMINI_MCP_PATH, code_xray_exe, include_env=True, check_only=args.check)
    cursor_drifted = sync_mcp_file(CURSOR_MCP_PATH, code_xray_exe, include_env=False, check_only=args.check)

    if args.check:
        missing = [
            k
            for k in ("codeXrayRoot", "codeXrayMcpExe")
            if not isinstance(reg.get(k), str) or not Path(str(reg[k])).exists()
        ]
        if missing or gemini_drifted or cursor_drifted:
            print(
                f"[check] Drift/missing detected: missing={missing}, "
                f"gemini_mcp={gemini_drifted}, cursor_mcp={cursor_drifted}",
                file=sys.stderr,
            )
            return 1
        print("[check] All tooling paths and MCP configs are in sync.")
        return 0

    print(f"[mcp] gemini={gemini_drifted}, cursor={cursor_drifted}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
