using System.Diagnostics;
using System.Globalization;
using System.Text;
using System.Text.Json;

namespace CodeXrayMcp.Services;

public static class StructuralCloneRunner
{
    public const string DefaultExtensions = "cs,xaml,rs,ts,tsx,js,jsx,svelte,py";

    public static async Task<CloneScanResult> RunAsync(
        string solutionOrRepoPath,
        int minLines,
        double minSimilarity,
        string? pathFilter,
        bool includeSharedRoots,
        int maxGroups,
        string? extensions = null,
        CancellationToken ct = default)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(solutionOrRepoPath);
        string repoRoot = XrayIndexService.ResolveRepoRoot(solutionOrRepoPath);
        string exe = CodeXrayBinaryResolver.TryResolveExecutable("structural-clone", "STRUCTURAL_CLONE_EXE")
            ?? throw new InvalidOperationException(
                CodeXrayBinaryResolver.BuildMissingExecutableMessage("structural-clone", "STRUCTURAL_CLONE_EXE"));

        var scanRoots = CollectScanRoots(repoRoot, includeSharedRoots);
        int effectiveMinLines = Math.Max(4, minLines);
        int fetchTop = Math.Max(maxGroups * 4, 80);
        string effectiveExt = string.IsNullOrWhiteSpace(extensions) ? DefaultExtensions : extensions.Trim();

        using var proc = new Process();
        proc.StartInfo = BuildStartInfo(exe, scanRoots, effectiveExt, effectiveMinLines, minSimilarity, fetchTop);

        proc.Start();
        Task<string> stdoutTask = proc.StandardOutput.ReadToEndAsync(ct);
        Task<string> stderrTask = proc.StandardError.ReadToEndAsync(ct);
        await proc.WaitForExitAsync(ct);

        string stdout = await stdoutTask;
        string stderr = await stderrTask;
        if (proc.ExitCode != 0)
            throw new InvalidOperationException($"structural-clone failed (exit {proc.ExitCode}): {stderr}\n{stdout}");

        int brace = stdout.IndexOf('{', StringComparison.Ordinal);
        if (brace < 0)
            throw new InvalidOperationException("structural-clone produced no JSON payload.");

        using var doc = JsonDocument.Parse(stdout[brace..]);
        return CloneReportEnricher.ParseAndEnrich(doc.RootElement, repoRoot, scanRoots, effectiveMinLines, pathFilter, maxGroups);
    }

    private static ProcessStartInfo BuildStartInfo(
        string exe,
        IReadOnlyList<string> scanRoots,
        string extensions,
        int effectiveMinLines,
        double minSimilarity,
        int fetchTop)
    {
        var startInfo = new ProcessStartInfo
        {
            FileName = exe,
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            CreateNoWindow = true,
            StandardOutputEncoding = Encoding.UTF8
        };

        foreach (string root in scanRoots)
        {
            startInfo.ArgumentList.Add("--path");
            startInfo.ArgumentList.Add(root);
        }

        string[] tailArgs =
        [
            "--extensions", extensions,
            "--min-lines", effectiveMinLines.ToString(CultureInfo.InvariantCulture),
            "--min-similarity", Math.Clamp(minSimilarity, 50.0, 100.0).ToString("F1", CultureInfo.InvariantCulture),
            "--top", fetchTop.ToString(CultureInfo.InvariantCulture),
            "--format", "json"
        ];
        foreach (string arg in tailArgs)
            startInfo.ArgumentList.Add(arg);

        return startInfo;
    }

    private static List<string> CollectScanRoots(string repoRoot, bool includeSharedRoots)
    {
        var roots = new List<string> { repoRoot };
        if (!includeSharedRoots)
            return roots;

        foreach (string candidate in ReadConfiguredSharedRoots(repoRoot))
        {
            string full = Path.GetFullPath(candidate);
            if (!Directory.Exists(full))
                continue;
            if (roots.Contains(full, StringComparer.OrdinalIgnoreCase))
                continue;
            if (IsSubPathOf(full, repoRoot))
                continue;
            roots.Add(full);
        }

        return roots;
    }

    private static List<string> ReadConfiguredSharedRoots(string repoRoot)
    {
        var found = new List<string>();
        string? envRoots = Environment.GetEnvironmentVariable("XRAY_SHARED_ROOTS");
        if (!string.IsNullOrWhiteSpace(envRoots))
        {
            foreach (string raw in envRoots.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
                found.Add(raw);
        }

        ReadSharedRootsFromJson(Path.Combine(repoRoot, ".xray", "config.json"), repoRoot, found);
        string? toolingJson = CodeXrayBinaryResolver.ResolveToolingJsonPath();
        if (!string.IsNullOrWhiteSpace(toolingJson))
            ReadSharedRootsFromJson(toolingJson, Path.GetDirectoryName(toolingJson) ?? repoRoot, found);

        return found;
    }

    private static void ReadSharedRootsFromJson(string jsonPath, string baseDir, List<string> output)
    {
        if (!File.Exists(jsonPath))
            return;
        try
        {
            using FileStream stream = File.OpenRead(jsonPath);
            using JsonDocument doc = JsonDocument.Parse(stream);
            if (doc.RootElement.ValueKind != JsonValueKind.Object)
                return;
            if (!doc.RootElement.TryGetProperty("sharedRoots", out JsonElement arr) || arr.ValueKind != JsonValueKind.Array)
                return;

            using var enumerator = arr.EnumerateArray();
            while (enumerator.MoveNext())
            {
                string? raw = enumerator.Current.GetString();
                if (string.IsNullOrWhiteSpace(raw))
                    continue;
                string expanded = Environment.ExpandEnvironmentVariables(raw.Trim());
                string resolved = Path.IsPathRooted(expanded) ? expanded : Path.Combine(baseDir, expanded);
                output.Add(resolved);
            }
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or JsonException)
        {
            // Ignore malformed optional sharedRoots config files.
        }
    }

    private static bool IsSubPathOf(string candidate, string parent)
    {
        string cNorm = candidate.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar) + Path.DirectorySeparatorChar;
        string pNorm = parent.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar) + Path.DirectorySeparatorChar;
        return cNorm.StartsWith(pNorm, StringComparison.OrdinalIgnoreCase);
    }
}
