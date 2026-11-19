#!/usr/bin/env python3
"""Review-First grep manifest inspector, marker, and dry-run/apply tool."""

from __future__ import annotations

import argparse
import collections
import difflib
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
VALID_DISPOSITIONS = ("modify", "skip", "manual")
CONSISTENCY_EXTS = {".cs", ".xaml", ".rs"}


def _load_manifest(path: Path) -> dict[str, Any]:
    if not path.is_file():
        raise FileNotFoundError(f"Review manifest not found: {path}")
    data = json.loads(path.read_text(encoding="utf-8-sig"))
    if not isinstance(data, dict) or not isinstance(data.get("matches"), list):
        raise ValueError(f"Invalid review manifest JSON: {path}")
    return data


def _atomic_write_text(path: Path, content: str) -> None:
    tmp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    tmp.write_text(content, encoding="utf-8")
    os.replace(tmp, path)


def _recompute_summary(manifest: dict[str, Any]) -> None:
    matches = manifest.get("matches") or []
    by_cat: dict[str, int] = collections.Counter()
    by_proj: dict[str, int] = collections.Counter()
    by_disp: dict[str, int] = collections.Counter()
    by_status: dict[str, int] = collections.Counter()
    for m in matches:
        by_cat[str(m.get("category") or "code")] += 1
        by_proj[str(m.get("projectName") or m.get("project_name") or "Unknown")] += 1
        by_disp[str(m.get("disposition") or "manual")] += 1
        by_status[str(m.get("status") or "pending")] += 1
    manifest["totalMatches"] = len(matches)
    manifest["summary"] = {
        "byCategory": dict(sorted(by_cat.items())),
        "byProject": dict(sorted(by_proj.items())),
        "byDisposition": dict(sorted(by_disp.items())),
        "byStatus": dict(sorted(by_status.items())),
    }


def _render_companion_markdown(manifest: dict[str, Any], json_path: Path) -> str:
    lines = [
        f"# Grep Review Checklist (`{manifest.get('runId', '')}`)",
        f"- Pattern: `{manifest.get('pattern', '')}` (regex={manifest.get('isRegex', False)})",
        f"- Repo: `{manifest.get('repoRoot', '')}`",
        f"- Manifest: `{json_path.as_posix()}`",
        "",
    ]
    grouped: dict[str, list[dict[str, Any]]] = collections.defaultdict(list)
    for m in manifest.get("matches") or []:
        grouped[str(m.get("file") or "")].append(m)

    for rel_file, items in grouped.items():
        first = items[0]
        proj = first.get("projectName") or first.get("project_name") or "Unknown"
        cat = first.get("category") or "code"
        lines.append(f"## `{rel_file}` ({proj}, category={cat})")
        for m in items:
            sym = m.get("enclosingSymbol") or m.get("enclosing_symbol") or "-"
            status = str(m.get("status") or "pending")
            mark = "x" if status in ("reviewed", "applied") and m.get("disposition") != "manual" else " "
            snippet = str(m.get("snippet") or "").strip()
            lines.append(
                f"- [{mark}] `{m.get('id')}` L{m.get('line')}:{m.get('column', 1)} `{sym}` "
                f"[{m.get('category', 'code')}] ({m.get('disposition', 'manual')}/{status}) — `{snippet}`"
            )
        lines.append("")
    return "\n".join(lines)


def _save_manifest(manifest_path: Path, manifest: dict[str, Any]) -> None:
    _recompute_summary(manifest)
    _atomic_write_text(
        manifest_path,
        json.dumps(manifest, indent=2, ensure_ascii=False) + "\n",
    )
    md_path = manifest_path.with_suffix(".md")
    if md_path.exists():
        _atomic_write_text(md_path, _render_companion_markdown(manifest, manifest_path))


def list_manifests(reviews_dir: Path, limit: int = 20) -> list[dict[str, Any]]:
    if not reviews_dir.is_dir():
        return []
    files = sorted(
        reviews_dir.glob("grep_*.json"),
        key=lambda p: (p.stat().st_mtime, p.name),
        reverse=True,
    )
    items: list[dict[str, Any]] = []
    for path in files[: max(1, limit)]:
        try:
            data = _load_manifest(path)
        except (OSError, ValueError, json.JSONDecodeError):
            continue
        _recompute_summary(data)
        items.append(
            {
                "runId": data.get("runId", path.stem),
                "createdAt": data.get("createdAt") or data.get("createdAtUtc", ""),
                "pattern": data.get("pattern", ""),
                "totalMatches": len(data.get("matches") or []),
                "summary": data.get("summary", {}),
                "manifestPath": path.as_posix(),
            }
        )
    return items


def filter_matches(
    manifest: dict[str, Any],
    *,
    category: str | None = None,
    disposition: str | None = None,
    status: str | None = None,
) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for m in manifest.get("matches") or []:
        if category and str(m.get("category", "")).lower() != category.lower():
            continue
        if disposition and str(m.get("disposition", "")).lower() != disposition.lower():
            continue
        if status and str(m.get("status", "")).lower() != status.lower():
            continue
        out.append(m)
    return out


def mark_manifest(
    manifest_path: Path,
    *,
    ids: list[str],
    mark_all: bool,
    category: str | None,
    file_contains: str | None,
    disposition: str,
    note: str | None = None,
) -> dict[str, Any]:
    if disposition not in VALID_DISPOSITIONS:
        raise ValueError(f"Invalid disposition '{disposition}'; expected one of {VALID_DISPOSITIONS}")
    if not (ids or mark_all or category or file_contains):
        raise ValueError("Specify at least one selector: --ids, --category, --file-contains, or --all")

    manifest = _load_manifest(manifest_path)
    id_set = {x.strip().upper() for x in ids if x.strip()}
    updated_ids: list[str] = []

    for m in manifest.get("matches") or []:
        mid = str(m.get("id", "")).upper()
        if id_set and mid not in id_set:
            continue
        if category and str(m.get("category", "")).lower() != category.lower():
            continue
        if file_contains and file_contains.lower() not in str(m.get("file", "")).lower():
            continue
        m["disposition"] = disposition
        if m.get("status") != "applied":
            m["status"] = "reviewed"
        if note is not None:
            m["note"] = note
        updated_ids.append(str(m.get("id", "")))

    _save_manifest(manifest_path, manifest)
    return {
        "manifestPath": manifest_path.as_posix(),
        "disposition": disposition,
        "updatedCount": len(updated_ids),
        "updatedIds": updated_ids,
        "summary": manifest.get("summary", {}),
    }


def _resolve_target_file(entry: dict[str, Any], repo_root: Path | None) -> Path:
    abs_raw = str(entry.get("absPath") or "").strip()
    if abs_raw:
        p = Path(abs_raw)
        if p.is_file():
            return p
    rel_raw = str(entry.get("file") or "").strip()
    if repo_root is not None and rel_raw:
        p = (repo_root / rel_raw).resolve()
        if p.is_file():
            return p
    return Path(abs_raw or rel_raw)


def _read_lines_preserving_style(path: Path) -> tuple[bool, str, list[str]]:
    raw = path.read_bytes()
    has_bom = raw.startswith(b"\xef\xbb\xbf")
    text = raw[3:].decode("utf-8") if has_bom else raw.decode("utf-8")
    eol = "\r\n" if "\r\n" in text else "\n"
    lines = text.splitlines(keepends=False)
    trailing_nl = text.endswith("\n")
    return has_bom, eol, lines + ([""] if trailing_nl and not lines else [])


def _write_lines_preserving_style(
    path: Path,
    has_bom: bool,
    eol: str,
    lines: list[str],
    had_trailing_nl: bool,
) -> None:
    joined = eol.join(lines)
    if had_trailing_nl and not joined.endswith(("\n", "\r")):
        joined += eol
    payload = joined.encode("utf-8")
    if has_bom:
        payload = b"\xef\xbb\xbf" + payload
    path.write_bytes(payload)


def apply_manifest(
    manifest_path: Path,
    *,
    replace_old: str,
    replace_new: str,
    is_regex: bool = False,
    dry_run: bool = False,
    repo_root: Path | None = None,
    run_consistency: bool = True,
) -> dict[str, Any]:
    if not replace_old:
        raise ValueError("--replace must not be empty")

    manifest = _load_manifest(manifest_path)
    effective_root = repo_root or (
        Path(str(manifest["repoRoot"])) if manifest.get("repoRoot") else manifest_path.parent.parent.parent
    )
    regex = re.compile(replace_old) if is_regex else None

    targets_by_file: dict[Path, list[dict[str, Any]]] = collections.defaultdict(list)
    skipped_count = 0
    for m in manifest.get("matches") or []:
        if m.get("disposition") == "modify" and m.get("status") != "applied":
            tpath = _resolve_target_file(m, effective_root)
            targets_by_file[tpath].append(m)
        else:
            skipped_count += 1

    diffs: list[str] = []
    modified_files: list[str] = []
    applied_ids: list[str] = []
    conflicts: list[dict[str, Any]] = []

    for fpath in sorted(targets_by_file.keys(), key=lambda p: p.as_posix()):
        if not fpath.is_file():
            for m in targets_by_file[fpath]:
                conflicts.append({"id": m.get("id"), "file": fpath.as_posix(), "reason": "file_missing"})
            continue

        raw_bytes = fpath.read_bytes()
        has_bom = raw_bytes.startswith(b"\xef\xbb\xbf")
        text = raw_bytes[3:].decode("utf-8") if has_bom else raw_bytes.decode("utf-8")
        eol = "\r\n" if "\r\n" in text else "\n"
        had_trailing_nl = text.endswith("\n")
        body = text[:-1] if had_trailing_nl else text
        old_lines = [ln[:-1] if ln.endswith("\r") else ln for ln in body.split("\n")] if body else []
        new_lines = list(old_lines)

        file_applied: list[dict[str, Any]] = []
        for m in targets_by_file[fpath]:
            line_num = int(m.get("line") or 0)
            idx = line_num - 1
            if idx < 0 or idx >= len(new_lines):
                conflicts.append({"id": m.get("id"), "file": fpath.as_posix(), "line": line_num, "reason": "line_out_of_range"})
                continue
            current = new_lines[idx]
            if regex is not None:
                updated, n_subs = regex.subn(replace_new, current)
            else:
                n_subs = current.count(replace_old)
                updated = current.replace(replace_old, replace_new) if n_subs > 0 else current
            if n_subs == 0:
                conflicts.append({"id": m.get("id"), "file": fpath.as_posix(), "line": line_num, "reason": "pattern_not_found_on_line"})
                continue
            new_lines[idx] = updated
            file_applied.append(m)

        if not file_applied:
            continue

        rel_display = (
            fpath.relative_to(effective_root).as_posix()
            if fpath.is_relative_to(effective_root)
            else fpath.as_posix()
        )
        diff_lines = list(
            difflib.unified_diff(
                old_lines,
                new_lines,
                fromfile=f"a/{rel_display}",
                tofile=f"b/{rel_display}",
                lineterm="",
            )
        )
        if diff_lines:
            diffs.append("\n".join(diff_lines))

        modified_files.append(fpath.as_posix())
        for m in file_applied:
            applied_ids.append(str(m.get("id", "")))
            if not dry_run:
                m["status"] = "applied"

        if not dry_run:
            _write_lines_preserving_style(fpath, has_bom, eol, new_lines, had_trailing_nl)

    if not dry_run and applied_ids:
        _save_manifest(manifest_path, manifest)

    consistency_targets = [f for f in modified_files if Path(f).suffix.lower() in CONSISTENCY_EXTS]
    consistency_cmd = [
        sys.executable,
        str(SCRIPT_DIR / "check_consistency.py"),
        "--repo-root",
        str(effective_root),
    ]
    if consistency_targets:
        consistency_cmd.extend(["--path", *consistency_targets])
    consistency_hint = " ".join(f'"{c}"' if " " in c else c for c in consistency_cmd)

    consistency_result: dict[str, Any] | None = None
    if not dry_run and run_consistency and consistency_targets:
        proc = subprocess.run(
            consistency_cmd,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
        consistency_result = {
            "exitCode": proc.returncode,
            "passed": proc.returncode == 0,
            "output": (proc.stdout or proc.stderr).strip(),
        }

    return {
        "manifestPath": manifest_path.as_posix(),
        "dryRun": dry_run,
        "appliedCount": len(applied_ids),
        "appliedIds": applied_ids,
        "skippedCount": skipped_count,
        "modifiedFiles": modified_files,
        "conflicts": conflicts,
        "diffs": diffs,
        "consistencyHint": consistency_hint,
        "consistencyResult": consistency_result,
    }


def _parse_ids(raw_ids: list[str]) -> list[str]:
    out: list[str] = []
    for token in raw_ids:
        for part in token.split(","):
            s = part.strip()
            if s:
                out.append(s)
    return out


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Review-First grep manifest manager & batch apply CLI.")
    sub = parser.add_subparsers(dest="command", required=True)

    p_list = sub.add_parser("list", help="List isolated grep review manifests in .xray/reviews/")
    p_list.add_argument("--repo-root", "-r", default=".", help="Repository root")
    p_list.add_argument("--reviews-dir", default="", help="Override reviews directory")
    p_list.add_argument("--limit", type=int, default=20)
    p_list.add_argument("--json", action="store_true")

    p_show = sub.add_parser("show", help="Show matches in a grep review manifest")
    p_show.add_argument("manifest", help="Path to grep_<ts>_<id>.json")
    p_show.add_argument("--category", choices=("code", "test", "config", "docs"))
    p_show.add_argument("--disposition", choices=VALID_DISPOSITIONS)
    p_show.add_argument("--status", choices=("pending", "reviewed", "applied"))
    p_show.add_argument("--json", action="store_true")

    p_mark = sub.add_parser("mark", help="Mark match dispositions in a review manifest")
    p_mark.add_argument("manifest", help="Path to grep_<ts>_<id>.json")
    p_mark.add_argument("--ids", nargs="+", default=[], help="Match IDs (e.g. M001 M002 or M001,M002)")
    p_mark.add_argument("--all", action="store_true", help="Select all matches (subject to optional filters)")
    p_mark.add_argument("--category", choices=("code", "test", "config", "docs"))
    p_mark.add_argument("--file-contains", default="", help="Case-insensitive file path substring filter")
    p_mark.add_argument("--disposition", required=True, choices=VALID_DISPOSITIONS)
    p_mark.add_argument("--note", default=None)
    p_mark.add_argument("--json", action="store_true")

    p_apply = sub.add_parser("apply", help="Apply literal/regex replacement ONLY to disposition=='modify' entries")
    p_apply.add_argument("manifest", help="Path to grep_<ts>_<id>.json")
    p_apply.add_argument("--replace", required=True, help="Text or regex to replace on approved match lines")
    p_apply.add_argument("--with", dest="replace_with", required=True, help="Replacement text")
    p_apply.add_argument("--regex", action="store_true", help="Treat --replace as a Python regex")
    p_apply.add_argument("--dry-run", action="store_true", help="Preview unified diff without editing files")
    p_apply.add_argument("--repo-root", "-r", default="", help="Optional repo root override")
    p_apply.add_argument("--no-check", action="store_true", help="Skip automatic check_consistency.py execution")
    p_apply.add_argument("--json", action="store_true")
    return parser


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    args = _build_parser().parse_args()
    if args.command == "list":
        rdir = Path(args.reviews_dir).resolve() if args.reviews_dir else Path(args.repo_root).resolve() / ".xray" / "reviews"
        items = list_manifests(rdir, args.limit)
        if args.json:
            print(json.dumps({"reviewsDir": rdir.as_posix(), "manifests": items}, indent=2, ensure_ascii=False))
            return 0
        if not items:
            print(f"No grep review manifests in {rdir}")
            return 0
        for item in items:
            disp = item.get("summary", {}).get("byDisposition", {})
            print(f"{item['runId']} | {item['createdAt']} | pattern={item['pattern']!r} | matches={item['totalMatches']} | disp={disp} | {item['manifestPath']}")
        return 0

    if args.command == "show":
        mpath = Path(args.manifest).resolve()
        manifest = _load_manifest(mpath)
        _recompute_summary(manifest)
        filtered = filter_matches(manifest, category=args.category, disposition=args.disposition, status=args.status)
        if args.json:
            print(json.dumps({"runId": manifest.get("runId"), "pattern": manifest.get("pattern"), "summary": manifest.get("summary"), "matches": filtered}, indent=2, ensure_ascii=False))
            return 0
        print(f"Manifest: {mpath} | pattern={manifest.get('pattern')!r} | showing {len(filtered)}/{len(manifest.get('matches') or [])}")
        for m in filtered:
            sym = m.get("enclosingSymbol") or m.get("enclosing_symbol") or "-"
            print(f"  {m.get('id')} | {m.get('file')}:{m.get('line')} | {m.get('category')} | {sym} | {m.get('disposition')}/{m.get('status')} | {str(m.get('snippet', '')).strip()}")
        return 0

    if args.command == "mark":
        res = mark_manifest(
            Path(args.manifest).resolve(),
            ids=_parse_ids(args.ids),
            mark_all=args.all,
            category=args.category,
            file_contains=args.file_contains or None,
            disposition=args.disposition,
            note=args.note,
        )
        if args.json:
            print(json.dumps(res, indent=2, ensure_ascii=False))
        else:
            print(f"Marked {res['updatedCount']} match(es) as '{res['disposition']}': {', '.join(res['updatedIds']) or '(none)'}")
        return 0

    repo_override = Path(args.repo_root).resolve() if args.repo_root else None
    res = apply_manifest(
        Path(args.manifest).resolve(),
        replace_old=args.replace,
        replace_new=args.replace_with,
        is_regex=args.regex,
        dry_run=args.dry_run,
        repo_root=repo_override,
        run_consistency=not args.no_check,
    )
    if args.json:
        print(json.dumps(res, indent=2, ensure_ascii=False))
        return 0

    mode = "DRY-RUN" if res["dryRun"] else "APPLIED"
    print(f"[{mode}] matches={res['appliedCount']} files={len(res['modifiedFiles'])} skipped={res['skippedCount']} conflicts={len(res['conflicts'])}")
    for diff in res["diffs"]:
        print(diff)
    if res["conflicts"]:
        print("Conflicts:")
        for c in res["conflicts"]:
            print(f"  - {c}")
    print(f"Consistency hint: {res['consistencyHint']}")
    if res["consistencyResult"] is not None:
        print(f"Consistency exit={res['consistencyResult']['exitCode']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
