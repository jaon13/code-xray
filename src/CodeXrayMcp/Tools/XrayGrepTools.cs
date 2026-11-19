using System.ComponentModel;
using CodeXrayMcp.Services;
using ModelContextProtocol.Server;

namespace CodeXrayMcp.Tools;

[McpServerToolType]
public sealed class XrayGrepTools
{
    private XrayGrepTools()
    {
    }

    [McpServerTool(Name = "grep_code")]
    [Description("Review-First high-speed multi-language grep across C#, XAML, Rust, TS/JS, Svelte, Python, config, and docs with tree-sitter enclosing symbol and category tagging. Strictly read-only: returns grouped matches and optionally saves an isolated .xray/reviews/*.json manifest for review before editing.")]
    public static async Task<string> GrepCodeAsync(
        [Description("Absolute path to the repository root directory or .slnx / .sln file.")] string solutionPath,
        [Description("Literal text or regular expression pattern to search for.")] string pattern,
        [Description("Treat pattern as a regular expression (default false).")] bool isRegex = false,
        [Description("Case-insensitive search (default false).")] bool caseInsensitive = false,
        [Description("Optional comma-separated file extensions (e.g. 'cs,xaml,rs,ts,svelte,py,json').")] string? extensions = null,
        [Description("Include recursive external references outside repo root (default false).")] bool includeExternal = false,
        [Description("Maximum matched files to return (default 50, up to 500).")] int maxFiles = 50,
        [Description("Save an isolated review manifest under .xray/reviews/grep_<ts>_<id>.json (default true).")] bool saveReview = true,
        CancellationToken ct = default)
    {
        var request = new XrayGrepRequest(
            SolutionPath: solutionPath,
            Pattern: pattern,
            IsRegex: isRegex,
            CaseInsensitive: caseInsensitive,
            Extensions: extensions,
            IncludeExternal: includeExternal,
            MaxFiles: maxFiles,
            SaveReview: saveReview);

        return await XrayGrepService.SearchAsync(request, ct);
    }
}
