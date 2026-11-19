#!/usr/bin/env python3
"""Bootstrap idiomatic Rust static analysis & supply-chain configs into any Cargo crate/workspace."""

from __future__ import annotations

import argparse
import shutil
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
TEMPLATE_DIR = SCRIPT_DIR.parent / "templates" / "rust"
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from rust_profiles import discover_rust_targets  # noqa: E402

PROFILE_BLOCKS = {
    "ffi": """[lints.rust]
unsafe_op_in_unsafe_fn = "deny"
unused_must_use = "deny"
rust_2018_idioms = { level = "warn", priority = -1 }

[lints.clippy]
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
missing_safety_doc = "deny"
undocumented_unsafe_blocks = "deny"
not_unsafe_ptr_arg_deref = "deny"
ptr_as_ptr = "warn"
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
todo = "deny"
unimplemented = "deny"
dbg_macro = "deny"
print_stdout = "deny"
print_stderr = "deny"
exit = "deny"
let_underscore_must_use = "warn"
cast_possible_truncation = "allow"
cast_possible_wrap = "allow"
cast_precision_loss = "allow"
cast_sign_loss = "allow"
cast_lossless = "allow"
similar_names = "allow"
many_single_char_names = "allow"
module_name_repetitions = "allow"
must_use_candidate = "allow"
""",
    "server-async": """[lints.rust]
unsafe_op_in_unsafe_fn = "deny"
unused_must_use = "deny"

[lints.clippy]
all = { level = "warn", priority = -1 }
missing_safety_doc = "deny"
undocumented_unsafe_blocks = "deny"
not_unsafe_ptr_arg_deref = "deny"
unwrap_used = "warn"
expect_used = "warn"
panic = "deny"
todo = "deny"
unimplemented = "deny"
dbg_macro = "deny"
await_holding_lock = "deny"
await_holding_refcell_ref = "deny"
let_underscore_must_use = "warn"
large_futures = "warn"
""",
    "cli-lib": """[lints.rust]
unsafe_op_in_unsafe_fn = "deny"
unused_must_use = "deny"

[lints.clippy]
all = { level = "warn", priority = -1 }
missing_safety_doc = "deny"
undocumented_unsafe_blocks = "deny"
not_unsafe_ptr_arg_deref = "deny"
unwrap_used = "warn"
expect_used = "warn"
panic = "deny"
todo = "deny"
unimplemented = "deny"
dbg_macro = "deny"
let_underscore_must_use = "warn"
exit = "warn"
""",
}


def format_lints_block(profile: str, is_workspace: bool) -> str:
    raw = PROFILE_BLOCKS[profile]
    if is_workspace:
        return raw.replace("[lints.", "[workspace.lints.")
    return raw


def _append_section(toml_path: Path, header_marker: str, block: str) -> bool:
    txt = toml_path.read_text(encoding="utf-8")
    if header_marker in txt:
        return False
    sep = "\n" if txt.endswith("\n") else "\n\n"
    if not txt.endswith("\n\n") and txt.endswith("\n"):
        sep = "\n"
    toml_path.write_text(txt + sep + block.lstrip("\n"), encoding="utf-8")
    return True


def bootstrap_target(target: dict, profile_override: str, apply_cargo: bool, force: bool) -> dict:
    ws_root: Path = target["workspaceRoot"]
    profile = target["profile"] if profile_override == "auto" else profile_override
    created: list[str] = []
    skipped: list[str] = []
    cargo_updated: list[str] = []

    for fname in ("deny.toml", "rustfmt.toml", "clippy.toml"):
        src = TEMPLATE_DIR / fname
        dst = ws_root / fname
        if dst.exists() and not force:
            skipped.append(fname)
            continue
        shutil.copy2(src, dst)
        created.append(fname)

    if apply_cargo:
        root_manifest: Path = target["manifest"]
        is_ws = bool(target["isWorkspace"])
        marker = "[workspace.lints" if is_ws else "[lints"
        if _append_section(root_manifest, marker, format_lints_block(profile, is_ws)):
            cargo_updated.append(str(root_manifest.relative_to(ws_root)))
        if is_ws:
            for member_dir in target["memberDirs"]:
                member_manifest = (member_dir / "Cargo.toml").resolve()
                if member_manifest == root_manifest or not member_manifest.is_file():
                    continue
                if _append_section(member_manifest, "[lints", "[lints]\nworkspace = true\n"):
                    cargo_updated.append(str(member_manifest.relative_to(ws_root)))

    return {
        "target": target["name"],
        "workspaceRoot": str(ws_root),
        "profile": profile,
        "created": created,
        "skipped": skipped,
        "cargoUpdated": cargo_updated,
        "lintsSnippet": format_lints_block(profile, bool(target["isWorkspace"])),
    }


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    p = argparse.ArgumentParser(description="Initialize standard Rust governance configs (deny.toml, rustfmt.toml, clippy.toml, [workspace.lints])")
    p.add_argument("--repo-root", "-r", default=".", help="Target repository root")
    p.add_argument("--profile", choices=("auto", "ffi", "server-async", "cli-lib"), default="auto")
    p.add_argument("--apply-cargo", action="store_true", help="Append [workspace.lints] / [lints] into Cargo.toml manifests")
    p.add_argument("--force", action="store_true", help="Overwrite existing deny.toml / rustfmt.toml / clippy.toml")
    args = p.parse_args()

    repo_root = Path(args.repo_root).resolve()
    targets = discover_rust_targets(repo_root)
    if not targets:
        print(f"No Cargo.toml manifests found under {repo_root}")
        return 1

    for target in targets:
        res = bootstrap_target(target, args.profile, args.apply_cargo, args.force)
        print(f"# Target: {res['target']} ({res['workspaceRoot']}) [profile={res['profile']}]")
        print(f"  created: {', '.join(res['created']) or '(none)'} | skipped: {', '.join(res['skipped']) or '(none)'}")
        if args.apply_cargo:
            print(f"  Cargo.toml updated: {', '.join(res['cargoUpdated']) or '(already configured)'}")
        else:
            print("\nSuggested Cargo.toml lints block (re-run with --apply-cargo to inject automatically):\n")
            print(res["lintsSnippet"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
