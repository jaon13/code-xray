#!/usr/bin/env python3
"""Unit tests for public wrappers (xray_run.py, codexray-hook.py, sync_tooling.py) and publish_tools."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT_DIR = Path(__file__).resolve().parents[2]
WRAPPERS_DIR = ROOT_DIR / "wrappers"
SCRIPTS_DIR = ROOT_DIR / "scripts"
for entry in (str(WRAPPERS_DIR), str(SCRIPTS_DIR)):
    if entry not in sys.path:
        sys.path.insert(0, entry)

import sync_tooling  # noqa: E402
import xray_run  # noqa: E402


class PublicWrappersTests(unittest.TestCase):
    def test_resolution_priority_env_over_tooling_json_over_default(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            env_root = tmp_path / "env_xray"
            json_root = tmp_path / "json_xray"

            tooling_file = tmp_path / "tooling.json"
            tooling_file.write_text(
                json.dumps({"codeXrayRoot": str(json_root)}),
                encoding="utf-8",
            )

            with mock.patch.dict(
                os.environ,
                {"CODEXRAY_HOME": str(env_root), "TOOLING_JSON": str(tooling_file)},
                clear=True,
            ):
                self.assertEqual(xray_run.resolve_codexray_root(), env_root)

            with mock.patch.dict(os.environ, {"TOOLING_JSON": str(tooling_file)}, clear=True):
                self.assertEqual(xray_run.resolve_codexray_root(), json_root)

            missing_json = tmp_path / "missing.json"
            with mock.patch.dict(os.environ, {"TOOLING_JSON": str(missing_json)}, clear=True):
                self.assertEqual(xray_run.resolve_codexray_root(), xray_run.DEFAULT_CODEXRAY_ROOT)

    def test_sync_mcp_file_updates_code_xray_and_removes_legacy_key(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            mcp_file = tmp_path / "mcp_config.json"
            mcp_file.write_text(
                json.dumps(
                    {
                        "mcpServers": {
                            "other": {"command": "npx", "args": []},
                            "roslyn-graph": {"command": "old", "args": [], "env": {}},
                        }
                    }
                ),
                encoding="utf-8",
            )
            exe_path = tmp_path / "CodeXrayMcp.exe"
            changed = sync_tooling.sync_mcp_file(mcp_file, exe_path, include_env=True, check_only=False)
            self.assertTrue(changed)
            updated = json.loads(mcp_file.read_text(encoding="utf-8"))
            self.assertNotIn("roslyn-graph", updated["mcpServers"])
            self.assertEqual(updated["mcpServers"]["code-xray"]["command"], str(exe_path))
            self.assertEqual(updated["mcpServers"]["other"]["command"], "npx")

    def test_codexray_hook_dispatches_and_fails_open(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cx_root = Path(tmp) / "code-xray"
            hooks_dir = cx_root / "scripts" / "hooks"
            hooks_dir.mkdir(parents=True)
            (hooks_dir / "stop_guard.py").write_text(
                "import json, sys\np = json.loads(sys.stdin.read())\nprint(json.dumps({'decision': 'stop', 'seen': p.get('ok')}))\n",
                encoding="utf-8",
            )
            env = os.environ.copy()
            env["CODEXRAY_HOME"] = str(cx_root)
            proc = subprocess.run(
                [sys.executable, str(WRAPPERS_DIR / "codexray-hook.py"), "stop_guard"],
                input='{"ok": true}',
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertEqual(json.loads(proc.stdout.strip()), {"decision": "stop", "seen": True})

            env["CODEXRAY_HOME"] = str(Path(tmp) / "missing")
            proc_missing = subprocess.run(
                [sys.executable, str(WRAPPERS_DIR / "codexray-hook.py"), "stop_guard"],
                input="{}",
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(proc_missing.returncode, 0)
            self.assertEqual(json.loads(proc_missing.stdout.strip()), {"decision": "stop"})


if __name__ == "__main__":
    unittest.main()
