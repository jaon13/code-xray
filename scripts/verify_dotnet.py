#!/usr/bin/env python3
"""Cross-platform .NET 10 + Rust Stabilization Gate (Touched Line Budget + SARIF + Clippy/Deny/Audit + Governance Tests)."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import urllib.parse
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from rust_profiles import collect_rust_consistency_findings, discover_rust_targets  # noqa: E402
from rust_xray import scan_rust_workspace  # noqa: E402
from xaml_style_xray import scan_xaml_repo  # noqa: E402


def rel_repo_path(repo_root: Path, raw_uri_or_path: str) -> str:
    if not raw_uri_or_path:
        return ""
    cleaned = urllib.parse.unquote(re.sub(r"^file:///?", "", raw_uri_or_path))
    if os.name != "nt" and not cleaned.startswith("/") and not re.match(r"^[A-Za-z]:", cleaned):
        cleaned = "/" + cleaned
    p = Path(cleaned)
    try:
        return p.resolve(strict=False).relative_to(repo_root).as_posix()
    except ValueError:
        s_norm = cleaned.replace("\\", "/")
        r_norm = repo_root.as_posix().rstrip("/") + "/"
        if s_norm.lower().startswith(r_norm.lower()):
            return s_norm[len(r_norm) :]
        return s_norm


def get_git_touched_files(repo_root: Path) -> set[str]:
    touched: set[str] = set()
    try:
        proc = subprocess.run(
            ["git", "-C", str(repo_root), "status", "--porcelain", "-uall"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
        if proc.returncode == 0 and proc.stdout:
            for line in proc.stdout.splitlines():
                if len(line) > 3:
                    raw = line[3:].strip().strip('"')
                    if " -> " in raw:
                        raw = raw.split(" -> ")[-1].strip('"')
                    norm = raw.replace("\\", "/")
                    full = repo_root / norm
                    if full.is_file():
                        touched.add(norm)
    except OSError:
        pass
    return touched


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    p = argparse.ArgumentParser(description=".NET 10 + Rust Stabilization Gate (cross-platform Python)")
    p.add_argument("--repo-root", "-r", default=".", help="Repository root directory")
    p.add_argument("--configuration", "-c", default="Debug")
    p.add_argument("--no-build", action="store_true")
    p.add_argument("--governance-test-project", default="")
    p.add_argument("--test-filter-trait", default="Category=Governance")
    p.add_argument("--extra-test-project", default="")
    p.add_argument("--extra-filter-class", default="")
    p.add_argument("--line-max-limit", type=int, default=300)
    p.add_argument("--fail-on-touched-warnings", action="store_true")
    p.add_argument("--skip-rust", action="store_true", help="Skip Rust Clippy/Rustfmt/Deny/Audit gate")
    p.add_argument("--skip-supply-chain", action="store_true", help="Skip cargo-deny and cargo-audit")
    args = p.parse_args()

    repo_root = Path(args.repo_root).resolve()
    failed = False

    print("============================================================")
    print(f" .NET 10 + Rust Stabilization Gate ({repo_root})")
    print("============================================================")

    touched = get_git_touched_files(repo_root)
    touched_lower_map = {t.lower(): t for t in touched}

    print(f"[1/5] Touched Files Line-Budget Audit (.cs / .xaml <= {args.line_max_limit} lines; .rs prod-only guard)")
    violations: list[str] = []
    for rel in sorted(touched):
        low = rel.lower()
        if low.startswith(("build/", "bin/", "obj/", "target/", "target-final/", ".agents/", "node_modules/")):
            continue
        full = repo_root / rel
        if low.endswith((".cs", ".xaml")):
            try:
                cnt = len(full.read_text(encoding="utf-8-sig", errors="replace").splitlines())
            except OSError:
                continue
            if cnt > args.line_max_limit:
                violations.append(f"{rel} ({cnt} lines > {args.line_max_limit})")
        elif low.endswith(".rs"):
            try:
                text = full.read_text(encoding="utf-8-sig", errors="replace")
            except OSError:
                continue
            for item in collect_rust_consistency_findings(full, rel, text):
                if item["severity"] == "fail":
                    violations.append(f"{rel}:{item['line']} [{item['id']}] {item['message']}")

    if violations:
        print("  [FAIL] Touched-file governance violation(s):")
        for v in violations:
            print(f"    - {v}")
        failed = True
    else:
        print(f"  [PASS] {len(touched)} touched file(s) checked; 0 violations.")

    print(f"[2/5] MSBuild + Roslyn Static Analysis (Configuration: {args.configuration})")
    slns = list(repo_root.glob("*.slnx")) + list(repo_root.glob("*.sln"))
    has_dotnet = bool(slns) or any(repo_root.rglob("*.csproj"))
    if not args.no_build and has_dotnet:
        target = str(slns[0] if slns else repo_root)
        b_proc = subprocess.run(
            ["dotnet", "build", target, "-c", args.configuration, "-v", "minimal"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
        if b_proc.returncode != 0:
            print("  [FAIL] dotnet build failed:")
            combined = (b_proc.stdout + "\n" + b_proc.stderr).splitlines()
            for ln in combined[-30:]:
                print(f"    {ln}")
            return 1
        print("  [PASS] Build succeeded.")
    elif not has_dotnet:
        print("  [SKIP] No .sln/.slnx/.csproj detected.")
    else:
        print("  [SKIP] Build skipped (--no-build).")

    sarif_dir = repo_root / "Build" / "sarif" / args.configuration
    touched_findings: list[tuple[str, int, str, str]] = []
    total_warnings = 0
    total_errors = 0
    seen: set[str] = set()

    if sarif_dir.is_dir():
        for sf in sorted(sarif_dir.glob("*.sarif")):
            try:
                sarif = json.loads(sf.read_text(encoding="utf-8-sig"))
            except (OSError, json.JSONDecodeError):
                continue
            for run in sarif.get("runs") or []:
                for res in run.get("results") or []:
                    lvl = str(res.get("level", "warning"))
                    if lvl in ("none", "note"):
                        continue
                    locs = res.get("locations") or []
                    uri, ln = "", 0
                    if locs:
                        phys = locs[0].get("physicalLocation") or {}
                        uri = (phys.get("artifactLocation") or {}).get("uri", "")
                        ln = int((phys.get("region") or {}).get("startLine") or 0)
                    norm = rel_repo_path(repo_root, uri)
                    rule_id = str(res.get("ruleId", ""))
                    key = f"{norm.lower()}|{ln}|{rule_id.lower()}"
                    if key in seen:
                        continue
                    seen.add(key)
                    if lvl == "error":
                        total_errors += 1
                    else:
                        total_warnings += 1
                    if norm.lower() in touched_lower_map:
                        msg = str((res.get("message") or {}).get("text", ""))
                        touched_findings.append((norm, ln, rule_id, msg))

    print(f"  [SARIF] Solution totals -> Errors: {total_errors} | Warnings: {total_warnings} | In Touched Files: {len(touched_findings)}")
    if touched_findings:
        print("  [WARN] Static analysis findings in touched file(s):")
        for fpath, ln, rid, msg in sorted(touched_findings)[:25]:
            print(f"    - {fpath}:{ln} [{rid}] {msg}")
        if args.fail_on_touched_warnings:
            failed = True

    if any(t.lower().endswith(".xaml") for t in touched):
        xaml_res = scan_xaml_repo(repo_root)
        print(
            f"  [XAML] Styles={xaml_res['stylesFound']} Templates={xaml_res['templatesFound']} "
            f"ExactStyleClones={len(xaml_res['exactStyleClones'])} TemplateClones={len(xaml_res['templateClones'])} "
            f"BasedOn={len(xaml_res['basedOnCandidates'])} Fail={xaml_res['failCount']} Warn={xaml_res['warnCount']}"
        )
        if not xaml_res["passed"]:
            failed = True

    print("[3/5] Rust Native Static Analysis, Supply-Chain & Unit Tests")
    rust_targets = [] if args.skip_rust else discover_rust_targets(repo_root)
    if rust_targets:
        touched_rs = {t for t in touched if t.lower().endswith(".rs")}
        rust_res = scan_rust_workspace(
            repo_root,
            "",
            deny_warnings=True,
            skip_supply_chain=args.skip_supply_chain,
            touched_files=touched_rs,
            touched_only=bool(touched_rs),
        )
        for rc in rust_res["crates"]:
            print(
                f"  [RUST] {rc['crate']} (profile={rc['profile']}) -> Clippy: {'PASS' if rc['clippyPassed'] else 'FAIL'} "
                f"({rc['findingCount']} total, {rc['touchedFindingCount']} touched) | "
                f"Rustfmt: {'PASS' if rc['rustfmtPassed'] else 'FAIL'} | "
                f"cargo-deny: {rc['cargoDeny'].upper()} | cargo-audit: {rc['cargoAudit'].upper()}"
            )
            if not rc["rustfmtPassed"]:
                print(f"    [WARN/FAIL] Unformatted Rust code in {rc['manifest']} (run `cargo fmt`).")
            if rc["cargoDeny"] == "failed":
                print(f"    [FAIL] cargo-deny failed:\n{rc['cargoDenyDetail']}")
            if rc["cargoAudit"] == "failed":
                print(f"    [FAIL] cargo-audit failed:\n{rc['cargoAuditDetail']}")
        if not rust_res["passed"]:
            shown = rust_res["touchedFindings"] if touched_rs else rust_res["findings"]
            for rf in shown[:20]:
                print(f"    - {rf['file']}:{rf['line']} [{rf['ruleId']}] {rf['message']}")
            failed = True
        elif not args.no_build:
            for target in rust_targets:
                cmd = ["cargo", "test"]
                if target["isWorkspace"]:
                    cmd.append("--workspace")
                t_proc = subprocess.run(
                    cmd,
                    cwd=target["workspaceRoot"],
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    check=False,
                )
                if t_proc.returncode != 0:
                    print(f"  [FAIL] cargo test failed in {target['name']}:")
                    for ln in (t_proc.stdout + "\n" + t_proc.stderr).splitlines()[-25:]:
                        print(f"    {ln}")
                    failed = True
                else:
                    print(f"  [PASS] cargo test green ({target['name']}).")
    else:
        print("  [SKIP] No Cargo.toml targets detected (or --skip-rust).")

    print(f"[4/5] Architecture & Code Governance Tests ({args.test_filter_trait})")
    gov_proj = (
        repo_root / args.governance_test_project.replace("\\", "/")
        if args.governance_test_project
        else next(iter(sorted(repo_root.glob("tests/*Tests*/*.csproj"))), None)
    )
    if gov_proj is not None and gov_proj.is_file():
        g_proc = subprocess.run(
            ["dotnet", "test", "--project", str(gov_proj), "-c", args.configuration, "--no-build", "--filter-trait", args.test_filter_trait],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
        out_lines = (g_proc.stdout + "\n" + g_proc.stderr).splitlines()
        if g_proc.returncode != 0:
            print("  [FAIL] Governance test suite failed:")
            for ln in out_lines[-35:]:
                print(f"    {ln}")
            failed = True
        else:
            summary_hits = [x.strip() for x in out_lines if re.search(r"성공|통과|succeeded|passed|total", x, re.I)]
            print(f"  [PASS] Governance tests green. {' | '.join(summary_hits[-2:])}")
    else:
        print(f"  [SKIP] Governance test project not found at {args.governance_test_project}")

    print("[5/5] Focused Module Regression Tests")
    if args.extra_test_project:
        extra_full = repo_root / args.extra_test_project.replace("\\", "/")
        cmd = ["dotnet", "test", "--project", str(extra_full), "-c", args.configuration]
        if args.extra_filter_class:
            cmd.extend(["--filter-class", args.extra_filter_class])
        e_proc = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", check=False)
        if e_proc.returncode != 0:
            print("  [FAIL] Focused tests failed:")
            for ln in (e_proc.stdout + "\n" + e_proc.stderr).splitlines()[-35:]:
                print(f"    {ln}")
            failed = True
        else:
            print("  [PASS] Focused module tests passed.")
    else:
        print("  [INFO] No --extra-test-project specified.")

    if failed:
        print("RESULT: FAILED")
        return 1
    print("RESULT: PASSED")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
