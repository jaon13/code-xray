#!/usr/bin/env python3
"""Unit tests for Review-First grep manifest management and dry-run/apply safety."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

import publish_tools  # noqa: E402
from grep_review import (  # noqa: E402
    apply_manifest,
    filter_matches,
    list_manifests,
    mark_manifest,
)


class GrepReviewTests(unittest.TestCase):
    def _seed_workspace(self, root: Path) -> tuple[Path, Path, Path]:
        src = root / "src"
        docs = root / "docs"
        reviews = root / ".xray" / "reviews"
        src.mkdir(parents=True)
        docs.mkdir(parents=True)
        reviews.mkdir(parents=True)

        cs_file = src / "Worker.cs"
        cs_file.write_bytes(
            b'namespace Demo;\r\npublic class Worker {\r\n    string k1 = "OLD_KEY";\r\n    string k2 = "OLD_KEY";\r\n}\r\n'
        )
        doc_file = docs / "README.md"
        doc_file.write_bytes(b"# Docs\nKeep OLD_KEY here.\n")

        manifest_path = reviews / "grep_20261119_010000_abcd1234.json"
        manifest = {
            "schemaVersion": "1.0",
            "runId": "grep_20261119_010000_abcd1234",
            "createdAtUtc": "2026-11-19T01:00:00Z",
            "repoRoot": root.as_posix(),
            "pattern": "OLD_KEY",
            "isRegex": False,
            "caseInsensitive": False,
            "extensions": [],
            "includeExternal": False,
            "totalFilesScanned": 2,
            "totalMatches": 3,
            "truncated": False,
            "summary": {},
            "matches": [
                {
                    "id": "M001",
                    "file": "src/Worker.cs",
                    "absPath": cs_file.as_posix(),
                    "line": 3,
                    "column": 18,
                    "matchText": "OLD_KEY",
                    "enclosingSymbol": "Worker",
                    "projectName": "Demo",
                    "isExternal": False,
                    "category": "code",
                    "snippet": '    string k1 = "OLD_KEY";',
                    "contextBefore": [],
                    "contextAfter": [],
                    "status": "pending",
                    "disposition": "manual",
                    "note": "",
                },
                {
                    "id": "M002",
                    "file": "src/Worker.cs",
                    "absPath": cs_file.as_posix(),
                    "line": 4,
                    "column": 18,
                    "matchText": "OLD_KEY",
                    "enclosingSymbol": "Worker",
                    "projectName": "Demo",
                    "isExternal": False,
                    "category": "code",
                    "snippet": '    string k2 = "OLD_KEY";',
                    "contextBefore": [],
                    "contextAfter": [],
                    "status": "pending",
                    "disposition": "manual",
                    "note": "",
                },
                {
                    "id": "M003",
                    "file": "docs/README.md",
                    "absPath": doc_file.as_posix(),
                    "line": 2,
                    "column": 6,
                    "matchText": "OLD_KEY",
                    "enclosingSymbol": None,
                    "projectName": "docs",
                    "isExternal": False,
                    "category": "docs",
                    "snippet": "Keep OLD_KEY here.",
                    "contextBefore": [],
                    "contextAfter": [],
                    "status": "pending",
                    "disposition": "manual",
                    "note": "",
                },
            ],
        }
        manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
        return cs_file, doc_file, manifest_path

    def test_list_show_mark_dry_run_and_apply_only_modify_entries(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cs_file, doc_file, manifest_path = self._seed_workspace(root)

            listed = list_manifests(root / ".xray" / "reviews")
            self.assertEqual(len(listed), 1)
            self.assertEqual(listed[0]["totalMatches"], 3)

            mark_res = mark_manifest(
                manifest_path,
                ids=["M001"],
                mark_all=False,
                category="code",
                file_contains=None,
                disposition="modify",
            )
            self.assertEqual(mark_res["updatedIds"], ["M001"])

            skip_res = mark_manifest(
                manifest_path,
                ids=[],
                mark_all=False,
                category="docs",
                file_contains=None,
                disposition="skip",
            )
            self.assertEqual(skip_res["updatedIds"], ["M003"])

            preview = apply_manifest(
                manifest_path,
                replace_old="OLD_KEY",
                replace_new="NEW_KEY",
                dry_run=True,
                repo_root=root,
            )
            self.assertTrue(preview["dryRun"])
            self.assertEqual(preview["appliedIds"], ["M001"])
            self.assertEqual(preview["skippedCount"], 2)
            self.assertIn("-    string k1 = \"OLD_KEY\";", preview["diffs"][0])
            self.assertIn("+    string k1 = \"NEW_KEY\";", preview["diffs"][0])
            # File untouched after dry-run
            self.assertIn('string k1 = "OLD_KEY";', cs_file.read_text(encoding="utf-8"))

            applied = apply_manifest(
                manifest_path,
                replace_old="OLD_KEY",
                replace_new="NEW_KEY",
                dry_run=False,
                repo_root=root,
                run_consistency=True,
            )
            self.assertEqual(applied["appliedIds"], ["M001"])
            self.assertIn("check_consistency.py", applied["consistencyHint"])
            self.assertIsNotNone(applied["consistencyResult"])
            self.assertTrue(applied["consistencyResult"]["passed"])

            cs_raw = cs_file.read_bytes()
            self.assertIn(b"\r\n", cs_raw)
            cs_text = cs_raw.decode("utf-8")
            self.assertIn('string k1 = "NEW_KEY";', cs_text)
            self.assertIn('string k2 = "OLD_KEY";', cs_text)
            self.assertIn("Keep OLD_KEY here.", doc_file.read_text(encoding="utf-8"))

            loaded = json.loads(manifest_path.read_text(encoding="utf-8"))
            applied_items = filter_matches(loaded, status="applied")
            self.assertEqual([m["id"] for m in applied_items], ["M001"])

    def test_regex_apply_bom_conflict_and_cli_roundtrip(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cs_file, _, manifest_path = self._seed_workspace(root)
            cs_file.write_bytes(
                b"\xef\xbb\xbf"
                + b'namespace Demo;\npublic class Worker {\n    string k1 = "OLD_KEY";\n    string k2 = "CHANGED_ON_DISK";\n}\n'
            )
            mark_manifest(
                manifest_path,
                ids=["M001", "M002"],
                mark_all=False,
                category=None,
                file_contains=None,
                disposition="modify",
            )

            cli_show = subprocess.run(
                [sys.executable, str(SCRIPTS_DIR / "grep_review.py"), "show", str(manifest_path), "--disposition", "modify", "--json"],
                capture_output=True,
                text=True,
                check=True,
            )
            self.assertEqual(len(json.loads(cli_show.stdout)["matches"]), 2)

            res = apply_manifest(
                manifest_path,
                replace_old=r"OLD_(\w+)",
                replace_new=r"NEW_\1",
                is_regex=True,
                dry_run=False,
                repo_root=root,
                run_consistency=False,
            )
            self.assertEqual(res["appliedIds"], ["M001"])
            self.assertEqual(len(res["conflicts"]), 1)
            self.assertEqual(res["conflicts"][0]["id"], "M002")
            raw_after = cs_file.read_bytes()
            self.assertTrue(raw_after.startswith(b"\xef\xbb\xbf"))
            self.assertIn(b"NEW_KEY", raw_after)

    def test_publish_tools_lock_fallback_selects_unlocked_live_dir(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bin_dir = Path(tmp)
            with (
                patch.object(publish_tools, "BIN_DIR", bin_dir),
                patch.object(
                    publish_tools,
                    "_can_replace_directory",
                    side_effect=lambda p: p.name != "code-xray-mcp-live",
                ),
            ):
                chosen = publish_tools._select_live_fallback_dir()
                self.assertEqual(chosen.name, "code-xray-mcp-live-2")


if __name__ == "__main__":
    unittest.main()
