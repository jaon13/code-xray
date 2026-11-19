#!/usr/bin/env python3
"""Semantic WPF/XAML Style, ControlTemplate, DataTemplate & ResourceDictionary X-Ray."""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import os
import re
import sys
import xml.etree.ElementTree as ET
from dataclasses import asdict, dataclass
from pathlib import Path

EXCLUDED_DIRS = {
    "build",
    "bin",
    "obj",
    ".git",
    ".vs",
    ".agents",
    "node_modules",
    "testresults",
    "target",
    "dist",
}

XAML_NS = "http://schemas.microsoft.com/winfx/2006/xaml"
PRES_OPT_NS = "http://schemas.microsoft.com/winfx/2006/xaml/presentation/options"
X_KEY_ATTR = f"{{{XAML_NS}}}Key"
X_NAME_ATTR = f"{{{XAML_NS}}}Name"
PO_FREEZE_ATTR = f"{{{PRES_OPT_NS}}}Freeze"

HEX_COLOR_RE = re.compile(r"^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$")
STATIC_RES_RE = re.compile(r"\{StaticResource\s+([^\s},]+)")
DYNAMIC_RES_RE = re.compile(r"\{DynamicResource\s+([^\s},]+)")
XTYPE_RE = re.compile(r"\{x:Type\s+([^\s}]+)\}")

FREEZABLE_TAGS = {
    "SolidColorBrush",
    "LinearGradientBrush",
    "RadialGradientBrush",
    "DrawingBrush",
    "ImageBrush",
    "VisualBrush",
    "Pen",
    "PathGeometry",
    "StreamGeometry",
    "GeometryGroup",
}

INLINE_STYLE_CONTROLS = {
    "Button",
    "ToggleButton",
    "RepeatButton",
    "TextBlock",
    "TextBox",
    "PasswordBox",
    "RichTextBox",
    "Border",
    "ComboBox",
    "CheckBox",
    "RadioButton",
    "Label",
    "ListBox",
    "ListView",
    "DataGrid",
    "TabControl",
    "TabItem",
    "GroupBox",
}

VISUAL_ATTRS = {
    "Margin",
    "Padding",
    "FontSize",
    "FontWeight",
    "FontFamily",
    "Foreground",
    "Background",
    "BorderBrush",
    "BorderThickness",
    "CornerRadius",
    "HorizontalAlignment",
    "VerticalAlignment",
    "HorizontalContentAlignment",
    "VerticalContentAlignment",
    "Height",
    "Width",
    "MinHeight",
    "MinWidth",
    "Opacity",
    "Cursor",
}


@dataclass(frozen=True)
class StyleRecord:
    file: str
    line: int
    key: str
    target_type: str
    based_on: str
    setters: tuple[tuple[str, str], ...]
    triggers: tuple[str, ...]
    fingerprint: str


@dataclass(frozen=True)
class TemplateRecord:
    file: str
    line: int
    kind: str
    key: str
    target_or_data_type: str
    fingerprint: str
    node_count: int


@dataclass(frozen=True)
class InlineCandidate:
    file: str
    line: int
    tag: str
    props: tuple[tuple[str, str], ...]


def _local_name(tag: str) -> str:
    if "}" in tag:
        return tag.rsplit("}", 1)[1]
    if ":" in tag:
        return tag.rsplit(":", 1)[1]
    return tag


def _norm_target_type(raw: str) -> str:
    cleaned = raw.strip()
    m = XTYPE_RE.search(cleaned)
    if m:
        cleaned = m.group(1)
    if ":" in cleaned:
        cleaned = cleaned.rsplit(":", 1)[1]
    return cleaned


def _norm_resource_ref(raw: str) -> str:
    cleaned = raw.strip()
    m = STATIC_RES_RE.search(cleaned) or DYNAMIC_RES_RE.search(cleaned)
    return m.group(1) if m else cleaned


def _short_hash(payload: str) -> str:
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()[:12]


def _canonical_xml(
    elem: ET.Element,
    ignore_keys: bool = True,
    normalize_bindings: bool = False,
) -> str:
    tag = _local_name(elem.tag)
    attrs: list[str] = []
    for k, v in sorted(elem.attrib.items()):
        lk = _local_name(k)
        if ignore_keys and k in (X_KEY_ATTR, X_NAME_ATTR, "Name"):
            continue
        if lk == "Ignorable" or k.startswith("{http://schemas.openxmlformats.org/markup-compatibility/"):
            continue
        val = v.strip()
        if normalize_bindings:
            if val.startswith("{Binding"):
                val = "{Binding}"
            elif val.startswith("{x:Bind"):
                val = "{x:Bind}"
            elif val.startswith("{TemplateBinding"):
                val = "{TemplateBinding}"
        attrs.append(f"{lk}={val}")
    children = [
        _canonical_xml(c, ignore_keys=ignore_keys, normalize_bindings=normalize_bindings)
        for c in elem
    ]
    text = (elem.text or "").strip()
    body = "|".join(p for p in [text, *children] if p)
    return f"<{tag} {' '.join(attrs)}>{body}</{tag}>"


def _attach_line_numbers(xml_text: str, root: ET.Element) -> dict[int, int]:
    """Map element id() to approximate 1-based start line via tag stream scan."""
    line_map: dict[int, int] = {}
    tag_positions: list[tuple[str, int]] = []
    for m in re.finditer(r"<([A-Za-z_][A-Za-z0-9_.:-]*)\b", xml_text):
        tag_name = m.group(1).rsplit(":", 1)[-1]
        ln = xml_text.count("\n", 0, m.start()) + 1
        tag_positions.append((tag_name, ln))

    idx = 0
    total = len(tag_positions)

    def walk(el: ET.Element) -> None:
        nonlocal idx
        local = _local_name(el.tag)
        while idx < total and tag_positions[idx][0] != local:
            idx += 1
        if idx < total:
            line_map[id(el)] = tag_positions[idx][1]
            idx += 1
        else:
            line_map[id(el)] = 1
        for child in el:
            walk(child)

    walk(root)
    return line_map


def _walk_xaml_files(roots: list[Path]) -> list[Path]:
    matched: list[Path] = []
    seen: set[Path] = set()
    for base in roots:
        if not base.exists():
            continue
        for cur_root, dirs, files in os.walk(base):
            dirs[:] = [d for d in dirs if d.lower() not in EXCLUDED_DIRS and "_wpftmp" not in d.lower()]
            for fname in files:
                low = fname.lower()
                if "_wpftmp" in low or not low.endswith(".xaml"):
                    continue
                full = (Path(cur_root) / fname).resolve()
                if full not in seen:
                    seen.add(full)
                    matched.append(full)
    return sorted(matched)


def _extract_setter_pair(setter_el: ET.Element) -> tuple[str, str] | None:
    prop = (setter_el.attrib.get("Property") or "").strip()
    if not prop:
        return None
    if "Value" in setter_el.attrib:
        return prop, setter_el.attrib["Value"].strip()
    for child in setter_el:
        if _local_name(child.tag) == "Setter.Value":
            inner = "".join(_canonical_xml(sub, ignore_keys=False) for sub in child)
            if not inner and child.text:
                inner = child.text.strip()
            return prop, inner
    return prop, ""


def _parse_style(el: ET.Element, rel: str, line_map: dict[int, int]) -> StyleRecord:
    key = (el.attrib.get(X_KEY_ATTR) or "").strip() or "(implicit)"
    target_type = _norm_target_type(el.attrib.get("TargetType") or "")
    based_on = _norm_resource_ref(el.attrib.get("BasedOn") or "")
    setters: list[tuple[str, str]] = []
    triggers: list[str] = []

    for child in el:
        cname = _local_name(child.tag)
        if cname == "Setter":
            pair = _extract_setter_pair(child)
            if pair is not None:
                setters.append(pair)
        elif cname == "Style.Triggers":
            for trg in child:
                triggers.append(_canonical_xml(trg, ignore_keys=True))

    sorted_setters = tuple(sorted(setters, key=lambda kv: (kv[0], kv[1])))
    sorted_triggers = tuple(sorted(triggers))
    fp_payload = json.dumps(
        {
            "targetType": target_type,
            "basedOn": based_on,
            "setters": sorted_setters,
            "triggers": sorted_triggers,
        },
        ensure_ascii=False,
    )
    return StyleRecord(
        file=rel,
        line=line_map.get(id(el), 1),
        key=key,
        target_type=target_type,
        based_on=based_on,
        setters=sorted_setters,
        triggers=sorted_triggers,
        fingerprint=_short_hash(fp_payload),
    )


def _analyze_xaml_file(
    full: Path,
    rel: str,
    styles: list[StyleRecord],
    templates: list[TemplateRecord],
    inline_candidates: list[InlineCandidate],
    defined_keys: dict[str, list[tuple[str, int]]],
    referenced_keys: set[str],
    findings: list[dict],
) -> None:
    raw = full.read_text(encoding="utf-8-sig", errors="replace")
    for m in STATIC_RES_RE.finditer(raw):
        referenced_keys.add(m.group(1).strip())
    for m in DYNAMIC_RES_RE.finditer(raw):
        referenced_keys.add(m.group(1).strip())

    try:
        root = ET.fromstring(raw)
    except ET.ParseError as exc:
        findings.append(
            {
                "id": "XMLParseError",
                "severity": "fail",
                "file": rel,
                "line": 1,
                "message": f"Malformed XAML XML: {exc}",
            }
        )
        return

    line_map = _attach_line_numbers(raw, root)

    def visit(el: ET.Element, in_resources: bool) -> None:
        local = _local_name(el.tag)
        ln = line_map.get(id(el), 1)
        is_res_container = local == "ResourceDictionary" or local.endswith(".Resources")

        if is_res_container:
            local_keys: dict[str, int] = {}
            implicit_targets: dict[str, int] = {}
            for child in el:
                cln = line_map.get(id(child), ln)
                ck = (child.attrib.get(X_KEY_ATTR) or "").strip()
                if ck:
                    defined_keys.setdefault(ck, []).append((rel, cln))
                    if ck in local_keys:
                        findings.append(
                            {
                                "id": "DuplicateResourceKey",
                                "severity": "fail",
                                "file": rel,
                                "line": cln,
                                "message": f"Duplicate x:Key '{ck}' in same ResourceDictionary (first at L{local_keys[ck]})",
                            }
                        )
                    else:
                        local_keys[ck] = cln
                elif _local_name(child.tag) == "Style":
                    tt = _norm_target_type(child.attrib.get("TargetType") or "")
                    if tt:
                        if tt in implicit_targets:
                            findings.append(
                                {
                                    "id": "ImplicitStyleConflict",
                                    "severity": "fail",
                                    "file": rel,
                                    "line": cln,
                                    "message": f"Multiple implicit Styles for TargetType '{tt}' in same dictionary (first at L{implicit_targets[tt]})",
                                }
                            )
                        else:
                            implicit_targets[tt] = cln

        if in_resources and local in FREEZABLE_TAGS and PO_FREEZE_ATTR not in el.attrib:
            findings.append(
                {
                    "id": "MissingFreeze",
                    "severity": "warn",
                    "file": rel,
                    "line": ln,
                    "message": f"Resource '{local}' lacks po:Freeze=\"True\" (FP immutability & render perf)",
                }
            )

        for attr_name, attr_val in el.attrib.items():
            if HEX_COLOR_RE.match(attr_val.strip()):
                findings.append(
                    {
                        "id": "HardcodedColor",
                        "severity": "warn",
                        "file": rel,
                        "line": ln,
                        "message": f"Hardcoded color literal '{attr_val}' on {_local_name(attr_name)}; extract to theme ResourceDictionary token",
                    }
                )

        if local == "Style":
            styles.append(_parse_style(el, rel, line_map))
        elif local in ("ControlTemplate", "DataTemplate"):
            tt = _norm_target_type(el.attrib.get("TargetType") or el.attrib.get("DataType") or "")
            key = (el.attrib.get(X_KEY_ATTR) or "").strip() or "(implicit)"
            canon = _canonical_xml(el, ignore_keys=True, normalize_bindings=True)
            node_cnt = sum(1 for _ in el.iter())
            if node_cnt >= 3:
                templates.append(
                    TemplateRecord(
                        file=rel,
                        line=ln,
                        kind=local,
                        key=key,
                        target_or_data_type=tt,
                        fingerprint=_short_hash(canon),
                        node_count=node_cnt,
                    )
                )
        elif local in INLINE_STYLE_CONTROLS and "Style" not in el.attrib:
            vis = tuple(
                sorted(
                    (k, v.strip())
                    for k, v in el.attrib.items()
                    if k in VISUAL_ATTRS and v.strip()
                )
            )
            if len(vis) >= 3:
                inline_candidates.append(InlineCandidate(file=rel, line=ln, tag=local, props=vis))

        next_in_res = in_resources or is_res_container
        for child in el:
            visit(child, next_in_res)

    visit(root, False)


def _find_style_duplicates(
    styles: list[StyleRecord],
    min_shared_setters: int,
    sim_threshold: float,
) -> tuple[list[dict], list[dict]]:
    by_fp: dict[str, list[StyleRecord]] = collections.defaultdict(list)
    for st in styles:
        if len(st.setters) >= 2 or st.triggers:
            by_fp[st.fingerprint].append(st)

    exact_clones: list[dict] = []
    for fp, group in by_fp.items():
        if len(group) >= 2:
            exact_clones.append(
                {
                    "fingerprint": fp,
                    "targetType": group[0].target_type or "(untyped)",
                    "setterCount": len(group[0].setters),
                    "occurrences": [asdict(g) for g in group],
                }
            )

    by_target: dict[str, list[StyleRecord]] = collections.defaultdict(list)
    for st in styles:
        if st.target_type and len(st.setters) >= min_shared_setters:
            by_target[st.target_type].append(st)

    based_on_candidates: list[dict] = []
    seen_pairs: set[tuple[str, int, str, int]] = set()
    for target, items in by_target.items():
        for i in range(len(items)):
            for j in range(i + 1, len(items)):
                a, b = items[i], items[j]
                if a.fingerprint == b.fingerprint:
                    continue
                if a.based_on and a.based_on == b.based_on:
                    continue
                pair_key = (a.file, a.line, b.file, b.line)
                if pair_key in seen_pairs:
                    continue
                set_a, set_b = set(a.setters), set(b.setters)
                shared = sorted(set_a & set_b)
                if len(shared) < min_shared_setters:
                    continue
                union_len = len(set_a | set_b)
                jaccard = round(len(shared) / union_len, 3) if union_len else 0.0
                if jaccard >= sim_threshold:
                    seen_pairs.add(pair_key)
                    based_on_candidates.append(
                        {
                            "targetType": target,
                            "jaccardSimilarity": jaccard,
                            "sharedSetters": [{"property": p, "value": v} for p, v in shared],
                            "first": {"file": a.file, "line": a.line, "key": a.key, "basedOn": a.based_on},
                            "second": {"file": b.file, "line": b.line, "key": b.key, "basedOn": b.based_on},
                            "recommendation": f"Extract {len(shared)} shared Setters into Base{target}Style and derive both via BasedOn.",
                        }
                    )

    based_on_candidates.sort(key=lambda x: x["jaccardSimilarity"], reverse=True)
    return exact_clones, based_on_candidates


def _find_template_clones(templates: list[TemplateRecord]) -> list[dict]:
    by_fp: dict[str, list[TemplateRecord]] = collections.defaultdict(list)
    for t in templates:
        by_fp[t.fingerprint].append(t)
    clones = []
    for fp, group in by_fp.items():
        if len(group) >= 2:
            clones.append(
                {
                    "fingerprint": fp,
                    "kind": group[0].kind,
                    "targetOrDataType": group[0].target_or_data_type or "(untyped)",
                    "nodeCount": group[0].node_count,
                    "occurrences": [asdict(g) for g in group],
                }
            )
    clones.sort(key=lambda x: (len(x["occurrences"]), x["nodeCount"]), reverse=True)
    return clones


def _find_inline_clusters(candidates: list[InlineCandidate]) -> list[dict]:
    buckets: dict[tuple[str, tuple[tuple[str, str], ...]], list[InlineCandidate]] = collections.defaultdict(list)
    for c in candidates:
        buckets[(c.tag, c.props)].append(c)
    clusters: list[dict] = []
    for (tag, props), hits in buckets.items():
        if len(hits) >= 3:
            clusters.append(
                {
                    "control": tag,
                    "occurrencesCount": len(hits),
                    "sharedAttributes": [{"property": p, "value": v} for p, v in props],
                    "locations": [{"file": h.file, "line": h.line} for h in hits],
                    "recommendation": f"Promote repeated {len(props)} attributes on <{tag}> ({len(hits)}x) into a shared Style.",
                }
            )
    clusters.sort(key=lambda x: x["occurrencesCount"], reverse=True)
    return clusters


def scan_xaml_repo(
    repo_root: Path,
    external_roots: list[Path] | None = None,
    min_shared_setters: int = 3,
    sim_threshold: float = 0.70,
) -> dict:
    primary_files = _walk_xaml_files([repo_root])
    ext_files = _walk_xaml_files(external_roots or [])

    styles: list[StyleRecord] = []
    templates: list[TemplateRecord] = []
    inline_candidates: list[InlineCandidate] = []
    defined_keys: dict[str, list[tuple[str, int]]] = {}
    referenced_keys: set[str] = set()
    findings: list[dict] = []

    for full in primary_files:
        rel = full.relative_to(repo_root).as_posix()
        _analyze_xaml_file(
            full,
            rel,
            styles,
            templates,
            inline_candidates,
            defined_keys,
            referenced_keys,
            findings,
        )

    external_keys: set[str] = set()
    for ext_full in ext_files:
        try:
            ext_raw = ext_full.read_text(encoding="utf-8-sig", errors="replace")
            ext_root = ET.fromstring(ext_raw)
            for el in ext_root.iter():
                k = (el.attrib.get(X_KEY_ATTR) or "").strip()
                if k:
                    external_keys.add(k)
        except (OSError, ET.ParseError):
            continue

    for k, locs in sorted(defined_keys.items()):
        if k in referenced_keys:
            continue
        for rel, ln in locs:
            low = rel.lower()
            if "/themes/" in f"/{low}" or low.endswith("app.xaml"):
                continue
            findings.append(
                {
                    "id": "DeadResource",
                    "severity": "warn",
                    "file": rel,
                    "line": ln,
                    "message": f"Local x:Key '{k}' is never referenced by StaticResource/DynamicResource",
                }
            )

    exact_styles, based_on = _find_style_duplicates(styles, min_shared_setters, sim_threshold)
    template_clones = _find_template_clones(templates)
    inline_clusters = _find_inline_clusters(inline_candidates)

    fail_count = sum(1 for f in findings if f["severity"] == "fail") + len(exact_styles) + len(template_clones)
    warn_count = sum(1 for f in findings if f["severity"] == "warn") + len(based_on) + len(inline_clusters)

    return {
        "repoRoot": str(repo_root),
        "xamlFilesScanned": len(primary_files),
        "stylesFound": len(styles),
        "templatesFound": len(templates),
        "externalKeysLoaded": len(external_keys),
        "exactStyleClones": exact_styles,
        "basedOnCandidates": based_on,
        "templateClones": template_clones,
        "inlinePropertyClusters": inline_clusters,
        "findings": findings,
        "failCount": fail_count,
        "warnCount": warn_count,
        "passed": fail_count == 0,
    }


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    p = argparse.ArgumentParser(description="Semantic WPF/XAML Style, Template & ResourceDictionary X-Ray")
    p.add_argument("--repo-root", "-r", default=".")
    p.add_argument("--external-xaml-dir", nargs="*", default=[], help="Shared XAML dirs (e.g., Shared.UI/Themes)")
    p.add_argument("--min-shared-setters", type=int, default=3)
    p.add_argument("--similarity-threshold", type=float, default=0.70)
    p.add_argument("--top", type=int, default=15)
    p.add_argument("--strict", action="store_true", help="Fail on warnings, BasedOn candidates & inline clusters too")
    p.add_argument("--json", action="store_true")
    args = p.parse_args()

    repo_root = Path(args.repo_root).resolve()
    ext_dirs = [Path(d).resolve() for d in args.external_xaml_dir if d]
    report = scan_xaml_repo(repo_root, ext_dirs, args.min_shared_setters, args.similarity_threshold)
    passed = (report["failCount"] == 0 and report["warnCount"] == 0) if args.strict else report["passed"]
    report["passed"] = passed

    if args.json:
        print(json.dumps(report, indent=2, ensure_ascii=False))
        return 0 if passed else 1

    print("# WPF / XAML Semantic Style & Template X-Ray")
    print(f"- **Repo**: {repo_root} | **XAML Files**: {report['xamlFilesScanned']} | **Styles**: {report['stylesFound']} | **Templates**: {report['templatesFound']}")
    print(f"- **Summary**: {report['failCount']} fail, {report['warnCount']} warn")

    for label, key in (
        ("Exact Style Clones (Setter Order-Independent)", "exactStyleClones"),
        ("Partial Style Clones (BasedOn Extraction Candidates)", "basedOnCandidates"),
        ("ControlTemplate / DataTemplate Clones", "templateClones"),
        ("Inline Repeated Property Clusters (Style Promotion)", "inlinePropertyClusters"),
    ):
        items = report[key]
        if items:
            print(f"\n## {label} ({len(items)})")
            for item in items[: args.top]:
                print(f"- {json.dumps(item, ensure_ascii=False)}")

    if report["findings"]:
        print(f"\n## ResourceDictionary & Hygiene Findings ({len(report['findings'])})")
        for f in report["findings"][: args.top]:
            print(f"- [{f['severity'].upper()}] `{f['file']}:{f['line']}` [{f['id']}] {f['message']}")

    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
