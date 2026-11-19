#!/usr/bin/env python3
"""Unified dispatcher and path resolver for CodeXray scripts, hooks, and binaries.

Resolution order for CodeXray root:
  1. CODEXRAY_HOME / CODE_XRAY_HOME environment variable
  2. ~/.agents/tooling.json ("codeXrayRoot") [override registry path via TOOLING_JSON]
  3. Default: ~/.agents/code-xray
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

DEFAULT_CODEXRAY_ROOT = Path.home() / ".agents" / "code-xray"
DEFAULT_TOOLING_JSON = Path.home() / ".agents" / "tooling.json"

TargetKind = Literal["script", "hook", "binary", "dotnet", "path"]

HOOK_ALIASES: dict[str, str] = {
    "stop": "stop_guard.py",
    "stop-guard": "stop_guard.py",
    "stop_guard": "stop_guard.py",
    "stop_guard.py": "stop_guard.py",
    "pre-command": "pre_command_guard.py",
    "pre-command-guard": "pre_command_guard.py",
    "pre_command_guard": "pre_command_guard.py",
    "pre_command_guard.py": "pre_command_guard.py",
    "pre-tool-use": "pre_command_guard.py",
}

KNOWN_BINARIES: frozenset[str] = frozenset(
    {
        "xray-index",
        "xray-grep",
        "structural-clone",
    }
)


@dataclass(frozen=True)
class DispatchTarget:
    """Resolved executable or path target."""

    kind: TargetKind
    path: Path
    command: tuple[str, ...]


def _is_windows() -> bool:
    return os.name == "nt"


def _exe_name(stem: str) -> str:
    if _is_windows() and not stem.lower().endswith(".exe"):
        return f"{stem}.exe"
    return stem


def get_tooling_json_path() -> Path:
    raw = os.environ.get("TOOLING_JSON", "").strip()
    if raw:
        return Path(raw).expanduser()
    return DEFAULT_TOOLING_JSON


def load_tooling_config(config_path: Path | None = None) -> dict[str, str]:
    path = config_path or get_tooling_json_path()
    if not path.is_file():
        return {}
    raw = json.loads(path.read_text(encoding="utf-8-sig"))
    if not isinstance(raw, dict):
        raise ValueError(f"Invalid tooling registry JSON object in {path}")
    return {str(k): str(v) for k, v in raw.items() if isinstance(v, str) and v.strip()}


def resolve_codexray_root() -> Path:
    env_home = (
        os.environ.get("CODEXRAY_HOME", "").strip()
        or os.environ.get("CODE_XRAY_HOME", "").strip()
    )
    if env_home:
        return Path(env_home).expanduser()

    cfg = load_tooling_config()
    if "codeXrayRoot" in cfg:
        return Path(cfg["codeXrayRoot"]).expanduser()

    return DEFAULT_CODEXRAY_ROOT


def _pick_first_existing(candidates: list[Path], default_path: Path) -> Path:
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    return default_path


def resolve_code_xray_mcp_exe() -> Path:
    env_exe = os.environ.get("CODE_XRAY_MCP_EXE", "").strip()
    if env_exe:
        return Path(env_exe).expanduser()

    exe = _exe_name("CodeXrayMcp")
    root = resolve_codexray_root()
    candidates: list[Path] = []

    if not (os.environ.get("CODEXRAY_HOME", "").strip() or os.environ.get("CODE_XRAY_HOME", "").strip()):
        cfg = load_tooling_config()
        if "codeXrayMcpExe" in cfg:
            candidates.append(Path(cfg["codeXrayMcpExe"]).expanduser())

    candidates.extend(
        sorted(
            [
                p
                for p in (
                    root / "bin" / "code-xray-mcp" / exe,
                    root / "bin" / "code-xray-mcp-live" / exe,
                )
                if p.is_file()
            ],
            key=lambda p: p.stat().st_mtime,
            reverse=True,
        )
    )
    candidates.extend(
        [
            root / "bin" / "code-xray-mcp" / exe,
            root / "bin" / "code-xray-mcp-live" / exe,
            root / "bin" / "code-xray-mcp" / "CodeXrayMcp.dll",
            root / "bin" / "code-xray-mcp-live" / "CodeXrayMcp.dll",
        ]
    )
    default_target = candidates[0]
    return _pick_first_existing(candidates, default_target)


def resolve_opengrep_exe(codexray_root: Path) -> Path:
    exe = _exe_name("opengrep")
    candidates = [
        codexray_root / "bin" / "opengrep" / exe,
        codexray_root / "bin" / exe,
    ]
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    found = shutil.which("opengrep")
    if found:
        return Path(found)
    return candidates[0]


def _resolve_hook_path(raw: str, codexray_root: Path) -> Path | None:
    norm = raw.replace("\\", "/").strip()
    lower = norm.lower()
    if lower in HOOK_ALIASES:
        return codexray_root / "scripts" / "hooks" / HOOK_ALIASES[lower]
    if lower.startswith(("hooks/", "scripts/hooks/")):
        name = norm.split("/")[-1]
        mapped = HOOK_ALIASES.get(name.lower(), name if name.endswith(".py") else f"{name}.py")
        return codexray_root / "scripts" / "hooks" / mapped
    return None


def _resolve_binary_target(norm: str, codexray_root: Path) -> DispatchTarget | None:
    lower = norm.lower()
    if lower in {"code-xray", "code-xray-mcp", "codexraymcp", "codexraymcp.exe"}:
        mcp_path = resolve_code_xray_mcp_exe()
        if mcp_path.suffix.lower() == ".dll":
            return DispatchTarget("dotnet", mcp_path, ("dotnet", str(mcp_path)))
        return DispatchTarget("binary", mcp_path, (str(mcp_path),))

    if lower in {"opengrep", "opengrep.exe", "bin/opengrep", "bin/opengrep/opengrep.exe"}:
        og_path = resolve_opengrep_exe(codexray_root)
        return DispatchTarget("binary", og_path, (str(og_path),))

    rel_name = norm[4:] if lower.startswith("bin/") else norm
    stem = rel_name[:-4] if rel_name.lower().endswith(".exe") else rel_name
    if lower.startswith("bin/") or lower.endswith(".exe") or stem.lower() in KNOWN_BINARIES:
        bin_path = codexray_root / "bin" / _exe_name(stem)
        return DispatchTarget("binary", bin_path, (str(bin_path),))
    return None


def resolve_target(target: str) -> DispatchTarget:
    """Resolve a script, hook, binary, or subpath inside CodeXray."""
    norm = target.replace("\\", "/").strip()
    lower = norm.lower()
    codexray_root = resolve_codexray_root()

    if lower in {"root", "codexray", "codexray-root"}:
        return DispatchTarget("path", codexray_root, ())

    hook_path = _resolve_hook_path(norm, codexray_root)
    if hook_path is not None:
        return DispatchTarget("hook", hook_path, (sys.executable, str(hook_path)))

    bin_target = _resolve_binary_target(norm, codexray_root)
    if bin_target is not None:
        return bin_target

    if lower.startswith("templates/") or ("/" in norm and not lower.startswith("scripts/")):
        sub = codexray_root / Path(norm)
        return DispatchTarget("path", sub, ())

    script_rel = norm[8:] if lower.startswith("scripts/") else norm
    if not script_rel.lower().endswith(".py"):
        script_rel = f"{script_rel}.py"
    script_path = codexray_root / "scripts" / Path(script_rel)
    if script_path.is_file() or lower.endswith(".py") or lower.startswith("scripts/"):
        return DispatchTarget("script", script_path, (sys.executable, str(script_path)))

    candidate_bin = codexray_root / "bin" / _exe_name(norm)
    if candidate_bin.is_file():
        return DispatchTarget("binary", candidate_bin, (str(candidate_bin),))

    return DispatchTarget("script", script_path, (sys.executable, str(script_path)))


def expand_macro_args(args: list[str], codexray_root: Path) -> list[str]:
    """Expand @codexray/<rel> argument prefixes."""
    expanded: list[str] = []
    for arg in args:
        if arg.startswith(("@codexray/", "@codexray\\")):
            rel = arg[len("@codexray/") :].replace("\\", "/")
            expanded.append(str(codexray_root / Path(rel)))
        else:
            expanded.append(arg)
    return expanded


def print_help() -> None:
    print(
        "\n".join(
            [
                "Usage:",
                "  python ~/.agents/bin/xray_run.py <target> [args...]",
                "  python ~/.agents/bin/xray_run.py --which <target>",
                "  python ~/.agents/bin/xray_run.py --list",
                "",
                "Examples:",
                "  python ~/.agents/bin/xray_run.py check_consistency.py --repo-root .",
                "  python ~/.agents/bin/xray_run.py verify_dotnet.py --repo-root .",
                "  python ~/.agents/bin/xray_run.py verify_rust.py --repo-root .",
                "  python ~/.agents/bin/xray_run.py xray-index slice -r . -q Foo",
                "  python ~/.agents/bin/xray_run.py xray-grep -r . -p Pattern --save-review",
                "  python ~/.agents/bin/xray_run.py grep_review.py list --repo-root .",
                "  python ~/.agents/bin/xray_run.py --which code-xray",
                "",
                "Resolution order:",
                "  CODEXRAY_HOME env -> ~/.agents/tooling.json -> ~/.agents/code-xray",
            ]
        )
    )


def list_targets() -> int:
    cx_root = resolve_codexray_root()
    mcp_exe = resolve_code_xray_mcp_exe()
    print(f"tooling.json   : {get_tooling_json_path()}")
    print(f"codeXrayRoot   : {cx_root} (exists={cx_root.is_dir()})")
    print(f"codeXrayMcpExe : {mcp_exe} (exists={mcp_exe.is_file()})")

    scripts_dir = cx_root / "scripts"
    if scripts_dir.is_dir():
        scripts = sorted(p.name for p in scripts_dir.glob("*.py"))
        print(f"scripts ({len(scripts)}): {', '.join(scripts)}")

    hooks_dir = scripts_dir / "hooks"
    if hooks_dir.is_dir():
        hooks = sorted(p.name for p in hooks_dir.glob("*.py"))
        print(f"hooks   ({len(hooks)}): {', '.join(hooks)}")

    bin_dir = cx_root / "bin"
    if bin_dir.is_dir():
        bins = sorted(p.name for p in bin_dir.iterdir() if p.is_file())
        print(f"bins    ({len(bins)}): {', '.join(bins)}")
    return 0


def main(argv: list[str] | None = None) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")

    args = list(sys.argv[1:] if argv is None else argv)
    if not args or args[0] in {"-h", "--help", "help"}:
        print_help()
        return 0

    if args[0] in {"--list", "list"}:
        return list_targets()

    if args[0] in {"--root", "--print-root"}:
        print(resolve_codexray_root())
        return 0

    if args[0] in {"--which", "--resolve", "which", "resolve"}:
        if len(args) < 2:
            print("Error: --which requires a <target> argument.", file=sys.stderr)
            return 2
        resolved = resolve_target(args[1])
        print(resolved.path)
        return 0 if resolved.path.exists() else 1

    target_name = args[0]
    if target_name == "--":
        if len(args) < 2:
            print_help()
            return 2
        target_name = args[1]
        rest = args[2:]
    else:
        rest = args[1:]

    resolved = resolve_target(target_name)
    if resolved.kind == "path":
        print(resolved.path)
        return 0 if resolved.path.exists() else 1

    if not resolved.path.exists():
        print(
            f"[xray_run] Target not found: {resolved.path} (codeXrayRoot={resolve_codexray_root()})",
            file=sys.stderr,
        )
        return 1

    forwarded = expand_macro_args(rest, resolve_codexray_root())
    cmd = [*resolved.command, *forwarded]
    proc = subprocess.run(cmd, check=False)
    return proc.returncode


if __name__ == "__main__":
    raise SystemExit(main())
