#!/usr/bin/env python3
"""Low-overhead Antigravity/Cursor hook dispatcher for CodeXray hooks.

Dispatches hook scripts (stop_guard.py, pre_command_guard.py) via runpy in-process
by default to minimize hook latency, while resolving the hook path through
CODEXRAY_HOME -> ~/.agents/tooling.json -> ~/.agents/code-xray.
"""

from __future__ import annotations

import json
import os
import runpy
import subprocess
import sys
from pathlib import Path

BIN_DIR = Path(__file__).resolve().parent
if str(BIN_DIR) not in sys.path:
    sys.path.insert(0, str(BIN_DIR))

from xray_run import HOOK_ALIASES, resolve_codexray_root  # noqa: E402


def _fail_open_json(hook_name: str) -> str:
    lower = hook_name.lower()
    if "stop" in lower:
        return json.dumps({"decision": "stop"})
    return json.dumps({"decision": "allow"})


def resolve_hook_script(hook_name: str) -> Path:
    root = resolve_codexray_root()
    norm = hook_name.replace("\\", "/").split("/")[-1].strip()
    mapped = HOOK_ALIASES.get(norm.lower())
    if mapped is None:
        mapped = norm if norm.lower().endswith(".py") else f"{norm}.py"
    return root / "scripts" / "hooks" / mapped


def _print_help() -> None:
    print(
        "\n".join(
            [
                "Usage:",
                "  python ~/.agents/bin/codexray-hook.py <hook-name> [hook-args...]",
                "  python ~/.agents/bin/codexray-hook.py --which <hook-name>",
                "",
                "Hooks:",
                "  stop_guard (aliases: stop, stop-guard)",
                "  pre_command_guard (aliases: pre-command, pre-command-guard, pre-tool-use)",
            ]
        )
    )


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if not args or args[0] in {"-h", "--help"}:
        _print_help()
        return 0

    if args[0] in {"--which", "--resolve"}:
        if len(args) < 2:
            print("Error: --which requires a <hook-name> argument.", file=sys.stderr)
            return 2
        target = resolve_hook_script(args[1])
        print(target)
        return 0 if target.is_file() else 1

    hook_name = args[0]
    hook_args = args[1:]
    hook_path = resolve_hook_script(hook_name)

    if not hook_path.is_file():
        print(f"[codexray-hook] Hook script not found: {hook_path}", file=sys.stderr)
        print(_fail_open_json(hook_name))
        return 0

    if os.environ.get("CODEXRAY_HOOK_SUBPROCESS", "").strip() == "1":
        proc = subprocess.run([sys.executable, str(hook_path), *hook_args], check=False)
        return proc.returncode

    saved_argv = sys.argv[:]
    try:
        sys.argv = [str(hook_path), *hook_args]
        runpy.run_path(str(hook_path), run_name="__main__")
        return 0
    except SystemExit as exc:
        code = exc.code
        return code if isinstance(code, int) else 0
    except Exception as exc:
        print(f"[codexray-hook] Unhandled hook error ({hook_path.name}): {exc}", file=sys.stderr)
        print(_fail_open_json(hook_name))
        return 0
    finally:
        sys.argv = saved_argv


if __name__ == "__main__":
    raise SystemExit(main())
