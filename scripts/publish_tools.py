#!/usr/bin/env python3
"""Cross-platform build, publish, MCP registration, and portable bundler for 01.CodeXray."""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import stat
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parent.parent
RUST_CRATE_DIR = ROOT_DIR / "src" / "code-clone-xray"
MCP_CSPROJ = ROOT_DIR / "src" / "CodeXrayMcp" / "CodeXrayMcp.csproj"
BIN_DIR = ROOT_DIR / "bin"
MCP_OUT_DIR = BIN_DIR / "code-xray-mcp"
DIST_DIR = ROOT_DIR / "dist"


def is_windows() -> bool:
    return os.name == "nt"


def exe_name(stem: str) -> str:
    return f"{stem}.exe" if is_windows() else stem


def stop_running_mcp() -> None:
    if is_windows():
        for exe in ("CodeXrayMcp.exe", "RoslynGraphMcp.exe"):
            subprocess.run(
                ["taskkill", "/F", "/IM", exe],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )
    else:
        for pat in ("CodeXrayMcp", "RoslynGraphMcp"):
            subprocess.run(
                ["pkill", "-f", pat],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )


def build_rust_engine(skip_tests: bool = False) -> Path:
    print(f"[1/3] Building Rust engines (structural-clone, xray-index, xray-grep) in {RUST_CRATE_DIR} ...")
    if not skip_tests:
        subprocess.run(["cargo", "test"], cwd=RUST_CRATE_DIR, check=True)
    subprocess.run(["cargo", "build", "--release"], cwd=RUST_CRATE_DIR, check=True)

    BIN_DIR.mkdir(parents=True, exist_ok=True)
    primary_dest: Path | None = None
    for stem in ("structural-clone", "xray-index", "xray-grep"):
        built_bin = RUST_CRATE_DIR / "target" / "release" / exe_name(stem)
        if not built_bin.is_file():
            raise FileNotFoundError(f"Expected Rust binary not found: {built_bin}")
        dest_bin = BIN_DIR / exe_name(stem)
        _copy_binary_with_lock_fallback(built_bin, dest_bin)
        if not is_windows():
            dest_bin.chmod(dest_bin.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        print(f"  -> Deployed {dest_bin}")
        if primary_dest is None:
            primary_dest = dest_bin

    assert primary_dest is not None
    return primary_dest


def _copy_binary_with_lock_fallback(src: Path, dest: Path) -> None:
    try:
        shutil.copy2(src, dest)
    except PermissionError:
        backup = dest.with_name(f"{dest.name}.old")
        if backup.exists():
            try:
                backup.unlink()
            except OSError:
                pass
        os.replace(dest, backup)
        shutil.copy2(src, dest)


def _can_replace_directory(dest_dir: Path) -> bool:
    if not dest_dir.exists():
        return True
    for entry in dest_dir.rglob("*"):
        if not entry.is_file() or entry.suffix.lower() not in {".exe", ".dll"}:
            continue
        try:
            with entry.open("r+b"):
                pass
        except OSError:
            return False
    return True


def _deploy_stage_dir(stage_dir: Path, dest_dir: Path) -> None:
    if dest_dir.exists():
        shutil.rmtree(dest_dir, ignore_errors=False)
    shutil.copytree(stage_dir, dest_dir, dirs_exist_ok=True)


def _select_live_fallback_dir() -> Path:
    candidates = [BIN_DIR / "code-xray-mcp-live"]
    for idx in range(2, 10):
        candidates.append(BIN_DIR / f"code-xray-mcp-live-{idx}")
    for candidate in candidates:
        if _can_replace_directory(candidate):
            return candidate
    return BIN_DIR / f"code-xray-mcp-live-{os.getpid()}"


def publish_code_xray_mcp(kill_running: bool = False) -> Path:
    stage_dir = BIN_DIR / "_stage_code_xray_mcp"
    if stage_dir.exists():
        shutil.rmtree(stage_dir, ignore_errors=True)
    stage_dir.mkdir(parents=True, exist_ok=True)

    print(f"[2/3] Publishing .NET 10 CodeXrayMcp to staging ({stage_dir}) ...")
    subprocess.run(
        [
            "dotnet",
            "publish",
            str(MCP_CSPROJ),
            "-c",
            "Release",
            "-o",
            str(stage_dir),
            "--nologo",
            "--disable-build-servers",
            "-v",
            "quiet",
        ],
        check=True,
    )

    dest_dir = MCP_OUT_DIR
    if kill_running:
        stop_running_mcp()

    try:
        if _can_replace_directory(MCP_OUT_DIR):
            try:
                _deploy_stage_dir(stage_dir, MCP_OUT_DIR)
            except (PermissionError, shutil.Error, OSError):
                dest_dir = _select_live_fallback_dir()
                _deploy_stage_dir(stage_dir, dest_dir)
                print(f"  -> Active IDE locked {MCP_OUT_DIR}; staged live side-by-side at {dest_dir}")
        else:
            dest_dir = _select_live_fallback_dir()
            _deploy_stage_dir(stage_dir, dest_dir)
            print(f"  -> Active IDE locked {MCP_OUT_DIR}; staged live side-by-side at {dest_dir}")

        live_dir = BIN_DIR / "code-xray-mcp-live"
        if dest_dir == MCP_OUT_DIR and live_dir.exists() and _can_replace_directory(live_dir):
            shutil.rmtree(live_dir, ignore_errors=True)
    finally:
        shutil.rmtree(stage_dir, ignore_errors=True)

    apphost = dest_dir / exe_name("CodeXrayMcp")
    dll_path = dest_dir / "CodeXrayMcp.dll"
    target = apphost if apphost.is_file() else dll_path
    if not target.is_file():
        raise FileNotFoundError(f"Published MCP entrypoint not found in {dest_dir}")
    if not is_windows() and apphost.is_file():
        apphost.chmod(apphost.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    print(f"  -> Published {target}")
    return target


def register_mcp_configs(mcp_entry: Path) -> None:
    print("[3/3] Updating Cursor & Antigravity MCP configurations ...")
    home = Path.home()
    if mcp_entry.suffix.lower() == ".dll":
        cmd = "dotnet"
        args_list = [str(mcp_entry)]
    else:
        cmd = str(mcp_entry)
        args_list = []

    targets = [
        (home / ".cursor" / "mcp.json", False),
        (home / ".gemini" / "config" / "mcp_config.json", True),
    ]
    for cfg_path, include_env in targets:
        if not cfg_path.parent.exists():
            continue
        data: dict = {"mcpServers": {}}
        if cfg_path.is_file():
            data = json.loads(cfg_path.read_text(encoding="utf-8-sig"))
            data.setdefault("mcpServers", {})
        data["mcpServers"].pop("roslyn-graph", None)
        entry: dict = {"command": cmd, "args": args_list}
        if include_env:
            entry["env"] = {}
        data["mcpServers"]["code-xray"] = entry
        cfg_path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print(f"  -> Registered code-xray in {cfg_path}")

    tooling_path = Path(os.environ.get("TOOLING_JSON", str(home / ".agents" / "tooling.json")))
    if tooling_path.is_file():
        t_data = json.loads(tooling_path.read_text(encoding="utf-8-sig"))
        if isinstance(t_data, dict):
            t_data.pop("roslynGraphMcpExe", None)
            t_data["codeXrayMcpExe"] = str(mcp_entry)
            tooling_path.write_text(json.dumps(t_data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
            print(f"  -> Synced codeXrayMcpExe in {tooling_path}")


def create_portable_bundle() -> Path:
    os_tag = platform.system().lower()
    arch_tag = platform.machine().lower()
    DIST_DIR.mkdir(parents=True, exist_ok=True)
    zip_path = DIST_DIR / f"CodeXray-{os_tag}-{arch_tag}.zip"

    with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        for folder in (BIN_DIR, ROOT_DIR / "scripts"):
            if not folder.exists():
                continue
            for item in folder.rglob("*"):
                if item.is_file() and "__pycache__" not in item.parts:
                    zf.write(item, item.relative_to(ROOT_DIR))
        readme = ROOT_DIR / "README.md"
        if readme.is_file():
            zf.write(readme, "README.md")

    print(f"  -> Portable archive created: {zip_path}")
    return zip_path


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    parser = argparse.ArgumentParser(description="Build, publish, and package 01.CodeXray.")
    parser.add_argument("--skip-rust", action="store_true", help="Skip Rust cargo build")
    parser.add_argument("--skip-dotnet", action="store_true", help="Skip .NET MCP publish")
    parser.add_argument("--skip-tests", action="store_true", help="Skip cargo test before release build")
    parser.add_argument("--register-mcp", action="store_true", help="Update ~/.cursor and ~/.gemini MCP configs")
    parser.add_argument("--kill-running", action="store_true", help="Terminate running CodeXrayMcp processes before replacing bin/")
    parser.add_argument("--package", action="store_true", help="Create portable dist/CodeXray-<os>-<arch>.zip")
    args = parser.parse_args()

    if not args.skip_rust:
        build_rust_engine(skip_tests=args.skip_tests)

    mcp_entry = MCP_OUT_DIR / exe_name("CodeXrayMcp")
    if not args.skip_dotnet:
        mcp_entry = publish_code_xray_mcp(kill_running=args.kill_running)
    elif not mcp_entry.is_file():
        mcp_entry = MCP_OUT_DIR / "CodeXrayMcp.dll"

    if args.register_mcp and mcp_entry.is_file():
        register_mcp_configs(mcp_entry)
    else:
        print("[3/3] MCP config registration skipped (pass --register-mcp to update).")

    if args.package:
        create_portable_bundle()

    print("Done.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
