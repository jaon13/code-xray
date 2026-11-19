using System.Collections.Concurrent;
using System.Diagnostics;

namespace CodeXrayMcp.Services;

public sealed class XrayIndexService
{
    private readonly ConcurrentDictionary<string, SemaphoreSlim> _buildLocks = new(StringComparer.OrdinalIgnoreCase);

    public static string ResolveRepoRoot(string solutionOrRepoPath)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(solutionOrRepoPath);
        string full = Path.GetFullPath(solutionOrRepoPath.Trim());
        if (File.Exists(full))
            return Path.GetDirectoryName(full) ?? Directory.GetCurrentDirectory();

        if (!Directory.Exists(full))
            throw new DirectoryNotFoundException($"Solution or repository path not found: {solutionOrRepoPath}");

        var dir = new DirectoryInfo(full);
        while (dir is not null)
        {
            if (dir.EnumerateFiles("*.slnx").Any()
                || dir.EnumerateFiles("*.sln").Any()
                || Directory.Exists(Path.Combine(dir.FullName, ".git")))
            {
                return dir.FullName;
            }
            dir = dir.Parent;
        }
        return full;
    }

    public static string ResolveIndexerExecutable() =>
        CodeXrayBinaryResolver.TryResolveExecutable("xray-index", "XRAY_INDEX_EXE")
        ?? CodeXrayBinaryResolver.ExecutableName("xray-index");

    public async Task<string> BuildOrRefreshAsync(
        string solutionOrRepoPath,
        bool force = false,
        bool includeExternal = true,
        CancellationToken ct = default)
    {
        string repoRoot = ResolveRepoRoot(solutionOrRepoPath);
        var gate = _buildLocks.GetOrAdd(repoRoot, _ => new SemaphoreSlim(1, 1));
        await gate.WaitAsync(ct);
        try
        {
            List<string> args = ["build", "-r", repoRoot];
            if (force) args.Add("--force");
            if (!includeExternal) args.Add("--no-external");
            return await RunExeAsync(args, ct);
        }
        finally
        {
            gate.Release();
        }
    }

    public async Task<string> RunQueryAsync(
        string solutionOrRepoPath,
        string subcommand,
        IEnumerable<string> extraArgs,
        bool ensureFresh,
        CancellationToken ct)
    {
        string repoRoot = ResolveRepoRoot(solutionOrRepoPath);
        string dbPath = Path.Combine(repoRoot, ".xray", "index.db");
        if (ensureFresh || !File.Exists(dbPath))
            await BuildOrRefreshAsync(repoRoot, force: false, includeExternal: true, ct);

        List<string> args = [subcommand, "-r", repoRoot, "--db", dbPath];
        args.AddRange(extraArgs);
        if (subcommand != "status")
            args.Add("--no-refresh");
        return await RunExeAsync(args, ct);
    }

    public async Task<string?> TryGetTypeOverviewFromDbAsync(
        string solutionOrRepoPath,
        string typeName,
        int maxMembers,
        CancellationToken ct)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(typeName);
        string repoRoot = ResolveRepoRoot(solutionOrRepoPath);
        string dbPath = Path.Combine(repoRoot, ".xray", "index.db");
        if (!File.Exists(dbPath))
            await BuildOrRefreshAsync(repoRoot, force: false, includeExternal: true, ct);

        return await XrayIndexSqliteQueryService.QueryTypeOverviewAsync(dbPath, typeName, maxMembers, ct);
    }

    public async Task<string> SearchPublicSymbolsFromDbAsync(
        string solutionOrRepoPath,
        string query,
        string? kind,
        int maxResults,
        CancellationToken ct)
    {
        string repoRoot = ResolveRepoRoot(solutionOrRepoPath);
        string dbPath = Path.Combine(repoRoot, ".xray", "index.db");
        if (!File.Exists(dbPath))
            await BuildOrRefreshAsync(repoRoot, force: false, includeExternal: true, ct);

        return await XrayIndexSqliteQueryService.QueryPublicSymbolsAsync(dbPath, query, kind, maxResults, ct);
    }

    private static async Task<string> RunExeAsync(IReadOnlyList<string> args, CancellationToken ct)
    {
        string exe = CodeXrayBinaryResolver.TryResolveExecutable("xray-index", "XRAY_INDEX_EXE")
            ?? throw new InvalidOperationException(
                CodeXrayBinaryResolver.BuildMissingExecutableMessage("xray-index", "XRAY_INDEX_EXE"));

        var psi = new ProcessStartInfo
        {
            FileName = exe,
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            CreateNoWindow = true
        };
        foreach (string a in args)
            psi.ArgumentList.Add(a);

        using var proc = Process.Start(psi) ?? throw new InvalidOperationException($"Failed to start {psi.FileName}");
        Task<string> outTask = proc.StandardOutput.ReadToEndAsync(ct);
        Task<string> errTask = proc.StandardError.ReadToEndAsync(ct);
        await proc.WaitForExitAsync(ct);
        string stdout = await outTask;
        string stderr = await errTask;
        if (proc.ExitCode != 0)
            throw new InvalidOperationException($"xray-index exited with {proc.ExitCode}: {stderr}");
        return stdout;
    }
}
