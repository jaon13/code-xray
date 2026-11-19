#!/usr/bin/env python3
"""Cross-platform Polyglot Codebase X-Ray & Strict Paradigm Gate Orchestrator."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import site
import subprocess
import sys
import sysconfig
from dataclasses import asdict, dataclass
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
TOOLKIT_ROOT = SCRIPT_DIR.parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from rust_xray import scan_rust_workspace  # noqa: E402
from xaml_style_xray import scan_xaml_repo  # noqa: E402

SKIP_DIRS = {
    ".git",
    "node_modules",
    "target",
    "target-final",
    "dist",
    "dist-ext",
    ".svelte-kit",
    "bin",
    "obj",
    "build",
    "__pycache__",
    ".venv",
    "venv",
    ".agents",
}


@dataclass(frozen=True)
class StepResult:
    name: str
    status: str  # "passed" | "failed" | "skipped"
    exit_code: int
    summary: str
    detail: str = ""


def detect_repo_stacks(repo_root: Path) -> dict[str, bool]:
    stacks = {"dotnet": False, "xaml": False, "rust": False, "web": False, "python": False}
    for cur_root, dirs, files in os.walk(repo_root):
        dirs[:] = [d for d in dirs if d.lower() not in SKIP_DIRS and "_wpftmp" not in d.lower()]
        for fname in files:
            low = fname.lower()
            if low.endswith((".csproj", ".sln", ".slnx")):
                stacks["dotnet"] = True
            elif low.endswith(".xaml") and "_wpftmp" not in low:
                stacks["xaml"] = True
            elif low == "cargo.toml":
                stacks["rust"] = True
            elif low in ("package.json", "tsconfig.json") or low.endswith(
                (".ts", ".tsx", ".js", ".jsx", ".svelte", ".css", ".html")
            ):
                stacks["web"] = True
            elif low.endswith(".py"):
                stacks["python"] = True
        if all(stacks.values()):
            break
    return stacks


def _extra_bin_dirs() -> list[Path]:
    dirs: list[Path] = [TOOLKIT_ROOT / "bin", TOOLKIT_ROOT / "bin" / "opengrep", Path(sys.executable).resolve().parent]
    scripts_dir = Path(sys.executable).resolve().parent / "Scripts"
    if scripts_dir.is_dir():
        dirs.append(scripts_dir)
    user_scripts = Path(sysconfig.get_path("scripts", "nt_user" if os.name == "nt" else "posix_user") or "")
    if user_scripts.is_dir():
        dirs.append(user_scripts)
    if site.USER_BASE:
        ub_scripts = Path(site.USER_BASE) / ("Scripts" if os.name == "nt" else "bin")
        if ub_scripts.is_dir():
            dirs.append(ub_scripts)
    return dirs


def _resolve_cmd(name: str) -> str | None:
    suffixes = (".exe", ".cmd", ".bat", ".ps1", "") if os.name == "nt" else ("",)
    for cand_base in (name,):
        for sfx in suffixes:
            hit = shutil.which(f"{cand_base}{sfx}")
            if hit:
                return hit
    for bdir in _extra_bin_dirs():
        for sfx in suffixes:
            cand = bdir / f"{name}{sfx}"
            if cand.is_file():
                return str(cand)
    return None


def _run_step(name: str, cmd: list[str], cwd: Path) -> StepResult:
    proc = subprocess.run(
        cmd,
        cwd=cwd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    out = "\n".join(p for p in (proc.stdout.strip(), proc.stderr.strip()) if p).strip()
    tail = "\n".join(out.splitlines()[-20:]) if out else ""
    passed = proc.returncode == 0
    return StepResult(
        name=name,
        status="passed" if passed else "failed",
        exit_code=proc.returncode,
        summary=f"{'PASS' if passed else 'FAIL'} (exit={proc.returncode})",
        detail=tail,
    )


def _missing_tool_step(name: str, tool: str, strict: bool, hint: str) -> StepResult:
    return StepResult(
        name=name,
        status="failed" if strict else "skipped",
        exit_code=1 if strict else 0,
        summary=f"{'FAIL' if strict else 'SKIPPED'}: '{tool}' not installed ({hint})",
    )


def _run_web_checks(repo_root: Path, strict: bool) -> list[StepResult]:
    results: list[StepResult] = []
    pkg_path = repo_root / "package.json"
    scripts: dict[str, str] = {}
    if pkg_path.is_file():
        try:
            scripts = (json.loads(pkg_path.read_text(encoding="utf-8")).get("scripts") or {})
        except (OSError, json.JSONDecodeError):
            scripts = {}

    npm_cmd = _resolve_cmd("npm")
    npx_cmd = _resolve_cmd("npx")

    web_script_targets = [
        ("Web ESLint", "lint:js", ["eslint", "."], ("eslint.config.js", "eslint.config.mjs", "eslint.config.cjs")),
        ("Web Arch (dependency-cruiser)", "lint:arch", ["depcruise", "src", "--config", ".dependency-cruiser.cjs"], (".dependency-cruiser.cjs", ".dependency-cruiser.js")),
        ("Web CSS (Stylelint)", "lint:css", ["stylelint", "**/*.css"], ("stylelint.config.mjs", "stylelint.config.js", ".stylelintrc.json")),
        ("Web HTML (Markuplint)", "lint:html", ["markuplint", "**/*.html"], (".markuplintrc.json", ".markuplintrc")),
    ]

    for label, script_key, npx_args, config_names in web_script_targets:
        if script_key in scripts and npm_cmd:
            results.append(_run_step(label, [npm_cmd, "run", script_key], repo_root))
        elif any((repo_root / c).is_file() for c in config_names) and npx_cmd:
            results.append(_run_step(label, [npx_cmd, "--no-install", *npx_args], repo_root))
        else:
            results.append(
                StepResult(
                    name=label,
                    status="failed" if strict else "skipped",
                    exit_code=1 if strict else 0,
                    summary=f"{'FAIL' if strict else 'SKIPPED'}: no '{script_key}' script or config in {repo_root.name}",
                )
            )
    return results


def _run_python_checks(repo_root: Path, strict: bool) -> list[StepResult]:
    results: list[StepResult] = []
    py_exe = sys.executable

    for label, tool, mod_args, hint in (
        ("Python Ruff", "ruff", ["-m", "ruff", "check", "."], "pip install ruff"),
        ("Python Basedpyright", "basedpyright", ["-m", "basedpyright"], "pip install basedpyright"),
    ):
        exe = _resolve_cmd(tool)
        if exe:
            results.append(_run_step(label, [exe, *mod_args[2:]], repo_root))
        else:
            probe = subprocess.run([py_exe, "-m", tool, "--version"], capture_output=True, check=False)
            if probe.returncode == 0:
                results.append(_run_step(label, [py_exe, *mod_args], repo_root))
            else:
                results.append(_missing_tool_step(label, tool, strict, hint))

    if (repo_root / "tach.toml").is_file():
        tach_exe = _resolve_cmd("tach")
        if tach_exe:
            results.append(_run_step("Python Tach", [tach_exe, "check"], repo_root))
        else:
            results.append(_missing_tool_step("Python Tach", "tach", strict, "pip install tach"))

    return results


def _run_ast_grep(repo_root: Path, strict: bool) -> StepResult:
    sg_exe = _resolve_cmd("ast-grep") or _resolve_cmd("sg")
    if not sg_exe:
        return _missing_tool_step("AOP (ast-grep)", "ast-grep", strict, "npm i -g @ast-grep/cli")

    cfg = repo_root / "sgconfig.yml"
    if not cfg.is_file():
        cfg = TOOLKIT_ROOT / "templates" / "ast-grep" / "sgconfig.yml"
    if not cfg.is_file():
        return _missing_tool_step("AOP (ast-grep)", "sgconfig.yml", strict, "add sgconfig.yml")

    cmd = [sg_exe, "scan", "--config", str(cfg)]
    if strict:
        cmd.extend(["--error"])
    cmd.append(str(repo_root))
    return _run_step("AOP (ast-grep)", cmd, repo_root)


def _run_opengrep(repo_root: Path, strict: bool) -> StepResult:
    og_exe = _resolve_cmd("opengrep")
    if not og_exe:
        return _missing_tool_step("Security Taint (Opengrep)", "opengrep", strict, "install opengrep binary")

    cfg = repo_root / ".opengrep.yml"
    if not cfg.is_file():
        cfg = TOOLKIT_ROOT / "templates" / "opengrep" / "security-rules.yml"
    cmd = [og_exe, "scan", "--config", str(cfg), "--error", str(repo_root)]
    return _run_step("Security Taint (Opengrep)", cmd, repo_root)


def _run_clones(repo_root: Path, stacks: dict[str, bool], strict: bool) -> StepResult:
    bin_name = "structural-clone.exe" if os.name == "nt" else "structural-clone"
    exe = TOOLKIT_ROOT / "bin" / bin_name
    if not exe.is_file():
        return _missing_tool_step("Structural Clones", bin_name, strict, "run scripts/publish_tools.py")

    exts: list[str] = []
    if stacks["dotnet"]:
        exts.append("cs")
    if stacks["xaml"]:
        exts.append("xaml")
    if stacks["rust"]:
        exts.append("rs")
    if stacks["web"]:
        exts.extend(["ts", "js", "html", "css"])
    if stacks["python"]:
        exts.append("py")
    if not exts:
        exts = ["cs", "xaml", "rs"]

    cmd = [str(exe), "--path", str(repo_root), "--extensions", ",".join(exts), "--format", "json"]
    proc = subprocess.run(cmd, cwd=repo_root, capture_output=True, text=True, encoding="utf-8", errors="replace", check=False)
    if proc.returncode != 0:
        return StepResult("Structural Clones", "failed", proc.returncode, "FAIL executing structural-clone", proc.stderr)
    idx = proc.stdout.find("{")
    if idx < 0:
        return StepResult("Structural Clones", "failed", 1, "FAIL: no JSON output")
    data = json.loads(proc.stdout[idx:])
    tot = data["statistics"]["total"]
    return StepResult(
        "Structural Clones",
        "passed",
        0,
        f"PASS ({tot['cloneGroups']} groups, {tot['percentage']}% duplication across {tot['files']} files)",
    )


def run_polyglot_xray(
    repo_root: Path,
    strict: bool = False,
    check_clones: bool = False,
    security: bool = False,
) -> dict:
    stacks = detect_repo_stacks(repo_root)
    steps: list[StepResult] = []

    if stacks["dotnet"]:
        steps.append(_run_step(".NET X-Ray", [sys.executable, str(SCRIPT_DIR / "dotnet_xray.py"), "--repo-root", str(repo_root), "--skip-rust"], repo_root))

    if stacks["xaml"]:
        xaml_report = scan_xaml_repo(repo_root)
        xaml_ok = (xaml_report["failCount"] == 0 and xaml_report["warnCount"] == 0) if strict else xaml_report["passed"]
        steps.append(
            StepResult(
                name="WPF/XAML Style & Template X-Ray",
                status="passed" if xaml_ok else "failed",
                exit_code=0 if xaml_ok else 1,
                summary=f"{'PASS' if xaml_ok else 'FAIL'} ({xaml_report['failCount']} fail, {xaml_report['warnCount']} warn)",
            )
        )

    if stacks["rust"]:
        rust_res = scan_rust_workspace(repo_root, deny_warnings=True, touched_only=not strict)
        steps.append(
            StepResult(
                name="Rust Clippy/Fmt/Supply-Chain",
                status="passed" if rust_res["passed"] else "failed",
                exit_code=0 if rust_res["passed"] else 1,
                summary=f"{'PASS' if rust_res['passed'] else 'FAIL'} ({len(rust_res['findings'])} findings across {rust_res['crateCount']} workspace/crate(s))",
            )
        )

    if stacks["web"]:
        steps.extend(_run_web_checks(repo_root, strict))

    if stacks["python"]:
        steps.extend(_run_python_checks(repo_root, strict))

    steps.append(_run_ast_grep(repo_root, strict))

    if check_clones:
        steps.append(_run_clones(repo_root, stacks, strict))

    if security:
        steps.append(_run_opengrep(repo_root, strict))

    failed = any(s.status == "failed" for s in steps)
    return {
        "repoRoot": str(repo_root),
        "stacks": stacks,
        "strict": strict,
        "passed": not failed,
        "steps": [asdict(s) for s in steps],
    }


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    p = argparse.ArgumentParser(description="Polyglot Codebase X-Ray & Strict Paradigm Gate")
    p.add_argument("--repo-root", "-r", default=".")
    p.add_argument("--strict", action="store_true", help="Fail if any check/tool fails or is unconfigured")
    p.add_argument("--check-clones", action="store_true", help="Include 7-language structural-clone scan")
    p.add_argument("--security", action="store_true", help="Include Opengrep taint security scan")
    p.add_argument("--json", action="store_true")
    args = p.parse_args()

    repo_root = Path(args.repo_root).resolve()
    report = run_polyglot_xray(repo_root, strict=args.strict, check_clones=args.check_clones, security=args.security)

    if args.json:
        print(json.dumps(report, indent=2, ensure_ascii=False))
        return 0 if report["passed"] else 1

    active = [k for k, v in report["stacks"].items() if v]
    print("# Polyglot Codebase X-Ray Report")
    print(f"- **Repository**: {repo_root} | **Detected Stacks**: {', '.join(active) or 'none'} | **Strict**: {args.strict}")
    for s in report["steps"]:
        print(f"- **{s['name']}**: {s['summary']}")
        if s["status"] == "failed" and s["detail"]:
            for line in s["detail"].splitlines()[:10]:
                print(f"    {line}")
    print(f"\nResult: {'PASS' if report['passed'] else 'FAIL'}")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
