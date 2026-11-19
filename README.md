# CodeXray (`code-xray`) — Multi-Language Code Intelligence, Clone Detection & Governance Suite

Standalone, cross-platform static analysis, multi-language SQLite symbol/slice indexer, Review-First grep, structural clone detector, and .NET/Rust/Polyglot governance suite.

## Supported Languages

| Feature | C# / XAML | Rust | TS / JS / JSX | Svelte (`.svelte`) | Python | Config / Docs |
|---|---|---|---|---|---|---|
| `xray-index` (`.xray/index.db`) | `.csproj`/`.slnx` + Roslyn/tree-sitter | `Cargo.toml` + `struct`/`enum`/`trait`/`impl`/`use` | `package.json` + classes/interfaces/functions/imports | Component + `<script>` TS/JS + props/events/context | `pyproject.toml` + classes/defs/imports/`Depends` | — |
| `xray-grep` (`--save-review`) | Enclosing symbol + category | Enclosing symbol + `#[cfg(test)]` awareness | Enclosing class/fn symbol | Enclosing component & `<script>` symbol | Enclosing class/def symbol | Docs & config classification |
| `structural-clone` | Full AST token normalization | Full AST + `#[cfg(test)]` filtering | Full AST normalization | `<script>` TS/JS + `<style>` CSS | Full AST normalization | HTML / CSS |
| `CodeXrayMcp` (`code-xray` MCP) | Full SQLite + Roslyn semantic callers/blast radius | SQLite graph slice, overview, deps, grep, clones | SQLite graph slice, overview, deps, grep, clones | SQLite graph slice, overview, deps, grep, clones | SQLite graph slice, overview, deps, grep, clones | Grep review |

## Quick Install (1-Line Setup)

Installs `CodeXray` under `~/.agents/code-xray`, deploys global wrappers (`~/.agents/bin/{xray_run.py,codexray-hook.py}`), initializes `~/.agents/tooling.json`, and registers `"code-xray"` in Antigravity (`~/.gemini/config/mcp_config.json`) & Cursor (`~/.cursor/mcp.json`). Uses prebuilt GitHub Release bundles when available and falls back cleanly to a local source build (`cargo` + `.NET 10 SDK`).

**Windows (PowerShell):**
```powershell
irm https://raw.githubusercontent.com/jaon13/code-xray/main/install.ps1 | iex
```

**macOS / Linux (Bash):**
```bash
curl -fsSL https://raw.githubusercontent.com/jaon13/code-xray/main/install.sh | bash
```

---

## Directory Structure

```text
CodeXray/
├── CodeXray.slnx                        # .NET 10 solution for CodeXrayMcp
├── Directory.Build.props                # Self-governance (.NET 10 analyzers, SARIF v2.1, BannedApiAnalyzers)
├── .editorconfig                        # Shared C# analyzer severity policy
├── BannedSymbols.txt                    # RS0030 banned API list
├── bin/                                 # Deployed binaries (populated by publish_tools.py)
│   ├── structural-clone[.exe]           # Rust tree-sitter + Rabin-Karp structural clone detector
│   ├── xray-index[.exe]                 # Rust SQLite multi-language indexer & graph-slice CLI
│   ├── xray-grep[.exe]                  # Rust Review-First multi-language grep CLI
│   └── code-xray-mcp/                   # Published .NET 10 CodeXrayMcp stdio MCP server
├── dist/                                # Portable release bundles (CodeXray-<os>-<arch>.zip)
├── templates/                           # Standard Rust, ast-grep, and OpenGrep governance templates
├── scripts/                             # Python 3.10+ cross-platform CLI gates, hooks & unit tests
│   ├── publish_tools.py                 # One-click Rust + .NET build, `code-xray` MCP registration & zip bundler
│   ├── grep_review.py                   # Review-First grep manifest inspect/mark/dry-run/apply CLI
│   ├── dotnet_xray.py                   # SARIF v2.1 scorecard, blast radius, 300-line & clone X-Ray
│   ├── rust_xray.py                     # Workspace-aware Rust Clippy SARIF, fmt & deny/audit X-Ray
│   ├── xaml_style_xray.py               # WPF/XAML ResourceDictionary, Style & Template clone analyzer
│   ├── polyglot_xray.py                 # Unified multi-stack X-Ray gate (.NET/XAML/Rust/Web/Python)
│   ├── init_rust_governance.py          # Bootstrap deny.toml/rustfmt.toml/clippy.toml/[workspace.lints]
│   ├── verify_dotnet.py                 # .NET 10 + mixed repo stabilization gate
│   ├── verify_rust.py                   # Standalone Rust 5-stage stabilization gate (ratchet)
│   ├── codebase_map.py                  # Lightweight no-build solution/layer/hotspot mapper
│   ├── check_consistency.py             # Fast no-build touched-file consistency checker (.NET + Rust)
│   └── hooks/                           # PreToolUse & Stop safety/quality hooks
└── src/
    ├── CodeXrayMcp/                     # Multi-language & Roslyn stdio MCP server (`code-xray`)
    └── code-clone-xray/                 # Rust clone, multi-lang indexer & grep engine
```

## Binary Resolution Order

`CodeXrayMcp` resolves `xray-index`, `xray-grep`, and `structural-clone` in this order:
1. `XRAY_INDEX_EXE` / `XRAY_GREP_EXE` / `STRUCTURAL_CLONE_EXE`
2. `CODEXRAY_HOME` (`<CODEXRAY_HOME>/bin/<exe>` or `<CODEXRAY_HOME>/<exe>`)
3. `~/.agents/tooling.json` (`codeXrayRoot`, overridable via `TOOLING_JSON`)
4. `AppContext.BaseDirectory` and deployment-relative `bin/` / `target/release/` locations
5. `~/.agents/bin/` and `~/.local/bin/`
6. System `PATH`

## Optional Cross-Repo Shared Roots

To cross-scan external shared libraries during `find_structural_clones`, configure any of:
- Environment variable `XRAY_SHARED_ROOTS` (`;` on Windows or `:` on POSIX)
- Repository `.xray/config.json`: `{ "sharedRoots": ["../shared-core"] }`
- Global `~/.agents/tooling.json`: `{ "sharedRoots": ["..."] }`

## Quick Start (From Cloned Repo)

### 1. Build, Install Wrappers & Register `code-xray` MCP
```bash
python scripts/publish_tools.py --register-mcp --install-wrappers
```
If `CodeXrayMcp` is held open by a running IDE MCP client, `publish_tools.py` stages a side-by-side copy under `bin/code-xray-mcp-live/` and updates `~/.cursor/mcp.json`, `~/.gemini/config/mcp_config.json`, and `~/.agents/tooling.json`.

### 2. Create Portable Bundle
```bash
python scripts/publish_tools.py --package
```
Produces `dist/CodeXray-<os>-<arch>.zip` with prebuilt binaries, templates, wrappers, and Python scripts. Extract on another machine and run:
```bash
python scripts/publish_tools.py --skip-rust --skip-dotnet --register-mcp --install-wrappers
```
