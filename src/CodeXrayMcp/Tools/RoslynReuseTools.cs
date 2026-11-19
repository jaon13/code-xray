using System.ComponentModel;
using System.Text.Json;
using CodeXrayMcp.Services;
using CodeXrayMcp.Workspace;
using ModelContextProtocol.Server;

namespace CodeXrayMcp.Tools;

[McpServerToolType]
public sealed class RoslynReuseTools(ReusableApiIndexCache apiCache, XrayIndexService indexService)
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        WriteIndented = true,
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase
    };

    [McpServerTool(Name = "search_reusable_apis")]
    [Description("Pre-coding reuse discovery engine: searches public classes, functions, extension methods, interfaces, and converters in the repository (uses Roslyn semantic catalog on .NET solutions + referenced shared assemblies, and falls back to .xray/index.db on Rust/TS/Svelte/Python repositories).")]
    public async Task<string> SearchReusableApisAsync(
        [Description("Absolute path to the repository root directory or .slnx / .sln file.")] string solutionPath,
        [Description("Space-separated search keywords (e.g. 'visibility converter', 'debounce', 'serializer', 'dialog'). Use '*' to list public entries.")] string query,
        [Description("Optional kind filter: 'type', 'method', 'extension', or 'converter'.")] string? kind = null,
        [Description("Optional base/interface/return type filter (.NET Roslyn mode, e.g. 'IValueConverter', 'IDisposable').")] string? implementsOrReturnsType = null,
        [Description("Include referenced shared assemblies (.NET Roslyn mode, default true).")] bool includeReferencedAssemblies = true,
        [Description("Maximum candidates to return (default 35).")] int maxResults = 35,
        CancellationToken ct = default)
    {
        if (!SolutionWorkspaceCache.TryResolveSolutionPath(solutionPath, out _))
            return await indexService.SearchPublicSymbolsFromDbAsync(solutionPath, query, kind, maxResults, ct);

        var (scannedAssemblies, matches) = await apiCache.SearchAsync(
            solutionPath,
            query,
            kind,
            implementsOrReturnsType,
            includeReferencedAssemblies,
            maxResults,
            ct);

        return JsonSerializer.Serialize(new
        {
            query,
            kind,
            implementsOrReturnsType,
            scannedAssemblies,
            matchCount = matches.Count,
            reusableApis = matches
        }, JsonOptions);
    }

    [McpServerTool(Name = "find_structural_clones")]
    [Description("High-speed Rust native multi-language structural clone detector (tree-sitter + Rabin-Karp) across C#, XAML, Rust, TS/JS/Svelte, and Python. Scans the repository and optional external shared roots (XRAY_SHARED_ROOTS / .xray/config.json) to detect Type-1/2/3 clones and recommend consolidation targets.")]
    public async Task<string> FindStructuralClonesAsync(
        [Description("Absolute path to the repository root directory or .slnx / .sln file.")] string solutionPath,
        [Description("Minimum source lines in a duplicated block (default 12).")] int minStatements = 12,
        [Description("Minimum structural similarity percentage 50-100 (default 85.0).")] double minSimilarity = 85.0,
        [Description("Optional relative path or project substring filter (e.g. 'UI' or 'Services').")] string? pathFilter = null,
        [Description("Also cross-scan configured external shared roots (XRAY_SHARED_ROOTS or .xray/config.json) when present (default true).")] bool includeSharedRoots = true,
        [Description("Maximum clone groups to return (default 25).")] int maxGroups = 25,
        [Description("Optional comma-separated extensions (default: cs,xaml,rs,ts,tsx,js,jsx,svelte,py).")] string? extensions = null,
        CancellationToken ct = default)
    {
        var result = await StructuralCloneRunner.RunAsync(
            solutionPath,
            minStatements,
            minSimilarity,
            pathFilter,
            includeSharedRoots,
            maxGroups,
            extensions,
            ct);

        return JsonSerializer.Serialize(result, JsonOptions);
    }
}
