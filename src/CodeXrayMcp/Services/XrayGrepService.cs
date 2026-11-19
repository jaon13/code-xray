using System.Diagnostics;
using System.Globalization;
using System.Text;
using System.Text.Json;

namespace CodeXrayMcp.Services;

public sealed record XrayGrepRequest(
    string SolutionPath,
    string Pattern,
    bool IsRegex = false,
    bool CaseInsensitive = false,
    string? Extensions = null,
    bool IncludeExternal = false,
    int ContextLines = 2,
    int MaxFiles = 50,
    int MaxMatches = 250,
    bool SaveReview = true);

public static class XrayGrepService
{
    private static readonly JsonSerializerOptions JsonOptions = new() { WriteIndented = true };

    public static Task<string> SearchAsync(XrayGrepRequest request, CancellationToken ct = default)
    {
        ArgumentNullException.ThrowIfNull(request);
        if (string.IsNullOrWhiteSpace(request.SolutionPath))
            throw new ArgumentException("Solution path is required.", nameof(request));
        if (string.IsNullOrWhiteSpace(request.Pattern))
            throw new ArgumentException("Search pattern is required.", nameof(request));

        string repoRoot = XrayIndexService.ResolveRepoRoot(request.SolutionPath);
        string exe = CodeXrayBinaryResolver.TryResolveExecutable("xray-grep", "XRAY_GREP_EXE")
            ?? throw new InvalidOperationException(
                CodeXrayBinaryResolver.BuildMissingExecutableMessage("xray-grep", "XRAY_GREP_EXE"));

        return ExecuteGrepProcessAsync(exe, repoRoot, request, ct);
    }

    private static async Task<string> ExecuteGrepProcessAsync(
        string exe,
        string repoRoot,
        XrayGrepRequest request,
        CancellationToken ct)
    {
        using var proc = new Process();
        proc.StartInfo = BuildStartInfo(exe, repoRoot, request);
        proc.Start();

        Task<string> stdoutTask = proc.StandardOutput.ReadToEndAsync(ct);
        Task<string> stderrTask = proc.StandardError.ReadToEndAsync(ct);
        await proc.WaitForExitAsync(ct);

        string stdout = await stdoutTask;
        string stderr = await stderrTask;
        if (proc.ExitCode != 0)
            throw new InvalidOperationException($"xray-grep exited with {proc.ExitCode}: {stderr}");

        int brace = stdout.IndexOf('{', StringComparison.Ordinal);
        if (brace < 0)
            throw new InvalidOperationException("xray-grep produced no JSON payload.");

        using var doc = JsonDocument.Parse(stdout[brace..]);
        return WrapReadOnlyReport(doc.RootElement);
    }

    private static ProcessStartInfo BuildStartInfo(string exe, string repoRoot, XrayGrepRequest request)
    {
        var psi = new ProcessStartInfo
        {
            FileName = exe,
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            CreateNoWindow = true,
            StandardOutputEncoding = Encoding.UTF8
        };

        int context = Math.Clamp(request.ContextLines, 0, 10);
        int maxFiles = Math.Clamp(request.MaxFiles, 1, 500);
        int maxMatches = Math.Clamp(request.MaxMatches, 1, 2000);

        List<string> args =
        [
            "-r", repoRoot,
            "-p", request.Pattern,
            "--format", "json",
            "--context", context.ToString(CultureInfo.InvariantCulture),
            "--max-files", maxFiles.ToString(CultureInfo.InvariantCulture),
            "--max-matches", maxMatches.ToString(CultureInfo.InvariantCulture)
        ];

        if (request.IsRegex) args.Add("--regex");
        if (request.CaseInsensitive) args.Add("--case-insensitive");
        if (!string.IsNullOrWhiteSpace(request.Extensions))
        {
            args.Add("--ext");
            args.Add(request.Extensions.Trim());
        }
        if (request.IncludeExternal) args.Add("--include-external");
        if (request.SaveReview) args.Add("--save-review");

        foreach (string arg in args)
            psi.ArgumentList.Add(arg);

        return psi;
    }

    private static string WrapReadOnlyReport(JsonElement root)
    {
        string? manifest = root.TryGetProperty("reviewManifest", out JsonElement mEl)
            ? mEl.GetString()
            : null;
        string? nextStep = string.IsNullOrWhiteSpace(manifest)
            ? null
            : $"Review matches in '{manifest}', mark dispositions via scripts/grep_review.py mark, preview with apply --dry-run, then apply.";

        return JsonSerializer.Serialize(new
        {
            readOnly = true,
            nextStep,
            report = root
        }, JsonOptions);
    }
}
