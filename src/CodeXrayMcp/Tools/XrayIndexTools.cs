using System.ComponentModel;
using System.Globalization;
using CodeXrayMcp.Services;
using ModelContextProtocol.Server;

namespace CodeXrayMcp.Tools;

[McpServerToolType]
public sealed class XrayIndexTools(XrayIndexService indexService)
{
    [McpServerTool(Name = "refresh_code_index")]
    [Description("Builds or incrementally refreshes the SQLite multi-language codebase index (.xray/index.db) across C#, XAML, Rust, TypeScript/JavaScript/Svelte, Python, and external ProjectReferences. Returns counts and elapsed time.")]
    public async Task<string> RefreshCodeIndexAsync(
        [Description("Absolute path to the repository root directory or .slnx / .sln file.")] string solutionPath,
        [Description("Force full reparse ignoring mtime/hash cache (default false).")] bool force = false,
        [Description("Include recursive external references outside repo root (default true).")] bool includeExternal = true,
        CancellationToken ct = default)
    {
        return await indexService.BuildOrRefreshAsync(solutionPath, force, includeExternal, ct);
    }

    [McpServerTool(Name = "get_feature_slice")]
    [Description("Fast multi-language SQLite graph-slice discovery: finds seed types/structs/components/modules, action keys, partial clusters, DI/context bindings, event/message flows, and ranked files to read in milliseconds across C#, XAML, Rust, TS/JS/Svelte, and Python.")]
    public async Task<string> GetFeatureSliceAsync(
        [Description("Absolute path to the repository root directory or .slnx / .sln file.")] string solutionPath,
        [Description("Space-separated search keywords, symbol name, or constant key (e.g. 'OrderProcessor', 'OrderPanel', 'ACTION_RUN').")] string query,
        [Description("Graph expansion depth in hops (1-4, default 2).")] int hops = 2,
        [Description("Maximum ranked files to return (default 25).")] int limit = 25,
        [Description("Perform fast incremental freshness check before querying (default true).")] bool refresh = true,
        CancellationToken ct = default)
    {
        return await indexService.RunQueryAsync(
            solutionPath,
            "slice",
            ["-q", query, "--hops", hops.ToString(CultureInfo.InvariantCulture), "--limit", limit.ToString(CultureInfo.InvariantCulture)],
            ensureFresh: refresh,
            ct);
    }

    [McpServerTool(Name = "get_partial_clusters")]
    [Description("Lists split partial types grouped across multiple files (e.g. partial classes/ViewModels) with line counts per file.")]
    public async Task<string> GetPartialClustersAsync(
        [Description("Absolute path to the repository root directory or .slnx / .sln file.")] string solutionPath,
        [Description("Optional case-insensitive type name substring filter.")] string? filter = null,
        [Description("Minimum number of partial files required (default 2).")] int minFiles = 2,
        [Description("Perform fast incremental freshness check before querying (default true).")] bool refresh = true,
        CancellationToken ct = default)
    {
        List<string> args = ["--min-files", minFiles.ToString(CultureInfo.InvariantCulture)];
        if (!string.IsNullOrWhiteSpace(filter))
        {
            args.Add("--filter");
            args.Add(filter);
        }
        return await indexService.RunQueryAsync(solutionPath, "partials", args, ensureFresh: refresh, ct);
    }

    [McpServerTool(Name = "get_architecture_links")]
    [Description("Queries DI/context/state registrations, outgoing/incoming type or import references, trait implementations, and event/messenger send-receive links for a symbol or project from .xray/index.db.")]
    public async Task<string> GetArchitectureLinksAsync(
        [Description("Absolute path to the repository root directory or .slnx / .sln file.")] string solutionPath,
        [Description("Target symbol, trait, interface, message type, or project name substring (e.g. 'OrderProcessor', 'IOrderRepository').")] string target,
        [Description("Perform fast incremental freshness check before querying (default true).")] bool refresh = true,
        CancellationToken ct = default)
    {
        return await indexService.RunQueryAsync(solutionPath, "deps", ["-t", target], ensureFresh: refresh, ct);
    }
}
