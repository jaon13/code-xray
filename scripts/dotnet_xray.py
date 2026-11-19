#!/usr/bin/env python3
"""Cross-platform .NET 10 Codebase X-Ray (SARIF v2.1 + Blast-Radius + 300-Line + Rust Clones)."""

from __future__ import annotations

import argparse
import collections
import json
import os
import re
import shutil
import subprocess
import sys
import urllib.parse
import xml.etree.ElementTree as ET
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
TOOLKIT_ROOT = SCRIPT_DIR.parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from rust_xray import scan_rust_workspace  # noqa: E402
from xaml_style_xray import scan_xaml_repo  # noqa: E402
EXCLUDED_DIRS = {"build", "bin", "obj", ".git", ".vs", ".agents", "node_modules", "testresults", "target"}
PARTIAL_RE = re.compile(r"^([A-Za-z0-9_]+)\.[A-Za-z0-9_]+$")

CATEGORY_WEIGHTS = {
    "BannedAPI/Governance": 5,
    "Threading/Async": 4,
    "Resource/Disposal/Cancel": 4,
    "Security": 4,
    "Complexity/Coupling/SRP": 3,
    "Exception/NullSafety": 3,
    "Performance/ModernDotNet": 2,
    "Style/Maintainability": 1,
}


def classify_rule(rule_id: str) -> str:
    if re.match(r"^(VSTHRD|CS4014|CS1998)", rule_id):
        return "Threading/Async"
    if re.match(r"^RS003[01]$", rule_id):
        return "BannedAPI/Governance"
    if re.match(r"^(IDISP\d+|CA1001|CA1816|CA2000|CA2016|CA2213|MA0040|S2930|S2931)$", rule_id):
        return "Resource/Disposal/Cancel"
    if re.match(r"^(S3776|CA1502|CA1505|CA1506|MA0051|S107|S138|S1200)$", rule_id):
        return "Complexity/Coupling/SRP"
    if re.match(r"^(CA1031|CA2200|S108|S112|S2486|CS86\d\d|S2259)$", rule_id):
        return "Exception/NullSafety"
    if re.match(r"^(CA18\d\d|SYSLIB\d+|CA1307|CA1309|CA1310|MA0002|MA0006|IDE0051|IDE0052|RCS1077|RCS1213)$", rule_id):
        return "Performance/ModernDotNet"
    if re.match(r"^(CA2100|CA23\d\d|CA30\d\d|CA53\d\d|S2077|S2083)$", rule_id):
        return "Security"
    return "Style/Maintainability"


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


def walk_repo_files(repo_root: Path, suffixes: tuple[str, ...]) -> list[Path]:
    matched: list[Path] = []
    for cur_root, dirs, files in os.walk(repo_root):
        dirs[:] = [d for d in dirs if d.lower() not in EXCLUDED_DIRS and "_wpftmp" not in d.lower()]
        for fname in files:
            low = fname.lower()
            if "_wpftmp" in low:
                continue
            if low.endswith(suffixes):
                matched.append(Path(cur_root) / fname)
    return matched


def build_project_graph(repo_root: Path) -> tuple[dict[str, list[str]], dict[str, set[str]]]:
    proj_graph: dict[str, list[str]] = {}
    rev_graph: dict[str, set[str]] = collections.defaultdict(set)
    for csproj in walk_repo_files(repo_root, (".csproj",)):
        p_name = csproj.stem
        refs: set[str] = set()
        try:
            tree = ET.parse(csproj)
            for elem in tree.iter():
                if elem.tag.endswith("ProjectReference"):
                    inc = elem.attrib.get("Include", "")
                    if inc:
                        target = Path(inc.replace("\\", "/")).stem
                        refs.add(target)
                        rev_graph[target].add(p_name)
        except ET.ParseError:
            pass
        proj_graph[p_name] = sorted(refs, key=str.lower)
    return proj_graph, rev_graph


def transitive_dependents(root_proj: str, rev_graph: dict[str, set[str]]) -> list[str]:
    visited: set[str] = set()
    queue = collections.deque([root_proj])
    while queue:
        curr = queue.popleft()
        for dep in rev_graph.get(curr, ()):
            if dep not in visited:
                visited.add(dep)
                queue.append(dep)
    return sorted(visited, key=str.lower)


def load_sarif_diagnostics(
    repo_root: Path, config: str, target_project: str, include_notes: bool
) -> tuple[int, list[dict], dict[str, str]]:
    sarif_dir = repo_root / "Build" / "sarif" / config
    sarif_files = sorted(sarif_dir.glob("*.sarif")) if sarif_dir.is_dir() else walk_repo_files(repo_root, (".sarif",))
    seen: set[str] = set()
    diagnostics: list[dict] = []
    rule_descs: dict[str, str] = {}

    for sf in sarif_files:
        proj_owner = sf.stem
        if target_project and target_project.lower() not in proj_owner.lower():
            continue
        try:
            data = json.loads(sf.read_text(encoding="utf-8-sig"))
        except (OSError, json.JSONDecodeError):
            continue

        for run in data.get("runs") or []:
            rules = ((run.get("tool") or {}).get("driver") or {}).get("rules") or []
            for r in rules:
                rid = r.get("id")
                if rid and rid not in rule_descs:
                    desc = (r.get("shortDescription") or {}).get("text") or (r.get("fullDescription") or {}).get("text") or ""
                    rule_descs[rid] = desc

            for res in run.get("results") or []:
                level = res.get("level", "warning")
                if level == "none" or (not include_notes and level == "note"):
                    continue
                rule_id = str(res.get("ruleId", ""))
                locs = res.get("locations") or []
                uri, line = "", 0
                if locs:
                    phys = locs[0].get("physicalLocation") or {}
                    uri = (phys.get("artifactLocation") or {}).get("uri", "")
                    line = int((phys.get("region") or {}).get("startLine") or 0)
                rel_file = rel_repo_path(repo_root, uri)
                if rel_file.lower().startswith(("build/", "obj/", "bin/")):
                    continue
                dedup_key = f"{rel_file.lower()}|{line}|{rule_id.lower()}"
                if dedup_key in seen:
                    continue
                seen.add(dedup_key)
                cat = classify_rule(rule_id)
                msg = (res.get("message") or {}).get("text", "")
                diagnostics.append(
                    {
                        "project": proj_owner,
                        "file": rel_file,
                        "line": line,
                        "level": level,
                        "ruleId": rule_id,
                        "category": cat,
                        "weight": CATEGORY_WEIGHTS[cat],
                        "message": msg,
                    }
                )
    return len(sarif_files), diagnostics, rule_descs


def resolve_structural_clone_exe() -> Path | None:
    env_val = os.environ.get("STRUCTURAL_CLONE_EXE")
    if env_val and Path(env_val).is_file():
        return Path(env_val).resolve()
    bin_name = "structural-clone.exe" if os.name == "nt" else "structural-clone"
    candidates = [
        TOOLKIT_ROOT / "bin" / bin_name,
        TOOLKIT_ROOT / "src" / "code-clone-xray" / "target" / "release" / bin_name,
    ]
    for c in candidates:
        if c.is_file():
            return c.resolve()
    which = shutil.which("structural-clone")
    return Path(which) if which else None


def run_clone_scan(repo_root: Path, extra_paths: list[str], min_lines: int, top: int) -> dict:
    exe = resolve_structural_clone_exe()
    if not exe:
        raise FileNotFoundError(
            f"structural-clone binary not found. Run: python \"{TOOLKIT_ROOT / 'scripts' / 'publish_tools.py'}\""
        )
    src_dir = repo_root / "src"
    targets = [str(src_dir if src_dir.is_dir() else repo_root)]
    native_dir = repo_root / "native"
    if native_dir.is_dir() and native_dir != src_dir:
        targets.append(str(native_dir))
    for extra in extra_paths:
        if extra and Path(extra).exists():
            targets.append(str(Path(extra).resolve()))

    cmd = [str(exe)]
    for t in targets:
        cmd.extend(["--path", t])
    cmd.extend(["--min-lines", str(min_lines), "--top", str(top), "--format", "json"])

    proc = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", check=False)
    if proc.returncode != 0:
        raise RuntimeError(f"structural-clone failed ({proc.returncode}): {proc.stderr}\n{proc.stdout}")
    idx = proc.stdout.find("{")
    if idx < 0:
        raise RuntimeError("structural-clone produced no JSON output.")
    return json.loads(proc.stdout[idx:])


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    p = argparse.ArgumentParser(description=".NET 10 Codebase X-Ray (cross-platform Python)")
    p.add_argument("--repo-root", "-r", default=".", help="Repository root path")
    p.add_argument("--configuration", "-c", default="Debug", help="Build configuration (Debug/Release)")
    p.add_argument("--build", action="store_true", help="Run dotnet build --no-incremental first")
    p.add_argument("--target-project", default="", help="Optional project name filter")
    p.add_argument("--line-warn-threshold", type=int, default=240)
    p.add_argument("--line-max-limit", type=int, default=300)
    p.add_argument("--top", type=int, default=15)
    p.add_argument("--include-notes", action="store_true")
    p.add_argument("--include-tests-in-hotspots", action="store_true")
    p.add_argument("--check-clones", action="store_true", help="Run native Rust C#/XAML/Rust structural clone scan")
    p.add_argument("--additional-clone-paths", nargs="*", default=[], help="Additional directories for cross-repo clone scan")
    p.add_argument("--clone-min-lines", type=int, default=12)
    p.add_argument("--skip-rust", action="store_true", help="Skip Rust Cargo Clippy/Rustfmt/Supply-Chain scan")
    p.add_argument("--skip-supply-chain", action="store_true", help="Skip cargo-deny and cargo-audit during Rust scan")
    p.add_argument("--json", action="store_true", help="Emit JSON report")
    args = p.parse_args()

    repo_root = Path(args.repo_root).resolve()
    if args.build:
        slns = list(repo_root.glob("*.slnx")) + list(repo_root.glob("*.sln"))
        build_target = str(slns[0] if slns else repo_root)
        subprocess.run(["dotnet", "build", build_target, "-c", args.configuration, "--no-incremental", "-v", "quiet"], check=True)

    proj_graph, rev_graph = build_project_graph(repo_root)
    sarif_count, diagnostics, rule_descs = load_sarif_diagnostics(
        repo_root, args.configuration, args.target_project, args.include_notes
    )
    rust_report = (
        None
        if args.skip_rust
        else scan_rust_workspace(repo_root, args.target_project, False, args.skip_supply_chain)
    )

    source_files = walk_repo_files(repo_root, (".cs", ".xaml", ".rs"))
    if args.target_project:
        source_files = [f for f in source_files if args.target_project.lower() in f.as_posix().lower()]

    line_hotspots: list[dict] = []
    partial_groups: collections.Counter[str] = collections.Counter()
    for sf in source_files:
        try:
            line_count = len(sf.read_text(encoding="utf-8-sig", errors="replace").splitlines())
        except OSError:
            continue
        rel = rel_repo_path(repo_root, str(sf))
        if line_count >= args.line_warn_threshold:
            line_hotspots.append(
                {
                    "file": rel,
                    "lines": line_count,
                    "status": "OVER_LIMIT" if line_count > args.line_max_limit else "NEAR_LIMIT",
                }
            )
        if sf.suffix.lower() == ".cs":
            m = PARTIAL_RE.match(sf.stem)
            if m:
                parent_rel = str(Path(rel).parent).replace("\\", "/")
                partial_groups[f"{parent_rel}/{m.group(1)}"] += 1

    line_hotspots.sort(key=lambda x: x["lines"], reverse=True)
    line_hotspots = line_hotspots[: args.top]
    partial_sprawl = [
        {"classPrefix": k, "partialFacetCount": v + 1}
        for k, v in partial_groups.most_common()
        if v >= 4
    ]

    lvl_counts = collections.Counter(d["level"] for d in diagnostics)
    cat_counts = collections.Counter(d["category"] for d in diagnostics)
    rule_counts = collections.Counter(d["ruleId"] for d in diagnostics)
    top_rules = [
        {
            "ruleId": rid,
            "category": classify_rule(rid),
            "count": cnt,
            "description": rule_descs.get(rid) or next((d["message"] for d in diagnostics if d["ruleId"] == rid), ""),
        }
        for rid, cnt in rule_counts.most_common(args.top)
    ]

    by_file: dict[str, list[dict]] = collections.defaultdict(list)
    for d in diagnostics:
        if d["file"] and (args.include_tests_in_hotspots or not d["file"].lower().startswith("tests/")):
            by_file[d["file"]].append(d)

    file_hotspots = []
    for fpath, items in by_file.items():
        risk = sum(i["weight"] for i in items)
        rc = collections.Counter(i["ruleId"] for i in items).most_common(3)
        file_hotspots.append(
            {
                "file": fpath,
                "totalFindings": len(items),
                "riskScore": risk,
                "topRules": ", ".join(f"{r}({c})" for r, c in rc),
            }
        )
    file_hotspots.sort(key=lambda x: x["riskScore"], reverse=True)
    file_hotspots = file_hotspots[: args.top]

    clone_report = run_clone_scan(repo_root, args.additional_clone_paths, args.clone_min_lines, args.top) if args.check_clones else None
    has_xaml = any(sf.suffix.lower() == ".xaml" for sf in source_files)
    xaml_style_report = (
        scan_xaml_repo(repo_root, [Path(p) for p in args.additional_clone_paths if p])
        if has_xaml
        else None
    )

    if args.json:
        print(
            json.dumps(
                {
                    "repoRoot": str(repo_root),
                    "sarifFileCount": sarif_count,
                    "byLevel": dict(lvl_counts),
                    "byCategory": dict(cat_counts),
                    "topRules": top_rules,
                    "staticHotspots": file_hotspots,
                    "lineHotspots": line_hotspots,
                    "partialSprawl": partial_sprawl,
                    "rustAnalysis": rust_report,
                    "cloneScan": clone_report,
                    "xamlStyleAnalysis": xaml_style_report,
                },
                indent=2,
                ensure_ascii=False,
            )
        )
        return 0

    print("# .NET 10 + Rust Codebase X-Ray Report")
    print(f"- **Repository**: {repo_root}")
    print(f"- **Configuration**: {args.configuration} | **SARIF Reports Loaded**: {sarif_count} | **Source Files Scanned**: {len(source_files)}")
    if args.target_project:
        imp = transitive_dependents(args.target_project, rev_graph)
        print(f"- **Target Project**: {args.target_project} -> **Downstream Impacted ({len(imp)})**: {', '.join(imp)}")

    print("\n## 1. Project Dependency & Blast-Radius Summary")
    for p_name in sorted(proj_graph, key=str.lower):
        refs = proj_graph[p_name]
        if args.target_project and args.target_project.lower() not in p_name.lower() and args.target_project not in refs:
            continue
        deps_str = ", ".join(refs) if refs else "(leaf / zero refs)"
        blast = len(transitive_dependents(p_name, rev_graph))
        print(f"- **{p_name}** -> refs: [{deps_str}] | downstream blast radius: {blast}")

    print("\n## 2. Roslyn Static Analysis Scorecard (SARIF v2.1)")
    if not diagnostics:
        print(f"- Zero SARIF diagnostics found (run with `--build` if `Build/sarif/{args.configuration}` is empty).")
    else:
        lvl_str = " | ".join(f"{k}: {v}" for k, v in lvl_counts.most_common())
        print(f"- **Totals**: {len(diagnostics)} findings ({lvl_str})")
        for cat, cnt in cat_counts.most_common():
            print(f"  - **{cat}**: {cnt}")
        print("\n### Top Triggered Rules")
        for r in top_rules:
            desc = re.sub(r"\r?\n", " ", r["description"])
            if len(desc) > 90:
                desc = desc[:87] + "..."
            print(f"- **{r['ruleId']}** [{r['category']}] x{r['count']} — {desc}")
        print("\n### Top Static-Debt Hotspot Files (Weighted Risk)")
        for fh in file_hotspots:
            print(f"- `{fh['file']}` — RiskScore: **{fh['riskScore']}** ({fh['totalFindings']} hits: {fh['topRules']})")

    if rust_report and rust_report["crateCount"] > 0:
        print("\n## 2b. Rust Native Static Analysis & Supply-Chain Scorecard (Clippy + Rustfmt + Deny/Audit)")
        for rc in rust_report["crates"]:
            print(
                f"- **{rc['crate']}** (`{rc['manifest']}`, profile={rc['profile']}) -> "
                f"Clippy: **{'PASS' if rc['clippyPassed'] else 'FAIL'}** ({rc['findingCount']} findings) | "
                f"Rustfmt: **{'PASS' if rc['rustfmtPassed'] else 'FAIL'}** | "
                f"cargo-deny: **{rc['cargoDeny'].upper()}** | "
                f"cargo-audit: **{rc['cargoAudit'].upper()}**"
            )
        for rf in rust_report["findings"][: args.top]:
            print(f"  - `{rf['file']}:{rf['line']}` [{rf['ruleId']}] ({rf['category']}) {rf['message']}")

    print("\n## 3. 300-Line Ceiling & Partial-Class Sprawl Radar (.cs / .xaml / .rs)")
    if not line_hotspots:
        print(f"- All scanned `.cs`/`.xaml`/`.rs` files are under {args.line_warn_threshold} lines.")
    else:
        for lh in line_hotspots:
            print(f"- [{lh['status']}] `{lh['file']}` — **{lh['lines']}** / {args.line_max_limit} lines")
    if partial_sprawl:
        print("\n### High-Facet Partial Clusters (SRP Extraction Candidates)")
        for ps in partial_sprawl:
            print(f"- `{ps['classPrefix']}.*.cs` — ~{ps['partialFacetCount']} partial files")

    if clone_report:
        tot = clone_report["statistics"]["total"]
        print("\n## 4. Code & XAML Duplication / Reuse Scan (structural-clone)")
        print(
            f"- **Overall Duplication**: **{tot['percentage']}%** "
            f"({tot['duplicatedLines']} / {tot['lines']} lines across {tot.get('clonePairs', 0)} clone pairs in {tot.get('files', 0)} files)"
        )
        for fmt_name, ft in (clone_report["statistics"].get("formats") or {}).items():
            print(f"  - **{fmt_name}**: {ft['percentage']}% ({ft.get('clonePairs', 0)} clones, {ft['duplicatedLines']} duplicated lines)")
        dups = clone_report.get("duplicates") or []
        if dups:
            print("\n### Top Duplicated C# / XAML / Rust Blocks (Consolidation Candidates)")
            for d in dups[: args.top]:
                a = rel_repo_path(repo_root, d["firstFile"]["path"])
                b = rel_repo_path(repo_root, d["secondFile"]["path"])
                print(
                    f"- **{d['lines']} lines** [{d['format']}, {d['similarity']}% sim]: "
                    f"`{a}` (L{d['firstFile']['startLine']}-{d['firstFile']['endLine']}) <-> "
                    f"`{b}` (L{d['secondFile']['startLine']}-{d['secondFile']['endLine']})"
                )

    if xaml_style_report:
        print("\n## 5. WPF/XAML Style, Template & ResourceDictionary X-Ray")
        print(
            f"- **XAML Files**: {xaml_style_report['xamlFilesScanned']} | "
            f"**Styles**: {xaml_style_report['stylesFound']} | "
            f"**Templates**: {xaml_style_report['templatesFound']} | "
            f"**Exact Style Clones**: {len(xaml_style_report['exactStyleClones'])} | "
            f"**BasedOn Candidates**: {len(xaml_style_report['basedOnCandidates'])} | "
            f"**Template Clones**: {len(xaml_style_report['templateClones'])} | "
            f"**Inline Clusters**: {len(xaml_style_report['inlinePropertyClusters'])}"
        )
        for cand in xaml_style_report["basedOnCandidates"][: args.top]:
            print(f"  - [BasedOn {cand['jaccardSimilarity']}] {cand['recommendation']}")
        for f in xaml_style_report["findings"][: args.top]:
            print(f"  - [{f['severity'].upper()}] `{f['file']}:{f['line']}` [{f['id']}] {f['message']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
