#!/usr/bin/env python3
"""Cross-platform .NET 10 + Rust Code Consistency Gate (No-Build Line & Banned Pattern Check)."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
TOOLKIT_ROOT = SCRIPT_DIR.parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from rust_profiles import collect_rust_consistency_findings  # noqa: E402

POLYGLOT_EXTS = (".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".py", ".html", ".htm", ".css")

BANNED_CS_RULES = [
    ("ConfigureAwait", "fail", re.compile(r"\.ConfigureAwait\s*\("), "Banned ConfigureAwait (RS0030)"),
    ("Serilog", "fail", re.compile(r"(?:\busing\s+Serilog\b|\bSerilog\.)"), "Banned Serilog; use project log facade"),
    ("DllImport", "fail", re.compile(r"\[DllImport\b"), "Legacy DllImport; prefer LibraryImport / wrapper"),
    ("EmptyCatch", "fail", re.compile(r"catch\s*\{\s*\}"), "Empty catch block"),
    ("BareCatch", "fail", re.compile(r"catch\s*\{\s*//"), "Bare catch without exception parameter"),
    ("SyncOverAsyncWait", "fail", re.compile(r"\.\s*Wait\s*\("), "Sync-over-async Wait()"),
    ("SyncOverAsyncResult", "fail", re.compile(r"(?:\)|\b[A-Za-z0-9_]*[Tt]ask)\s*\.\s*Result\b(?!\s*=)"), "Sync-over-async .Result"),
    ("GetResult", "fail", re.compile(r"\.GetAwaiter\s*\(\s*\)\s*\.\s*GetResult\s*\("), "Sync-over-async GetResult()"),
    ("AsyncVoid", "warn", re.compile(r"\basync\s+void\b"), "async void (allowed only for UI event handlers)"),
]

SKIP_PATH_SEGS = {"build", "bin", "obj", "target", "target-final", ".agents", "node_modules"}


def collect_targets(repo_root: Path, explicit_paths: list[str]) -> list[Path]:
    targets: list[Path] = []
    if explicit_paths:
        for raw in explicit_paths:
            p = Path(raw)
            full = p if p.is_absolute() else (repo_root / p)
            if full.is_file():
                targets.append(full.resolve())
        return targets

    try:
        proc = subprocess.run(
            ["git", "-C", str(repo_root), "status", "--porcelain", "-uall"],
            capture_output=True, text=True, encoding="utf-8", errors="replace", check=False,
        )
        if proc.returncode == 0 and proc.stdout:
            for line in proc.stdout.splitlines():
                if len(line) > 3:
                    raw = line[3:].strip().strip('"')
                    if " -> " in raw:
                        raw = raw.split(" -> ")[-1].strip('"')
                    full = (repo_root / raw.replace("\\", "/")).resolve()
                    if full.is_file():
                        targets.append(full)
    except OSError:
        pass
    return targets


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    p = argparse.ArgumentParser(description=".NET 10 + Rust Code Consistency Gate (cross-platform Python)")
    p.add_argument("--repo-root", "-r", default=".")
    p.add_argument("--path", "-p", nargs="*", default=[], help="Specific file paths to check")
    p.add_argument("--line-warn-threshold", type=int, default=240)
    p.add_argument("--line-max-limit", type=int, default=300)
    p.add_argument("--rs-line-warn", type=int, default=300)
    p.add_argument("--rs-line-max", type=int, default=450)
    p.add_argument("--include-polyglot", action="store_true", help="Also run ast-grep on touched TS/JS/Py/HTML/CSS files")
    p.add_argument("--json", action="store_true")
    args = p.parse_args()

    repo_root = Path(args.repo_root).resolve()
    raw_targets = collect_targets(repo_root, args.path)

    scoped_files: list[Path] = []
    polyglot_files: list[Path] = []
    for t in raw_targets:
        try:
            rel = t.relative_to(repo_root).as_posix()
        except ValueError:
            rel = t.as_posix()
        low = rel.lower()
        if any(seg in low.split("/") for seg in SKIP_PATH_SEGS):
            continue
        if low.endswith((".cs", ".xaml", ".rs")):
            scoped_files.append(t)
        elif args.include_polyglot and low.endswith(POLYGLOT_EXTS):
            polyglot_files.append(t)

    findings: list[dict] = []
    fail_count = 0
    warn_count = 0

    for full in scoped_files:
        try:
            rel = full.relative_to(repo_root).as_posix()
        except ValueError:
            rel = full.as_posix()
        text = full.read_text(encoding="utf-8-sig", errors="replace")
        suffix = full.suffix.lower()
        fname_low = full.name.lower()

        if suffix in (".cs", ".xaml"):
            line_count = len(text.splitlines())
            if line_count > args.line_max_limit:
                fail_count += 1
                findings.append({"file": rel, "line": line_count, "severity": "fail", "id": "LineMax", "message": f"File has {line_count} lines > {args.line_max_limit}"})
            elif line_count >= args.line_warn_threshold:
                warn_count += 1
                findings.append({"file": rel, "line": line_count, "severity": "warn", "id": "LineWarn", "message": f"File has {line_count} lines (warn >= {args.line_warn_threshold})"})

        if suffix == ".cs" and not fname_low.endswith("governancetests.cs") and not fname_low.endswith(".g.cs"):
            for rule_id, sev, regex, msg in BANNED_CS_RULES:
                for m in regex.finditer(text):
                    ln = text.count("\n", 0, m.start()) + 1
                    if sev == "fail":
                        fail_count += 1
                    else:
                        warn_count += 1
                    findings.append({"file": rel, "line": ln, "severity": sev, "id": rule_id, "message": msg})
        elif suffix == ".rs":
            for item in collect_rust_consistency_findings(full, rel, text, args.rs_line_warn, args.rs_line_max):
                if item["severity"] == "fail":
                    fail_count += 1
                else:
                    warn_count += 1
                findings.append(item)

    if args.include_polyglot and polyglot_files:
        sg_exe = shutil.which("ast-grep.cmd" if os.name == "nt" else "ast-grep") or shutil.which("ast-grep") or shutil.which("sg")
        cfg = repo_root / "sgconfig.yml"
        if not cfg.is_file():
            cfg = TOOLKIT_ROOT / "templates" / "ast-grep" / "sgconfig.yml"
        if sg_exe and cfg.is_file():
            proc = subprocess.run(
                [sg_exe, "scan", "--config", str(cfg), "--json=compact", *[str(p) for p in polyglot_files]],
                cwd=repo_root,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                check=False,
            )
            if proc.stdout.strip().startswith("["):
                try:
                    for hit in json.loads(proc.stdout):
                        sev = "fail" if str(hit.get("severity", "")).lower() == "error" else "warn"
                        if sev == "fail":
                            fail_count += 1
                        else:
                            warn_count += 1
                        ln = int(((hit.get("range") or {}).get("start") or {}).get("line", 0)) + 1
                        findings.append(
                            {
                                "file": str(hit.get("file", "")),
                                "line": ln,
                                "severity": sev,
                                "id": str(hit.get("ruleId", "ast-grep")),
                                "message": str(hit.get("message", "")),
                            }
                        )
                except json.JSONDecodeError:
                    pass

    scoped_count = len(scoped_files) + len(polyglot_files)
    failed = fail_count > 0
    if args.json:
        print(json.dumps({"repoRoot": str(repo_root), "filesScanned": scoped_count, "failCount": fail_count, "warnCount": warn_count, "findings": findings, "passed": not failed}, indent=2, ensure_ascii=False))
        return 1 if failed else 0

    print("============================================================")
    print(f" Polyglot Code Consistency ({repo_root})")
    print("============================================================")
    print(f"Files scanned: {scoped_count}")
    if scoped_count == 0:
        print("  (no touched target files — pass with nothing to check)")
        print("Result: PASS")
        return 0

    fails = [f for f in findings if f["severity"] == "fail"]
    warns = [f for f in findings if f["severity"] == "warn"]
    if fails:
        print("\n[FAIL]")
        for f in fails:
            print(f"  {f['file']}:{f['line']}  [{f['id']}] {f['message']}")
    if warns:
        print("\n[WARN]")
        for w in warns:
            print(f"  {w['file']}:{w['line']}  [{w['id']}] {w['message']}")

    print("")
    if failed:
        print(f"Result: FAIL ({len(fails)} fail, {len(warns)} warn)")
        return 1
    print(f"Result: PASS ({len(warns)} warn)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
