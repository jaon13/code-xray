#!/usr/bin/env python3
"""Cross-platform Stop governance gate hook (fast consistency + optional rustfmt touched-line check)."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HOOK_DIR = Path(__file__).resolve().parent
SCRIPTS_DIR = HOOK_DIR.parent
TOOLKIT_ROOT = SCRIPTS_DIR.parent
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

from rust_profiles import collect_rust_consistency_findings  # noqa: E402

SERILOG_RE = re.compile(r"\busing\s+Serilog\b")
SKIP_SEGS = {"build", "bin", "obj", "target", "target-final", ".agents", "node_modules", "dist", "dist-ext", "work"}
DIFF_FILE_HEADER_RE = re.compile(r"^diff --git a/.+? b/(.+)$")
HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")
FMT_DIFF_LINE_RE = re.compile(r"^Diff in (?:\\\\\?\\)?(.+?):(\d+):?\s*$")


def _resolve_repo_root(raw_arg: str, payload: dict) -> Path:
    if raw_arg and raw_arg.lower() != "auto":
        return Path(raw_arg).resolve()
    cwd = payload.get("cwd")
    if isinstance(cwd, str) and cwd.strip() and Path(cwd).exists():
        return Path(cwd).resolve()
    ws_paths = payload.get("workspacePaths") or []
    if isinstance(ws_paths, list):
        for wp in ws_paths:
            if isinstance(wp, str) and wp.strip() and Path(wp).exists():
                return Path(wp).resolve()
    return Path(".").resolve()


def _norm_rel(repo_root: Path, raw_path: str) -> str:
    cleaned = raw_path[4:] if raw_path.startswith("\\\\?\\") else raw_path
    p = Path(cleaned)
    try:
        return p.resolve(strict=False).relative_to(repo_root).as_posix().lower()
    except ValueError:
        return cleaned.replace("\\", "/").lower()


def _collect_touched_line_ranges(repo_root: Path, untracked: set[str], modified: list[str]) -> dict[str, list[tuple[int, int]] | None]:
    ranges: dict[str, list[tuple[int, int]] | None] = {u.lower(): None for u in untracked}
    if not modified:
        return ranges
    for m in modified:
        ranges[m.lower()] = []
    for extra in ([], ["--cached"]):
        proc = subprocess.run(
            ["git", "-C", str(repo_root), "diff", "-U0", *extra, "--", *modified],
            capture_output=True, text=True, encoding="utf-8", errors="replace", check=False,
        )
        if proc.returncode != 0:
            continue
        cur_file = ""
        for line in proc.stdout.splitlines():
            fm = DIFF_FILE_HEADER_RE.match(line)
            if fm:
                cur_file = fm.group(1).strip('"').replace("\\", "/").lower()
                continue
            hm = HUNK_RE.match(line)
            if hm and cur_file in ranges and ranges[cur_file] is not None:
                start = int(hm.group(1))
                cnt = int(hm.group(2) or "1")
                if cnt > 0:
                    ranges[cur_file].append((max(1, start - 2), start + cnt + 2))
    return ranges


def _find_rustfmt_touched_files(repo_root: Path, rs_files: list[tuple[str, Path]], ranges: dict[str, list[tuple[int, int]] | None]) -> list[str]:
    if not rs_files or not shutil.which("cargo"):
        return []
    ws_roots: set[Path] = set()
    for _, full in rs_files:
        cur = full.parent
        candidate = None
        for _ in range(7):
            cargo = cur / "Cargo.toml"
            if cargo.is_file():
                candidate = cur
                if "[workspace]" in cargo.read_text(encoding="utf-8", errors="replace"):
                    break
            if cur.parent == cur:
                break
            cur = cur.parent
        if candidate:
            ws_roots.add(candidate)

    bad_files: set[str] = set()
    for ws in sorted(ws_roots):
        proc = subprocess.run(["cargo", "fmt", "--all", "--", "--check"], cwd=ws, capture_output=True, text=True, encoding="utf-8", errors="replace", check=False)
        if proc.returncode == 0:
            continue
        for line in proc.stdout.splitlines():
            m = FMT_DIFF_LINE_RE.match(line.strip())
            if not m:
                continue
            rel_low = _norm_rel(repo_root, m.group(1))
            if rel_low not in ranges:
                continue
            ln = int(m.group(2))
            file_ranges = ranges[rel_low]
            if file_ranges is None or any(lo <= ln <= hi for lo, hi in file_ranges):
                bad_files.add(rel_low)

    return [rel for rel, _ in rs_files if rel.lower() in bad_files]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", default="auto")
    parser.add_argument("--lang-filter", choices=("all", "rust", "dotnet", "web", "python"), default="all")
    parser.add_argument("--rustfmt-check", action="store_true", help="Also verify rustfmt on touched .rs lines")
    args = parser.parse_args()

    raw = sys.stdin.read()
    if not raw.strip():
        print('{"decision":"stop"}')
        return 0

    try:
        payload = json.loads(raw)
        if payload.get("continuationActive") is True or str(payload.get("terminationReason", "")) != "NO_TOOL_CALL":
            print('{"decision":"stop"}')
            return 0

        repo_root = _resolve_repo_root(args.repo_root, payload)
        proc = subprocess.run(
            ["git", "-C", str(repo_root), "status", "--porcelain", "-uall"],
            capture_output=True, text=True, encoding="utf-8", errors="replace", check=False,
        )
        if proc.returncode != 0 or not proc.stdout.strip():
            print('{"decision":"stop"}')
            return 0

        ext_map = {
            "rust": {".rs"},
            "dotnet": {".cs", ".xaml"},
            "web": {".ts", ".tsx", ".js", ".jsx", ".svelte", ".css", ".html"},
            "python": {".py"},
            "all": {".cs", ".xaml", ".rs", ".ts", ".tsx", ".js", ".jsx", ".svelte", ".css", ".html", ".py"},
        }
        allowed_exts = ext_map[args.lang_filter]
        violations: list[str] = []
        rs_files: list[tuple[str, Path]] = []
        poly_files: list[Path] = []
        untracked_rs: set[str] = set()
        modified_rs: list[str] = []

        for line in proc.stdout.splitlines():
            if len(line) <= 3:
                continue
            status_code = line[:2]
            rel = line[3:].strip().strip('"').split(" -> ")[-1].strip('"').replace("\\", "/")
            low = rel.lower()
            if any(seg in low.split("/") for seg in SKIP_SEGS):
                continue
            full = repo_root / rel
            if not full.is_file() or full.suffix.lower() not in allowed_exts:
                continue

            text = full.read_text(encoding="utf-8-sig", errors="replace")
            if low.endswith((".cs", ".xaml")):
                line_count = len(text.splitlines())
                if line_count > 300:
                    violations.append(f"{rel} ({line_count} lines > 300 limit)")
                if low.endswith(".cs") and not low.endswith("governancetests.cs"):
                    if ".ConfigureAwait(" in text:
                        violations.append(f"{rel} (banned .ConfigureAwait usage)")
                    if SERILOG_RE.search(text):
                        violations.append(f"{rel} (banned Serilog usage)")
            elif low.endswith(".rs"):
                rs_files.append((rel, full))
                if status_code == "??":
                    untracked_rs.add(rel)
                else:
                    modified_rs.append(rel)
                for item in collect_rust_consistency_findings(full, rel, text):
                    if item["severity"] == "fail":
                        violations.append(f"{rel}:{item['line']} ({item['id']}: {item['message']})")
            else:
                poly_files.append(full)

        if poly_files and args.lang_filter in ("web", "python", "all"):
            sg_exe = shutil.which("ast-grep.cmd" if os.name == "nt" else "ast-grep") or shutil.which("ast-grep") or shutil.which("sg")
            cfg = repo_root / "sgconfig.yml"
            if not cfg.is_file():
                cfg = TOOLKIT_ROOT / "templates" / "ast-grep" / "sgconfig.yml"
            if sg_exe and cfg.is_file():
                sg_proc = subprocess.run(
                    [sg_exe, "scan", "--config", str(cfg), "--json=compact", *[str(p) for p in poly_files]],
                    cwd=repo_root,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    check=False,
                )
                if sg_proc.stdout.strip().startswith("["):
                    for hit in json.loads(sg_proc.stdout):
                        if str(hit.get("severity", "")).lower() == "error":
                            ln = int(((hit.get("range") or {}).get("start") or {}).get("line", 0)) + 1
                            violations.append(f"{hit.get('file', '')}:{ln} ({hit.get('ruleId', 'ast-grep')}: {hit.get('message', '')})")

        if args.rustfmt_check and rs_files:
            ranges = _collect_touched_line_ranges(repo_root, untracked_rs, modified_rs)
            for bad_rel in _find_rustfmt_touched_files(repo_root, rs_files, ranges):
                violations.append(f"{bad_rel} (unformatted touched lines; run `cargo fmt`)")

        if violations:
            summary = "; ".join(violations[:5])
            conv_id = str(payload.get("conversationId", ""))
            digest = hashlib.sha256(f"{conv_id}|{repo_root}|{summary}".encode("utf-8")).hexdigest()[:16]
            flag_path = Path(tempfile.gettempdir()) / f"stop_guard_{digest}.flag"
            if not flag_path.exists():
                flag_path.write_text(summary, encoding="utf-8")
                print(json.dumps({
                    "decision": "continue",
                    "reason": f"[Stop Governance Gate] Modified file(s) violate static analysis rules: {summary}. Resolve before finishing.",
                }, ensure_ascii=False))
                return 0
    except Exception:
        pass

    print('{"decision":"stop"}')
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
