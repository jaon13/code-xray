#!/usr/bin/env python3
"""Cross-platform Rust Stabilization Gate (No-Build Guard + Format/Clippy Ratchet + Supply-Chain + Tests)."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from dotnet_xray import run_clone_scan  # noqa: E402
from rust_profiles import collect_rust_consistency_findings, discover_rust_targets  # noqa: E402
from rust_xray import get_git_touched_rust_files, scan_rust_workspace  # noqa: E402

HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")


def get_touched_line_ranges(repo_root: Path, rel_path: str) -> list[tuple[int, int]] | None:
    """Return 1-based inclusive line ranges added/modified in git, or None if untracked."""
    status = subprocess.run(
        ["git", "-C", str(repo_root), "status", "--porcelain", "--", rel_path],
        capture_output=True, text=True, encoding="utf-8", errors="replace", check=False,
    )
    if status.returncode == 0 and status.stdout.strip().startswith("??"):
        return None

    ranges: list[tuple[int, int]] = []
    for extra in ([], ["--cached"]):
        proc = subprocess.run(
            ["git", "-C", str(repo_root), "diff", "-U0", *extra, "--", rel_path],
            capture_output=True, text=True, encoding="utf-8", errors="replace", check=False,
        )
        if proc.returncode != 0:
            continue
        for line in proc.stdout.splitlines():
            m = HUNK_RE.match(line)
            if not m:
                continue
            start = int(m.group(1))
            count = int(m.group(2) or "1")
            if count > 0:
                ranges.append((max(1, start - 2), start + count + 2))
    return ranges


def _line_hits_ranges(line_no: int, ranges: list[tuple[int, int]] | None) -> bool:
    if ranges is None:
        return True
    return any(lo <= line_no <= hi for lo, hi in ranges)


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    p = argparse.ArgumentParser(description="Rust Stabilization Gate (cross-platform Python)")
    p.add_argument("--repo-root", "-r", default=".", help="Repository root directory")
    p.add_argument("--target-crate", default="", help="Optional crate/workspace filter")
    p.add_argument("--strict", action="store_true", help="Fail on ALL workspace Clippy warnings (default: touched-only ratchet)")
    p.add_argument("--fmt-mode", choices=("touched", "warn", "strict"), default="touched", help="rustfmt enforcement mode")
    p.add_argument("--no-build", action="store_true", help="Skip Clippy/cargo test execution")
    p.add_argument("--no-test", action="store_true", help="Skip cargo test")
    p.add_argument("--skip-supply-chain", action="store_true", help="Skip cargo-deny and cargo-audit")
    p.add_argument("--check-features", action="store_true", help="Verify --no-default-features and --all-features compilation")
    p.add_argument("--check-clones", action="store_true", help="Run Rust structural-clone scan")
    p.add_argument("--sarif-dir", default="", help="Optional SARIF output directory")
    args = p.parse_args()

    repo_root = Path(args.repo_root).resolve()
    failed = False
    print("============================================================")
    print(f" Rust Stabilization Gate ({repo_root})")
    print("============================================================")

    targets = discover_rust_targets(repo_root, args.target_crate)
    if not targets:
        print("No Cargo.toml targets detected — PASS.")
        return 0

    touched = get_git_touched_rust_files(repo_root)
    print(f"[1/5] Touched .rs Consistency Guard ({len(touched)} touched file(s))")
    consistency_fails: list[dict] = []
    consistency_warns: list[dict] = []
    for rel in sorted(touched):
        full = repo_root / rel
        text = full.read_text(encoding="utf-8-sig", errors="replace")
        for item in collect_rust_consistency_findings(full, rel, text):
            if item["severity"] == "fail":
                consistency_fails.append(item)
            else:
                consistency_warns.append(item)

    if consistency_fails:
        print(f"  [FAIL] {len(consistency_fails)} consistency violation(s):")
        for f in consistency_fails[:20]:
            print(f"    - {f['file']}:{f['line']} [{f['id']}] {f['message']}")
        failed = True
    else:
        print(f"  [PASS] 0 hard consistency failures ({len(consistency_warns)} warn).")
        for w in consistency_warns[:10]:
            print(f"    - [WARN] {w['file']}:{w['line']} [{w['id']}] {w['message']}")

    print("[2/5] Rustfmt & Clippy Static Analysis (Workspace Ratchet)")
    if not args.no_build:
        sarif_dir = Path(args.sarif_dir).resolve() if args.sarif_dir else None
        rust_res = scan_rust_workspace(
            repo_root,
            args.target_crate,
            deny_warnings=True,
            skip_supply_chain=args.skip_supply_chain,
            touched_files=touched,
            touched_only=not args.strict,
            sarif_dir=sarif_dir,
        )
        range_cache = {rel.lower(): get_touched_line_ranges(repo_root, rel) for rel in touched}
        ratchet_clippy_hits: list[dict] = []
        for rc in rust_res["crates"]:
            touched_fmt_hits = [
                d for d in rc["rustfmtDiffLocations"]
                if d["file"].lower() in range_cache and _line_hits_ranges(d["line"], range_cache[d["file"].lower()])
            ]
            fmt_ok = rc["rustfmtPassed"] if args.fmt_mode == "strict" else (len(touched_fmt_hits) == 0 if args.fmt_mode == "touched" else True)
            fmt_label = "PASS" if rc["rustfmtPassed"] else ("WARN" if fmt_ok else "FAIL")
            if args.strict:
                clippy_ok = rc["clippyPassed"]
                crate_hits = [f for f in rust_res["findings"] if f.get("workspace") == rc["crate"]]
            else:
                crate_hits = [
                    f for f in rust_res["touchedFindings"]
                    if f.get("workspace") == rc["crate"]
                    and f["file"].lower() in range_cache
                    and (f["level"] == "error" or _line_hits_ranges(f["line"], range_cache[f["file"].lower()]))
                ]
                has_err = any(f["level"] == "error" for f in rust_res["findings"] if f.get("workspace") == rc["crate"])
                clippy_ok = (not has_err) and (len(crate_hits) == 0)
            ratchet_clippy_hits.extend(crate_hits)

            print(
                f"  [RUST] {rc['crate']} (profile={rc['profile']}) -> Clippy: {'PASS' if clippy_ok else 'FAIL'} "
                f"({rc['findingCount']} total, {len(crate_hits)} touched-hunk) | Rustfmt: {fmt_label} | "
                f"cargo-deny: {rc['cargoDeny'].upper()} | cargo-audit: {rc['cargoAudit'].upper()}"
            )
            if not clippy_ok:
                failed = True
                if rc["clippyError"]:
                    for ln in rc["clippyError"].splitlines()[-12:]:
                        print(f"    {ln}")
            if not fmt_ok:
                failed = True
                for hit in touched_fmt_hits[:10] if touched_fmt_hits else rc["rustfmtDiffLocations"][:10]:
                    print(f"    - [FMT] {hit['file']}:{hit['line']} needs `cargo fmt`")
            if rc["cargoDeny"] == "failed":
                failed = True
                print(f"    [FAIL] cargo-deny:\n{rc['cargoDenyDetail']}")
            if rc["cargoAudit"] == "failed":
                failed = True
                print(f"    [FAIL] cargo-audit:\n{rc['cargoAuditDetail']}")

        for rf in ratchet_clippy_hits[:20]:
            print(f"    - {rf['file']}:{rf['line']} [{rf['ruleId']}] ({rf['category']}) {rf['message']}")
    else:
        print("  [SKIP] Build/Clippy skipped (--no-build).")

    print("[3/5] Feature Matrix Check (--no-default-features / --all-features)")
    if args.check_features and not args.no_build:
        for t in targets:
            for flag in ("--no-default-features", "--all-features"):
                cmd = ["cargo", "check"]
                if t["isWorkspace"]:
                    cmd.append("--workspace")
                cmd.append(flag)
                proc = subprocess.run(cmd, cwd=t["workspaceRoot"], capture_output=True, text=True, encoding="utf-8", errors="replace", check=False)
                if proc.returncode != 0:
                    print(f"  [FAIL] {t['name']} {flag} failed:")
                    for ln in (proc.stdout + "\n" + proc.stderr).splitlines()[-15:]:
                        print(f"    {ln}")
                    failed = True
                else:
                    print(f"  [PASS] {t['name']} {flag}")
    else:
        print("  [SKIP] Pass --check-features to verify feature combinations.")

    print("[4/5] Cargo Workspace Tests")
    if not args.no_test and not args.no_build:
        for t in targets:
            cmd = ["cargo", "test"]
            if t["isWorkspace"]:
                cmd.append("--workspace")
            t_proc = subprocess.run(cmd, cwd=t["workspaceRoot"], capture_output=True, text=True, encoding="utf-8", errors="replace", check=False)
            if t_proc.returncode != 0:
                print(f"  [FAIL] cargo test failed in {t['name']}:")
                for ln in (t_proc.stdout + "\n" + t_proc.stderr).splitlines()[-25:]:
                    print(f"    {ln}")
                failed = True
            else:
                print(f"  [PASS] cargo test green ({t['name']}).")
    else:
        print("  [SKIP] cargo test skipped.")

    print("[5/5] Structural Clone Scan (Rust)")
    if args.check_clones:
        try:
            rep = run_clone_scan(repo_root, [], 12, 15)
            rs_stats = (rep.get("statistics", {}).get("formats") or {}).get("rust")
            if rs_stats:
                print(f"  [INFO] Rust duplication: {rs_stats['percentage']}% ({rs_stats.get('clonePairs', 0)} pairs)")
            else:
                print("  [PASS] Zero Rust structural clones above threshold.")
        except Exception as exc:
            print(f"  [WARN] Clone scan skipped/failed: {exc}")
    else:
        print("  [SKIP] Pass --check-clones to run structural-clone.")

    print("RESULT: FAILED" if failed else "RESULT: PASSED")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
