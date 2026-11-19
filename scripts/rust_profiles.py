#!/usr/bin/env python3
"""Shared Rust profile discovery, #[cfg(test)] lexical stripping, and consistency rules."""

from __future__ import annotations

import json
import os
import re
import subprocess
from pathlib import Path

EXCLUDED_DIRS = {
    "build",
    "bin",
    "obj",
    ".git",
    ".vs",
    ".agents",
    ".cursor",
    ".gemini",
    "node_modules",
    "testresults",
    "target",
    "target-final",
}

ASYNC_CRATE_MARKERS = ("tokio", "axum", "tauri", "actix-web", "hyper", "tonic")
RAW_STR_RE = re.compile(r'(?:br|cr|rb|rc|r)(#*)(")')
CFG_TEST_MARKER_RE = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
UNSAFE_BLOCK_RE = re.compile(r"\bunsafe\s*\{")
STD_MUTEX_RE = re.compile(r"\b(?:std::sync::)?(?:Mutex|RwLock)\b")
AWAIT_RE = re.compile(r"\.\s*await\b")
SAFETY_COMMENT_RE = re.compile(r"//\s*SAFETY\s*:|///\s*#\s*Safety\b", re.I)

UNCONDITIONAL_RS_RULES = [
    ("RustUnwrap", "fail", re.compile(r"\.unwrap\s*\("), ("unwrap_used",), "Avoid .unwrap() in production Rust; propagate via ? or document invariant"),
    ("RustTodo", "fail", re.compile(r"\b(?:todo|unimplemented)!\s*\("), ("todo", "unimplemented"), "Banned todo!/unimplemented! in production Rust"),
    ("RustDbg", "fail", re.compile(r"\bdbg!\s*\("), ("dbg_macro",), "Banned dbg!() in production Rust"),
]

FFI_ONLY_RS_RULES = [
    ("RustExpect", "fail", re.compile(r"\.expect\s*\("), ("expect_used",), "Banned .expect() in FFI crate (cdylib/staticlib panic=abort hazard)"),
    ("RustPanic", "fail", re.compile(r"\bpanic!\s*\("), ("panic",), "Banned panic!() in FFI crate (cdylib/staticlib panic=abort hazard)"),
    ("RustFfiPrint", "fail", re.compile(r"\b(?:println|eprintln)!\s*\("), ("print_stdout", "print_stderr"), "Banned println!/eprintln! in FFI native library"),
]

NON_FFI_SOFT_RULES = [
    ("RustExpect", "warn", re.compile(r"\.expect\s*\("), ("expect_used",), "Prefer Result/? or justify invariant with #[expect(clippy::expect_used, reason = \"...\")]"),
    ("RustPanic", "warn", re.compile(r"\bpanic!\s*\("), ("panic",), "Prefer Result/? or justify panic with #[expect(clippy::panic, reason = \"...\")]"),
]


def _scan_rust_literal_or_comment(text: str, i: int) -> int | None:
    """If text[i:] starts a Rust comment, string, raw string, or char literal, return end index."""
    n = len(text)
    if text.startswith("//", i):
        nl = text.find("\n", i + 2)
        return n if nl < 0 else nl
    if text.startswith("/*", i):
        depth, j = 1, i + 2
        while j < n and depth > 0:
            if text.startswith("/*", j):
                depth += 1
                j += 2
            elif text.startswith("*/", j):
                depth -= 1
                j += 2
            else:
                j += 1
        return j
    raw_prefix = RAW_STR_RE.match(text, i)
    if raw_prefix:
        end_delim = '"' + raw_prefix.group(1)
        j = raw_prefix.end()
        end = text.find(end_delim, j)
        return n if end < 0 else end + len(end_delim)
    if text[i] == '"' or (text.startswith('b"', i) or text.startswith('c"', i)):
        j = i + (2 if text[i] in ("b", "c") else 1)
        while j < n:
            if text[j] == "\\":
                j += 2
            elif text[j] == '"':
                return j + 1
            else:
                j += 1
        return n
    if text.startswith("b'", i) or text[i] == "'":
        start = i + (2 if text[i] == "b" else 1)
        if start < n and text[start] == "\\":
            end = text.find("'", start + 2)
            return end + 1 if 0 <= end <= start + 8 else None
        if start + 1 < n and text[start + 1] == "'":
            return start + 2
    return None


def _blank_span(chars: list[str], start: int, end: int) -> None:
    for k in range(start, min(end, len(chars))):
        if chars[k] != "\n":
            chars[k] = " "


def strip_rust_cfg_test_blocks(text: str) -> str:
    """Replace #[cfg(test)] items with whitespace while preserving newlines and surrounding prod code."""
    chars = list(text)
    n = len(text)
    i = 0
    while i < n:
        lit_end = _scan_rust_literal_or_comment(text, i)
        if lit_end is not None:
            i = lit_end
            continue
        if text[i] == "#":
            m = CFG_TEST_MARKER_RE.match(text, i)
            if m:
                j = m.end()
                depth = 0
                saw_brace = False
                while j < n:
                    sub_end = _scan_rust_literal_or_comment(text, j)
                    if sub_end is not None:
                        j = sub_end
                        continue
                    ch = text[j]
                    if ch == "{":
                        depth += 1
                        saw_brace = True
                    elif ch == "}":
                        depth -= 1
                        if saw_brace and depth == 0:
                            j += 1
                            break
                    elif ch == ";" and not saw_brace:
                        j += 1
                        break
                    j += 1
                _blank_span(chars, i, j)
                i = j
                continue
        i += 1
    return "".join(chars)


def blank_rust_comments_and_strings(text: str) -> str:
    """Blank comments and string/char literals while preserving newlines and line offsets."""
    chars = list(text)
    n = len(text)
    i = 0
    while i < n:
        lit_end = _scan_rust_literal_or_comment(text, i)
        if lit_end is not None:
            _blank_span(chars, i, lit_end)
            i = lit_end
            continue
        i += 1
    return "".join(chars)


def is_rust_test_or_build(rel_posix: str) -> bool:
    low = rel_posix.lower()
    name = low.rsplit("/", 1)[-1]
    return (
        name in ("build.rs", "tests.rs")
        or name.endswith("_tests.rs")
        or "/tests/" in f"/{low}"
        or "/benches/" in f"/{low}"
    )


def infer_profile_from_text(cargo_text: str) -> str:
    low = cargo_text.lower()
    if "cdylib" in low or "staticlib" in low:
        return "ffi"
    if any(re.search(rf"(?:^|\b){re.escape(k)}\b", low) for k in ASYNC_CRATE_MARKERS):
        return "server-async"
    return "cli-lib"


def infer_profile_for_file(rs_file: Path) -> str:
    cur = rs_file.parent
    profiles: list[str] = []
    for _ in range(7):
        cargo = cur / "Cargo.toml"
        if cargo.is_file():
            try:
                txt = cargo.read_text(encoding="utf-8", errors="replace")
            except OSError:
                txt = ""
            prof = infer_profile_from_text(txt)
            profiles.append(prof)
            if "[workspace]" in txt:
                break
        if cur.parent == cur:
            break
        cur = cur.parent
    if "ffi" in profiles:
        return "ffi"
    if "server-async" in profiles:
        return "server-async"
    return "cli-lib"


def _has_lint_suppression(lines: list[str], line_idx: int, lint_names: tuple[str, ...]) -> bool:
    start = line_idx
    steps = 0
    while start > 0 and steps < 60:
        prev = lines[start - 1].strip()
        if prev.endswith(";") or prev.endswith("{") and not prev.startswith("#["):
            break
        start -= 1
        steps += 1
    window = "\n".join(lines[max(0, start - 3) : line_idx + 1])
    for name in lint_names:
        if re.search(rf"#\s*\[(?:allow|expect)\s*\([^]]*\b(?:clippy::)?{re.escape(name)}\b", window):
            return True
    return False


def collect_rust_consistency_findings(
    full_path: Path,
    rel_posix: str,
    text: str,
    line_warn: int = 300,
    line_max: int = 450,
) -> list[dict]:
    """Run Rust-idiomatic no-build consistency checks on a single .rs file."""
    findings: list[dict] = []
    is_test_file = is_rust_test_or_build(rel_posix)
    if is_test_file:
        return findings

    prod_only_text = strip_rust_cfg_test_blocks(text)
    prod_non_empty = sum(1 for ln in prod_only_text.splitlines() if ln.strip())

    if prod_non_empty > line_max:
        findings.append({"file": rel_posix, "line": len(text.splitlines()), "severity": "fail", "id": "RustProdLineMax", "message": f"Production code has {prod_non_empty} non-empty lines > {line_max} (excluding #[cfg(test)])"})
    elif prod_non_empty >= line_warn:
        findings.append({"file": rel_posix, "line": len(text.splitlines()), "severity": "warn", "id": "RustProdLineWarn", "message": f"Production code has {prod_non_empty} non-empty lines (warn >= {line_warn}, excluding #[cfg(test)])"})

    raw_lines = text.splitlines()
    sanitized = blank_rust_comments_and_strings(prod_only_text)
    profile = infer_profile_for_file(full_path)
    rules = list(UNCONDITIONAL_RS_RULES) + (FFI_ONLY_RS_RULES if profile == "ffi" else NON_FFI_SOFT_RULES)

    for rule_id, sev, regex, lint_names, msg in rules:
        for m in regex.finditer(sanitized):
            ln = sanitized.count("\n", 0, m.start()) + 1
            if _has_lint_suppression(raw_lines, ln - 1, lint_names):
                continue
            findings.append({"file": rel_posix, "line": ln, "severity": sev, "id": rule_id, "message": msg})

    for m in UNSAFE_BLOCK_RE.finditer(sanitized):
        ln = sanitized.count("\n", 0, m.start()) + 1
        idx = ln - 1
        if _has_lint_suppression(raw_lines, idx, ("undocumented_unsafe_blocks",)):
            continue
        snippet = "\n".join(raw_lines[max(0, idx - 3) : idx + 1])
        if not SAFETY_COMMENT_RE.search(snippet):
            findings.append({"file": rel_posix, "line": ln, "severity": "fail", "id": "RustUnsafeSafetyDoc", "message": "unsafe block missing adjacent `// SAFETY:` justification"})

    if profile == "server-async" and AWAIT_RE.search(sanitized):
        for m in STD_MUTEX_RE.finditer(sanitized):
            ln = sanitized.count("\n", 0, m.start()) + 1
            if "tokio::sync::" in raw_lines[ln - 1]:
                continue
            findings.append({"file": rel_posix, "line": ln, "severity": "warn", "id": "RustAsyncStdMutex", "message": "std::sync::Mutex/RwLock used in async file; ensure guard never crosses .await"})
            break

    return findings


def discover_rust_targets(repo_root: Path, target_filter: str = "") -> list[dict]:
    """Discover Cargo workspaces and standalone crates once per workspace via `cargo metadata`."""
    raw_manifests: list[Path] = []
    for cur_root, dirs, files in os.walk(repo_root):
        dirs[:] = [d for d in dirs if d.lower() not in EXCLUDED_DIRS]
        if "Cargo.toml" in files:
            raw_manifests.append((Path(cur_root) / "Cargo.toml").resolve())
    raw_manifests.sort(key=lambda p: (len(p.parts), str(p).lower()))

    claimed_manifests: set[Path] = set()
    targets: list[dict] = []
    for manifest in raw_manifests:
        if manifest in claimed_manifests:
            continue
        meta_proc = subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1", "--manifest-path", str(manifest)],
            cwd=manifest.parent,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
        if meta_proc.returncode != 0:
            claimed_manifests.add(manifest)
            prof = infer_profile_from_text(manifest.read_text(encoding="utf-8", errors="replace"))
            targets.append({"name": manifest.parent.name, "manifest": manifest, "workspaceRoot": manifest.parent, "isWorkspace": False, "profile": prof, "packages": [manifest.parent.name], "memberDirs": [manifest.parent]})
            continue

        meta = json.loads(meta_proc.stdout)
        ws_root = Path(meta.get("workspace_root", str(manifest.parent))).resolve()
        ws_members = set(meta.get("workspace_members") or [])
        pkgs = [p for p in (meta.get("packages") or []) if p.get("id") in ws_members]
        if not pkgs:
            pkgs = meta.get("packages") or []

        member_manifests: list[Path] = []
        pkg_profiles: list[str] = []
        pkg_names: list[str] = []
        for p in pkgs:
            mp = Path(p["manifest_path"]).resolve()
            claimed_manifests.add(mp)
            member_manifests.append(mp)
            pkg_names.append(str(p.get("name", mp.parent.name)))
            crate_types = {ct for t in (p.get("targets") or []) for ct in (t.get("crate_types") or [])}
            deps = {str(d.get("name", "")).lower() for d in (p.get("dependencies") or [])}
            if "cdylib" in crate_types or "staticlib" in crate_types:
                pkg_profiles.append("ffi")
            elif deps & set(ASYNC_CRATE_MARKERS):
                pkg_profiles.append("server-async")
            else:
                pkg_profiles.append("cli-lib")

        root_manifest = (ws_root / "Cargo.toml").resolve()
        claimed_manifests.add(root_manifest)
        ws_profile = "ffi" if "ffi" in pkg_profiles else ("server-async" if "server-async" in pkg_profiles else "cli-lib")
        is_ws = len(pkgs) > 1 or (root_manifest.is_file() and "[workspace]" in root_manifest.read_text(encoding="utf-8", errors="replace"))
        label = ws_root.name if is_ws else (pkg_names[0] if pkg_names else manifest.parent.name)

        if target_filter:
            tf = target_filter.lower()
            if tf not in label.lower() and not any(tf in n.lower() for n in pkg_names) and tf not in root_manifest.as_posix().lower():
                continue

        targets.append({
            "name": label,
            "manifest": root_manifest if root_manifest.is_file() else manifest,
            "workspaceRoot": ws_root,
            "isWorkspace": is_ws,
            "profile": ws_profile,
            "packages": pkg_names,
            "memberDirs": [mp.parent for mp in member_manifests],
        })
    return targets
