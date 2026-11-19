#!/usr/bin/env python3
"""Unit tests for Rust governance profiles, #[cfg(test)] lexical stripping, and Stop guard."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

from init_rust_governance import bootstrap_target  # noqa: E402
from rust_profiles import (  # noqa: E402
    collect_rust_consistency_findings,
    discover_rust_targets,
    strip_rust_cfg_test_blocks,
)
from rust_xray import classify_rust_rule  # noqa: E402


class RustGovernanceTests(unittest.TestCase):
    def test_strip_cfg_test_preserves_trailing_prod_code_and_strings(self) -> None:
        src = (
            'pub fn before() -> &\'static str { "#[cfg(test)] mod fake { }" }\n'
            '#[cfg(test)]\n'
            'mod tests {\n'
            '    #[test]\n'
            '    fn t() { let s = "}\\""; let _ = Some(1).unwrap(); }\n'
            '}\n'
            'pub fn after() { let _ = Some(2).unwrap(); }\n'
        )
        stripped = strip_rust_cfg_test_blocks(src)
        self.assertIn("before", stripped)
        self.assertIn("after", stripped)
        self.assertNotIn("Some(1).unwrap()", stripped)
        self.assertIn("Some(2).unwrap()", stripped)
        self.assertEqual(len(src.splitlines()), len(stripped.splitlines()))

    def test_ffi_vs_server_expect_and_expect_attribute(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            ffi_dir = root / "ffi_crate" / "src"
            srv_dir = root / "srv_crate" / "src"
            ffi_dir.mkdir(parents=True)
            srv_dir.mkdir(parents=True)
            (root / "ffi_crate" / "Cargo.toml").write_text('[package]\nname="k"\nversion="0.1.0"\n[lib]\ncrate-type=["cdylib"]\n', encoding="utf-8")
            (root / "srv_crate" / "Cargo.toml").write_text('[package]\nname="s"\nversion="0.1.0"\n[dependencies]\ntokio="1"\n', encoding="utf-8")

            code = (
                'pub fn boot() {\n'
                '    let _a = Some(1).expect("invariant");\n'
                '}\n'
                '#[expect(clippy::expect_used, reason = "compile-time constant")]\n'
                'pub fn safe_boot() {\n'
                '    let _b = Some(2).expect("const");\n'
                '}\n'
            )
            ffi_hits = collect_rust_consistency_findings(ffi_dir / "lib.rs", "ffi_crate/src/lib.rs", code)
            self.assertTrue(any(h["id"] == "RustExpect" and h["severity"] == "fail" and h["line"] == 2 for h in ffi_hits))
            self.assertFalse(any(h["line"] == 6 for h in ffi_hits))

            srv_hits = collect_rust_consistency_findings(srv_dir / "lib.rs", "srv_crate/src/lib.rs", code)
            self.assertTrue(any(h["id"] == "RustExpect" and h["severity"] == "warn" and h["line"] == 2 for h in srv_hits))
            self.assertFalse(any(h["severity"] == "fail" for h in srv_hits))
            self.assertFalse(any(h["line"] == 6 for h in srv_hits))

    def test_unsafe_safety_comment_and_async_mutex_and_inline_tests(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            src_dir = root / "src"
            src_dir.mkdir()
            (root / "Cargo.toml").write_text('[package]\nname="srv"\nversion="0.1.0"\n[dependencies]\naxum="0.8"\ntokio="1"\n', encoding="utf-8")

            # Over 300 total lines if inline test module counts, but only ~15 prod lines
            inline_tests = "\n".join(f"    #[test] fn t{i}() {{ let _ = Some({i}).unwrap(); }}" for i in range(320))
            code = (
                "use std::sync::Mutex;\n"
                "pub async fn f(m: &Mutex<i32>) -> i32 {\n"
                "    tokio::task::yield_now().await;\n"
                "    let x = unsafe { std::mem::zeroed::<i32>() };\n"
                "    // SAFETY: pointer is dummy test\n"
                "    let y = unsafe { std::ptr::read(&x) };\n"
                "    *m.lock().unwrap_or_else(|e| e.into_inner()) + y\n"
                "}\n"
                "#[cfg(test)]\nmod tests {\n" + inline_tests + "\n}\n"
            )
            hits = collect_rust_consistency_findings(src_dir / "lib.rs", "src/lib.rs", code)
            ids = {(h["id"], h["severity"], h["line"]) for h in hits}
            self.assertIn(("RustUnsafeSafetyDoc", "fail", 4), ids)
            self.assertNotIn(("RustUnsafeSafetyDoc", "fail", 6), ids)
            self.assertTrue(any(h["id"] == "RustAsyncStdMutex" and h["severity"] == "warn" for h in hits))
            self.assertFalse(any(h["id"].startswith("RustProdLine") for h in hits))

    def test_classify_rules_covers_supply_chain_async_and_casts(self) -> None:
        self.assertEqual(classify_rust_rule("clippy::await_holding_lock"), "Concurrency/Async")
        self.assertEqual(classify_rust_rule("RUSTSEC-2026-0001"), "SupplyChain/Security")
        self.assertEqual(classify_rust_rule("clippy::cast_possible_truncation"), "Error/CastSafety")
        self.assertEqual(classify_rust_rule("clippy::undocumented_unsafe_blocks"), "Unsafe/FFI")

    def test_workspace_discovery_dedups_members_and_bootstrap_creates_configs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Cargo.toml").write_text('[workspace]\nmembers=["crates/core","srv"]\nresolver="2"\n', encoding="utf-8")
            (root / "crates" / "core" / "src").mkdir(parents=True)
            (root / "crates" / "core" / "Cargo.toml").write_text('[package]\nname="c"\nversion="0.1.0"\nedition="2024"\n', encoding="utf-8")
            (root / "crates" / "core" / "src" / "lib.rs").write_text("pub fn ok() -> i32 { 1 }\n", encoding="utf-8")
            (root / "srv" / "src").mkdir(parents=True)
            (root / "srv" / "Cargo.toml").write_text('[package]\nname="s"\nversion="0.1.0"\nedition="2024"\n[dependencies]\ntokio="1"\n', encoding="utf-8")
            (root / "srv" / "src" / "main.rs").write_text("fn main() {}\n", encoding="utf-8")

            targets = discover_rust_targets(root)
            self.assertEqual(len(targets), 1)
            self.assertTrue(targets[0]["isWorkspace"])
            self.assertEqual(sorted(targets[0]["packages"]), ["c", "s"])

            res = bootstrap_target(targets[0], "auto", apply_cargo=True, force=False)
            self.assertEqual(set(res["created"]), {"deny.toml", "rustfmt.toml", "clippy.toml"})
            self.assertIn("[workspace.lints.clippy]", (root / "Cargo.toml").read_text(encoding="utf-8"))
            self.assertIn("[lints]\nworkspace = true", (root / "srv" / "Cargo.toml").read_text(encoding="utf-8"))

    def test_stop_guard_resolves_cwd_and_continuation_active(self) -> None:
        stop_guard = SCRIPTS_DIR / "hooks" / "stop_guard.py"
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            subprocess.run(["git", "init"], cwd=root, check=True, capture_output=True)
            (root / "Cargo.toml").write_text('[package]\nname="app"\nversion="0.1.0"\nedition="2024"\n', encoding="utf-8")
            (root / "crates" / "core" / "src").mkdir(parents=True)
            (root / "crates" / "core" / "src" / "lib.rs").write_text("pub fn bad() { let _ = Some(1).unwrap(); }\n", encoding="utf-8")

            payload = {
                "conversationId": f"test-conv-{os.getpid()}",
                "cwd": str(root),
                "workspacePaths": [str(root)],
                "terminationReason": "NO_TOOL_CALL",
                "continuationActive": False,
            }
            p1 = subprocess.run(
                [sys.executable, str(stop_guard), "--repo-root", "auto", "--lang-filter", "rust"],
                cwd=SCRIPTS_DIR,
                input=json.dumps(payload),
                text=True,
                capture_output=True,
                check=True,
            )
            out1 = json.loads(p1.stdout)
            self.assertEqual(out1["decision"], "continue")
            self.assertIn("RustUnwrap", out1["reason"])

            payload["continuationActive"] = True
            p2 = subprocess.run(
                [sys.executable, str(stop_guard), "--repo-root", "auto", "--lang-filter", "rust"],
                cwd=SCRIPTS_DIR,
                input=json.dumps(payload),
                text=True,
                capture_output=True,
                check=True,
            )
            self.assertEqual(json.loads(p2.stdout)["decision"], "stop")


if __name__ == "__main__":
    unittest.main()
