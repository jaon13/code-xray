#!/usr/bin/env python3
"""Unit tests for polyglot stack discovery and unified X-Ray orchestration."""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

from polyglot_xray import StepResult, detect_repo_stacks, run_polyglot_xray  # noqa: E402


class PolyglotXrayTests(unittest.TestCase):
    def test_detects_active_stacks_excluding_ignored_dirs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "src").mkdir()
            (root / "node_modules" / "pkg").mkdir(parents=True)
            (root / "src" / "main.ts").write_text("export const x = 1;\n", encoding="utf-8")
            (root / "scripts").mkdir()
            (root / "scripts" / "tool.py").write_text("def run() -> None: pass\n", encoding="utf-8")
            (root / "node_modules" / "pkg" / "ignored.csproj").write_text("<Project />", encoding="utf-8")

            stacks = detect_repo_stacks(root)
            self.assertTrue(stacks["web"])
            self.assertTrue(stacks["python"])
            self.assertFalse(stacks["dotnet"])
            self.assertFalse(stacks["rust"])
            self.assertFalse(stacks["xaml"])

    def test_strict_fails_when_any_step_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "app.py").write_text("x = 1\n", encoding="utf-8")

            with (
                patch("polyglot_xray._run_python_checks", return_value=[StepResult("Python Ruff", "failed", 1, "FAIL")]),
                patch("polyglot_xray._run_ast_grep", return_value=StepResult("AOP (ast-grep)", "passed", 0, "PASS")),
            ):
                report = run_polyglot_xray(root, strict=True)
                self.assertFalse(report["passed"])

            with (
                patch("polyglot_xray._run_python_checks", return_value=[StepResult("Python Ruff", "passed", 0, "PASS")]),
                patch("polyglot_xray._run_ast_grep", return_value=StepResult("AOP (ast-grep)", "passed", 0, "PASS")),
            ):
                report_ok = run_polyglot_xray(root, strict=True)
                self.assertTrue(report_ok["passed"])


if __name__ == "__main__":
    unittest.main()
