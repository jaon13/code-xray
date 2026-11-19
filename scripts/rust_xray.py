#!/usr/bin/env python3
"""Cross-platform Rust Static Analysis, SARIF v2.1 Exporter, & Supply-Chain X-Ray."""

from __future__ import annotations

import argparse
import collections
import json
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from rust_profiles import discover_rust_targets, is_rust_test_or_build  # noqa: E402

RUST_CATEGORY_WEIGHTS = {
    "BannedAPI/Panic": 5,
    "Unsafe/FFI": 5,
    "SupplyChain/Security": 4,
    "Concurrency/Async": 4,
    "Error/CastSafety": 3,
    "Complexity/Coupling/SRP": 3,
    "Performance/Allocation": 2,
    "Style/Maintainability": 1,
}

FMT_DIFF_LINE_RE = re.compile(r"^Diff in (?:\\\\\?\\)?(.+?):(\d+):?\s*$")


def classify_rust_rule(rule_id: str) -> str:
    rid = rule_id.lower()
    if any(k in rid for k in ("unwrap_used", "expect_used", "panic", "todo", "unimplemented", "dbg_macro", "print_stdout", "print_stderr", "exit")):
        return "BannedAPI/Panic"
    if any(k in rid for k in ("unsafe", "safety_doc", "ptr", "transmute", "mem_forget", "uninit", "dangling", "repr", "extern")):
        return "Unsafe/FFI"
    if any(k in rid for k in ("rustsec", "advisories", "bans", "licenses", "sources", "cargo-deny", "cargo-audit", "security", "vulnerability")):
        return "SupplyChain/Security"
    if any(k in rid for k in ("await_holding_lock", "await_holding_refcell_ref", "large_futures", "future_not_send", "mutex", "send", "sync", "atomic", "rc_buffer", "arc")):
        return "Concurrency/Async"
    if any(k in rid for k in ("must_use", "cast_possible_truncation", "cast_possible_wrap", "cast_sign_loss", "cast_lossless", "map_err_ignore")):
        return "Error/CastSafety"
    if any(k in rid for k in ("cognitive_complexity", "too_many_arguments", "too_many_lines", "type_complexity", "excessive_bools", "struct_excessive_bools")):
        return "Complexity/Coupling/SRP"
    if any(k in rid for k in ("needless_collect", "large_enum_variant", "box_collection", "inefficient_to_string", "trivially_copy_pass_by_ref", "unnecessary_wraps", "redundant_clone", "clone_on_copy")):
        return "Performance/Allocation"
    return "Style/Maintainability"


def discover_cargo_manifests(repo_root: Path, target_filter: str = "") -> list[Path]:
    """Legacy-compatible wrapper returning workspace/standalone root Cargo.toml manifests."""
    return [t["manifest"] for t in discover_rust_targets(repo_root, target_filter)]


def rel_repo_path(repo_root: Path, raw_path: str) -> str:
    cleaned = raw_path[4:] if raw_path.startswith("\\\\?\\") else raw_path
    p = Path(cleaned)
    try:
        return p.resolve(strict=False).relative_to(repo_root).as_posix()
    except ValueError:
        return cleaned.replace("\\", "/")


def get_git_touched_rust_files(repo_root: Path) -> set[str]:
    touched: set[str] = set()
    try:
        proc = subprocess.run(["git", "-C", str(repo_root), "status", "--porcelain", "-uall"], capture_output=True, text=True, encoding="utf-8", errors="replace", check=False)
        if proc.returncode == 0 and proc.stdout:
            for line in proc.stdout.splitlines():
                if len(line) > 3:
                    raw = line[3:].strip().strip('"')
                    if " -> " in raw:
                        raw = raw.split(" -> ")[-1].strip('"')
                    norm = raw.replace("\\", "/")
                    if norm.lower().endswith(".rs") and (repo_root / norm).is_file():
                        touched.add(norm)
    except OSError:
        pass
    return touched


def _resolve_sarif_path(repo_root: Path, target_name: str, sarif_dir: Path | None) -> Path | None:
    if sarif_dir is not None:
        return sarif_dir / f"{target_name}.sarif"
    if (repo_root / "Build").is_dir():
        return repo_root / "Build" / "sarif" / "rust" / f"{target_name}.sarif"
    if (repo_root / "target").is_dir():
        return repo_root / "target" / "xray" / "sarif" / f"{target_name}.sarif"
    return None


def write_clippy_sarif(sarif_path: Path, target_name: str, findings: list[dict]) -> None:
    sarif_path.parent.mkdir(parents=True, exist_ok=True)
    rules_map: dict[str, dict] = {}
    results: list[dict] = []

    for f in findings:
        rid = f["ruleId"]
        if rid not in rules_map:
            rules_map[rid] = {"id": rid, "shortDescription": {"text": f["message"]}, "properties": {"category": f["category"]}}
        results.append({
            "ruleId": rid,
            "level": "error" if f["level"] == "error" else "warning",
            "message": {"text": f["message"]},
            "locations": [{"physicalLocation": {"artifactLocation": {"uri": f["absUri"]}, "region": {"startLine": max(1, f["line"]), "startColumn": max(1, f["column"])}}}],
        })

    sarif_doc = {
        "$schema": "https://schemastore.azurewebsites.net/schemas/json/sarif-2.1.0-rtm.5.json",
        "version": "2.1.0",
        "runs": [{"tool": {"driver": {"name": f"cargo-clippy ({target_name})", "informationUri": "https://rust-lang.github.io/rust-clippy/", "rules": list(rules_map.values())}}, "results": results}],
    }
    sarif_path.write_text(json.dumps(sarif_doc, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def run_clippy_on_target(
    repo_root: Path,
    target: dict,
    deny_warnings: bool = False,
    sarif_dir: Path | None = None,
) -> tuple[bool, list[dict], str]:
    ws_root: Path = target["workspaceRoot"]
    target_name: str = target["name"]
    cmd = ["cargo", "clippy"]
    if target["isWorkspace"]:
        cmd.append("--workspace")
    cmd.extend(["--all-targets", "--no-deps", "--message-format=json"])
    if deny_warnings:
        cmd.extend(["--", "-D", "warnings"])

    proc = subprocess.run(cmd, cwd=ws_root, capture_output=True, text=True, encoding="utf-8", errors="replace", check=False)
    findings: list[dict] = []
    seen: set[str] = set()

    for raw_line in proc.stdout.splitlines():
        if not raw_line.startswith("{"):
            continue
        try:
            payload = json.loads(raw_line)
        except json.JSONDecodeError:
            continue
        if payload.get("reason") != "compiler-message":
            continue
        msg_obj = payload.get("message") or {}
        level = str(msg_obj.get("level", ""))
        if level not in ("warning", "error"):
            continue
        msg_text = str(msg_obj.get("message", ""))
        if ("generated " in msg_text and "warning" in msg_text and not msg_obj.get("spans")) or msg_text.startswith("aborting due to "):
            continue

        code_obj = msg_obj.get("code") or {}
        rule_id = str(code_obj.get("code") or f"rustc::{level}")
        spans = msg_obj.get("spans") or []
        primary = next((s for s in spans if s.get("is_primary")), spans[0] if spans else {})
        if rule_id == "clippy::let_underscore_must_use":
            span_text = " ".join(str(t.get("text", "")) for t in (primary.get("text") or []))
            if span_text and not re.search(r"\blet\b", span_text):
                continue
        file_name = str(primary.get("file_name", ""))
        line_num = int(primary.get("line_start") or 1)
        col_num = int(primary.get("column_start") or 1)

        abs_file = (ws_root / file_name).resolve(strict=False) if file_name else ws_root
        rel_file = rel_repo_path(repo_root, str(abs_file))
        dedup_key = f"{rel_file.lower()}|{line_num}|{rule_id.lower()}"
        if dedup_key in seen:
            continue
        seen.add(dedup_key)

        cat = classify_rust_rule(rule_id)
        pkg_name = str(payload.get("package_id", "")).split(" ", 1)[0].split("#", 1)[0] or target_name
        findings.append({
            "project": pkg_name,
            "workspace": target_name,
            "profile": target["profile"],
            "file": rel_file,
            "absUri": abs_file.as_uri(),
            "line": line_num,
            "column": col_num,
            "level": level,
            "ruleId": rule_id,
            "category": cat,
            "weight": RUST_CATEGORY_WEIGHTS[cat],
            "message": msg_text,
        })

    sarif_out = _resolve_sarif_path(repo_root, target_name, sarif_dir)
    if sarif_out is not None:
        write_clippy_sarif(sarif_out, target_name, findings)
    return proc.returncode == 0, findings, proc.stderr.strip()


def _parse_fmt_diff_locations(repo_root: Path, stdout_text: str) -> list[dict]:
    hits: list[dict] = []
    seen: set[tuple[str, int]] = set()
    for line in stdout_text.splitlines():
        m = FMT_DIFF_LINE_RE.match(line.strip())
        if not m:
            continue
        rel = rel_repo_path(repo_root, m.group(1))
        ln = int(m.group(2))
        if (rel.lower(), ln) not in seen:
            seen.add((rel.lower(), ln))
            hits.append({"file": rel, "line": ln})
    return hits


def check_target_supply_chain(repo_root: Path, target: dict, skip_supply_chain: bool) -> dict:
    ws_root: Path = target["workspaceRoot"]
    fmt_proc = subprocess.run(["cargo", "fmt", "--all", "--", "--check"], cwd=ws_root, capture_output=True, text=True, encoding="utf-8", errors="replace", check=False)
    fmt_ok = fmt_proc.returncode == 0
    fmt_diffs = [] if fmt_ok else _parse_fmt_diff_locations(repo_root, fmt_proc.stdout)

    deny_status = "skipped"
    deny_detail = ""
    deny_toml = ws_root / "deny.toml"
    if not skip_supply_chain and deny_toml.is_file() and shutil.which("cargo-deny"):
        advisory_db_dir = Path.home() / ".cargo" / "advisory-dbs"
        cmd = ["cargo", "deny", "--manifest-path", str(target["manifest"])]
        if advisory_db_dir.is_dir():
            cmd.append("--offline")
        cmd.append("check")
        d_proc = subprocess.run(cmd, cwd=ws_root, capture_output=True, text=True, encoding="utf-8", errors="replace", check=False)
        if d_proc.returncode != 0 and "--offline" in cmd and "advisory" in (d_proc.stdout + d_proc.stderr).lower() and "not found" in (d_proc.stdout + d_proc.stderr).lower():
            d_proc = subprocess.run(["cargo", "deny", "--manifest-path", str(target["manifest"]), "check"], cwd=ws_root, capture_output=True, text=True, encoding="utf-8", errors="replace", check=False)
        deny_status = "passed" if d_proc.returncode == 0 else "failed"
        deny_detail = (d_proc.stdout + "\n" + d_proc.stderr).strip()

    audit_status = "skipped"
    audit_detail = ""
    if not skip_supply_chain and (ws_root / "Cargo.lock").is_file() and shutil.which("cargo-audit"):
        advisory_db = Path.home() / ".cargo" / "advisory-db"
        audit_cmd = ["cargo", "audit", "--no-fetch"] if advisory_db.is_dir() else ["cargo", "audit"]
        if deny_toml.is_file():
            try:
                deny_cfg = tomllib.loads(deny_toml.read_text(encoding="utf-8"))
                for item in (deny_cfg.get("advisories") or {}).get("ignore") or []:
                    adv_id = item.get("id") if isinstance(item, dict) else (item if isinstance(item, str) else None)
                    if adv_id:
                        audit_cmd.extend(["--ignore", str(adv_id)])
            except (OSError, tomllib.TOMLDecodeError):
                pass
        a_proc = subprocess.run(audit_cmd, cwd=ws_root, capture_output=True, text=True, encoding="utf-8", errors="replace", check=False)
        audit_status = "passed" if a_proc.returncode == 0 else "failed"
        audit_detail = (a_proc.stdout + "\n" + a_proc.stderr).strip()

    return {
        "rustfmtPassed": fmt_ok,
        "rustfmtDiffLocations": fmt_diffs,
        "rustfmtDiff": fmt_proc.stdout.strip() if not fmt_ok else "",
        "cargoDeny": deny_status,
        "cargoDenyDetail": deny_detail,
        "cargoAudit": audit_status,
        "cargoAuditDetail": audit_detail,
    }


def scan_rust_workspace(
    repo_root: Path,
    target_project: str = "",
    deny_warnings: bool = False,
    skip_supply_chain: bool = False,
    touched_files: set[str] | None = None,
    touched_only: bool = False,
    sarif_dir: Path | None = None,
) -> dict:
    targets = discover_rust_targets(repo_root, target_project)
    touched_lower = {t.lower() for t in touched_files} if touched_files else set()
    crates: list[dict] = []
    all_findings: list[dict] = []
    touched_findings: list[dict] = []
    all_passed = True

    for target in targets:
        clippy_ok, findings, clippy_err = run_clippy_on_target(repo_root, target, deny_warnings and not touched_only, sarif_dir)
        hygiene = check_target_supply_chain(repo_root, target, skip_supply_chain)
        t_findings = [
            f for f in findings
            if f["file"].lower() in touched_lower
            and not (
                is_rust_test_or_build(f["file"])
                and any(k in f["ruleId"].lower() for k in ("unwrap_used", "expect_used", "panic", "print_stdout", "print_stderr", "dbg_macro"))
            )
        ]
        t_fmt_diffs = [d for d in hygiene["rustfmtDiffLocations"] if d["file"].lower() in touched_lower]
        has_compile_errors = any(f["level"] == "error" for f in findings)

        fmt_gate_ok = hygiene["rustfmtPassed"] if not touched_only else (len(t_fmt_diffs) == 0)
        clippy_gate_ok = (not has_compile_errors and (not deny_warnings or not t_findings)) if touched_only else clippy_ok
        target_ok = clippy_gate_ok and fmt_gate_ok and hygiene["cargoDeny"] != "failed" and hygiene["cargoAudit"] != "failed"
        if not target_ok:
            all_passed = False

        crates.append({
            "crate": target["name"],
            "manifest": rel_repo_path(repo_root, str(target["manifest"])),
            "isWorkspace": target["isWorkspace"],
            "profile": target["profile"],
            "packages": target["packages"],
            "clippyPassed": clippy_gate_ok,
            "clippyError": clippy_err if not clippy_ok else "",
            "findingCount": len(findings),
            "touchedFindingCount": len(t_findings),
            "touchedFmtDiffCount": len(t_fmt_diffs),
            **hygiene,
        })
        all_findings.extend(findings)
        touched_findings.extend(t_findings)

    cat_counts = collections.Counter(f["category"] for f in all_findings)
    lvl_counts = collections.Counter(f["level"] for f in all_findings)
    return {
        "crateCount": len(targets),
        "passed": all_passed,
        "crates": crates,
        "findings": all_findings,
        "touchedFindings": touched_findings,
        "byLevel": dict(lvl_counts),
        "byCategory": dict(cat_counts),
    }


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    p = argparse.ArgumentParser(description="Rust Static Analysis & Supply-Chain X-Ray")
    p.add_argument("--repo-root", "-r", default=".", help="Repository root path")
    p.add_argument("--target-crate", default="", help="Optional crate/workspace filter")
    p.add_argument("--strict", action="store_true", help="Fail on Clippy warnings (all files unless --touched-only)")
    p.add_argument("--touched-only", action="store_true", help="Gate only on git-touched .rs files (ratchet mode)")
    p.add_argument("--skip-supply-chain", action="store_true", help="Skip cargo-deny and cargo-audit")
    p.add_argument("--sarif-dir", default="", help="Optional explicit directory to emit SARIF files")
    p.add_argument("--json", action="store_true", help="Emit JSON report")
    args = p.parse_args()

    repo_root = Path(args.repo_root).resolve()
    touched = get_git_touched_rust_files(repo_root) if args.touched_only else None
    sarif_dir = Path(args.sarif_dir).resolve() if args.sarif_dir else None
    report = scan_rust_workspace(repo_root, args.target_crate, args.strict, args.skip_supply_chain, touched, args.touched_only, sarif_dir)

    if args.json:
        print(json.dumps(report, indent=2, ensure_ascii=False))
        return 0 if report["passed"] else 1

    print(f"# Rust Static Analysis & Supply-Chain X-Ray ({repo_root})")
    if report["crateCount"] == 0:
        print("- No Cargo.toml manifests found.")
        return 0

    for c in report["crates"]:
        pkgs = ", ".join(c["packages"])
        print(
            f"- **{c['crate']}** (`{c['manifest']}`, profile={c['profile']}, members=[{pkgs}]): "
            f"clippy={'PASS' if c['clippyPassed'] else 'FAIL'} ({c['findingCount']} total, {c['touchedFindingCount']} touched) | "
            f"rustfmt={'PASS' if c['rustfmtPassed'] else 'FAIL'} | "
            f"cargo-deny={c['cargoDeny'].upper()} | cargo-audit={c['cargoAudit'].upper()}"
        )
    shown = report["touchedFindings"] if args.touched_only else report["findings"]
    for f in shown[:25]:
        print(f"  - `{f['file']}:{f['line']}` [{f['ruleId']}] ({f['category']}) {f['message']}")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
