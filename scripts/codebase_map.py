#!/usr/bin/env python3
"""Cross-platform .NET 10 Codebase Map (Solution/Project Graph + Entry Points + Large Files)."""

from __future__ import annotations

import argparse
import collections
import json
import os
import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

EXCLUDED_DIRS = {"build", "bin", "obj", ".git", ".vs", ".agents", "node_modules", "testresults", "target"}
ENTRY_EXACT = {"program.cs", "app.xaml", "app.xaml.cs", "startup.cs", "host.cs"}
ENTRY_RE = re.compile(r"^(program|startup|.*host)\.cs$|^app\.xaml(\.cs)?$", re.I)


def guess_layer(proj_name: str, rel_path: str) -> str:
    hay = f"{proj_name}|{rel_path}".lower()
    if ".tests" in hay or "/tests/" in hay or "test/" in hay:
        return "Tests"
    if "contracts" in hay:
        return "Contracts"
    if any(k in hay for k in ("automation", "windows", "uia", "win32")):
        return "Automation"
    if any(k in hay for k in ("wpf", "ui.", "studio", "view", "xaml")):
        return "UI"
    if any(k in hay for k in ("host", "apphost", "bootstrap")):
        return "Host"
    if ".core" in hay or "core/" in hay:
        return "Core"
    return "Other"


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    p = argparse.ArgumentParser(description=".NET 10 Codebase Orientation Map (cross-platform Python)")
    p.add_argument("--repo-root", "-r", default=".")
    p.add_argument("--focus", "-f", default="")
    p.add_argument("--line-warn-threshold", type=int, default=240)
    p.add_argument("--line-max-limit", type=int, default=300)
    p.add_argument("--top-large", type=int, default=20)
    p.add_argument("--json", action="store_true")
    args = p.parse_args()

    repo_root = Path(args.repo_root).resolve()
    slns = sorted(x.name for x in list(repo_root.glob("*.slnx")) + list(repo_root.glob("*.sln")))

    csproj_files: list[Path] = []
    entry_points: list[str] = []
    code_files: list[Path] = []

    for cur, dirs, files in os.walk(repo_root):
        dirs[:] = [d for d in dirs if d.lower() not in EXCLUDED_DIRS and "_wpftmp" not in d.lower()]
        for fname in files:
            low = fname.lower()
            if "_wpftmp" in low:
                continue
            full = Path(cur) / fname
            rel = full.relative_to(repo_root).as_posix()
            if low.endswith(".csproj"):
                csproj_files.append(full)
            if low in ENTRY_EXACT or ENTRY_RE.match(fname):
                entry_points.append(rel)
            if low.endswith((".cs", ".xaml")):
                code_files.append(full)

    entry_points.sort()

    projects: list[dict] = []
    proj_graph: dict[str, list[str]] = {}
    for cp in sorted(csproj_files):
        p_name = cp.stem
        rel = cp.relative_to(repo_root).as_posix()
        refs: set[str] = set()
        try:
            tree = ET.parse(cp)
            for elem in tree.iter():
                if elem.tag.endswith("ProjectReference"):
                    inc = elem.attrib.get("Include", "")
                    if inc:
                        refs.add(Path(inc.replace("\\", "/")).stem)
        except ET.ParseError:
            pass
        sorted_refs = sorted(refs, key=str.lower)
        proj_graph[p_name] = sorted_refs
        projects.append({"name": p_name, "layer": guess_layer(p_name, rel), "path": rel, "refs": sorted_refs})

    large_files: list[dict] = []
    for cf in code_files:
        try:
            lines = len(cf.read_text(encoding="utf-8-sig", errors="replace").splitlines())
        except OSError:
            continue
        if lines >= args.line_warn_threshold:
            large_files.append(
                {
                    "path": cf.relative_to(repo_root).as_posix(),
                    "lines": lines,
                    "overHardLimit": lines > args.line_max_limit,
                }
            )
    large_files.sort(key=lambda x: x["lines"], reverse=True)
    large_files = large_files[: args.top_large]

    focus_hits: list[str] = []
    if args.focus.strip():
        foc_low = args.focus.strip().lower()
        type_re = re.compile(
            rf"^\s*(?:public|internal|sealed|static|partial|\s)*\b(?:class|record|interface|struct|enum)\s+{re.escape(args.focus.strip())}\b",
            re.M,
        )
        seen_hits: set[str] = set()
        for cf in list(csproj_files) + code_files:
            rel = cf.relative_to(repo_root).as_posix()
            if foc_low in cf.name.lower():
                seen_hits.add(rel)
                continue
            if cf.suffix.lower() == ".cs":
                try:
                    head = "\n".join(cf.read_text(encoding="utf-8-sig", errors="replace").splitlines()[:80])
                    if type_re.search(head):
                        seen_hits.add(rel)
                except OSError:
                    pass
        focus_hits = sorted(seen_hits)[:40]

    projects.sort(key=lambda x: (x["layer"], x["name"].lower()))
    result = {
        "repoRoot": str(repo_root),
        "solutions": slns,
        "projectCount": len(projects),
        "projects": projects,
        "projectGraph": proj_graph,
        "entryPoints": entry_points,
        "largeFiles": large_files,
        "focus": args.focus,
        "focusHits": focus_hits,
        "thresholds": {"warn": args.line_warn_threshold, "max": args.line_max_limit},
    }

    if args.json:
        print(json.dumps(result, indent=2, ensure_ascii=False))
        return 0

    print("============================================================")
    print(f" .NET 10 Codebase Map ({repo_root})")
    print("============================================================\n")
    print(f"[Solutions] {', '.join(slns)}")
    print(f"[Projects]  {len(projects)}\n")
    print("[Layers]")
    by_layer: dict[str, list[str]] = collections.defaultdict(list)
    for pr in projects:
        by_layer[pr["layer"]].append(pr["name"])
    for layer in sorted(by_layer):
        print(f"  {layer:<12} {', '.join(by_layer[layer])}")

    print("\n[ProjectReference edges]")
    for name in sorted(proj_graph, key=str.lower):
        refs = proj_graph[name]
        print(f"  {name} -> {', '.join(refs) if refs else '(none)'}")

    print("\n[Entry points]")
    for ep in entry_points or ["(none detected)"]:
        print(f"  - {ep}" if entry_points else f"  {ep}")

    print(f"\n[Large files >= {args.line_warn_threshold} lines] (hard max {args.line_max_limit})")
    if not large_files:
        print("  (none)")
    for lf in large_files:
        flag = f" FAIL>{args.line_max_limit}" if lf["overHardLimit"] else ""
        print(f"  {lf['lines']:5d}  {lf['path']}{flag}")

    if args.focus.strip():
        print(f"\n[Focus hits] '{args.focus}'")
        for fh in focus_hits or ["(none)"]:
            print(f"  - {fh}" if focus_hits else f"  {fh}")

    print("\nNext: arch-blueprint (multi-file) | dotnet-xray (SARIF debt) | implement + code-consistency")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
