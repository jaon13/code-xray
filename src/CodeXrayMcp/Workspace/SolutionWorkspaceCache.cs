using System.Collections.Concurrent;
using System.Diagnostics.CodeAnalysis;
using System.Text;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.MSBuild;
using Microsoft.CodeAnalysis.Text;

namespace CodeXrayMcp.Workspace;

public sealed class SolutionWorkspaceCache : IDisposable
{
    private readonly SemaphoreSlim _gate = new(1, 1);
    private readonly ConcurrentDictionary<string, CachedEntry> _entries = new(StringComparer.OrdinalIgnoreCase);

    public async Task<(Solution Solution, string RepoRoot)> GetSolutionAsync(string solutionOrRepoPath, CancellationToken ct)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(solutionOrRepoPath);
        if (!TryResolveSolutionPath(solutionOrRepoPath, out string? resolvedSln))
        {
            throw new FileNotFoundException(
                $"No .slnx or .sln file found at '{solutionOrRepoPath}'. Roslyn C# semantic queries require a .NET solution; use get_feature_slice, get_architecture_links, get_type_overview, or grep_code for non-.NET repositories.");
        }

        string repoRoot = Path.GetDirectoryName(resolvedSln) ?? Directory.GetCurrentDirectory();

        await _gate.WaitAsync(ct);
        try
        {
            if (_entries.TryGetValue(resolvedSln, out var existing) && !existing.NeedsFullReload)
            {
                existing.ApplyPendingIncrementalUpdates();
                return (existing.CurrentSolution, repoRoot);
            }

            existing?.Dispose();

            var entry = new CachedEntry(repoRoot);
            try
            {
                await entry.OpenAsync(resolvedSln, ct);
            }
            catch
            {
                entry.Dispose();
                throw;
            }
            _entries[resolvedSln] = entry;
            return (entry.CurrentSolution, repoRoot);
        }
        finally
        {
            _gate.Release();
        }
    }

    public bool TryGetCachedSolution(string solutionOrRepoPath, out (Solution Solution, string RepoRoot) loaded)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(solutionOrRepoPath);
        if (TryResolveSolutionPath(solutionOrRepoPath, out string? resolvedSln)
            && _entries.TryGetValue(resolvedSln, out var existing)
            && !existing.NeedsFullReload)
        {
            existing.ApplyPendingIncrementalUpdates();
            string repoRoot = Path.GetDirectoryName(resolvedSln) ?? Directory.GetCurrentDirectory();
            loaded = (existing.CurrentSolution, repoRoot);
            return true;
        }

        loaded = default;
        return false;
    }

    public static bool TryResolveSolutionPath(string inputPath, [NotNullWhen(true)] out string? resolvedPath)
    {
        resolvedPath = null;
        if (string.IsNullOrWhiteSpace(inputPath))
            return false;

        string full = Path.GetFullPath(inputPath);
        if (File.Exists(full)
            && (full.EndsWith(".slnx", StringComparison.OrdinalIgnoreCase)
                || full.EndsWith(".sln", StringComparison.OrdinalIgnoreCase)))
        {
            resolvedPath = full;
            return true;
        }

        string searchDir = Directory.Exists(full) ? full : (Path.GetDirectoryName(full) ?? full);
        if (!Directory.Exists(searchDir))
            return false;

        string? slnx = Directory.EnumerateFiles(searchDir, "*.slnx", SearchOption.TopDirectoryOnly).FirstOrDefault();
        if (slnx is not null)
        {
            resolvedPath = slnx;
            return true;
        }

        string? sln = Directory.EnumerateFiles(searchDir, "*.sln", SearchOption.TopDirectoryOnly).FirstOrDefault();
        if (sln is not null)
        {
            resolvedPath = sln;
            return true;
        }

        return false;
    }

    public static string MakeRelativePath(string repoRoot, string? filePath)
    {
        if (string.IsNullOrWhiteSpace(filePath))
            return string.Empty;
        try
        {
            return Path.GetRelativePath(repoRoot, filePath).Replace('\\', '/');
        }
        catch (ArgumentException)
        {
            return filePath;
        }
    }

    public void Dispose()
    {
        foreach (var kvp in _entries)
        {
            kvp.Value.Dispose();
        }
        _entries.Clear();
        _gate.Dispose();
    }

    private sealed class CachedEntry : IDisposable
    {
        private readonly MSBuildWorkspace _workspace;
        private readonly FileSystemWatcher _watcher;
        private readonly ConcurrentDictionary<string, byte> _dirtyCsFiles = new(StringComparer.OrdinalIgnoreCase);

        public Solution CurrentSolution { get; private set; }
        public bool NeedsFullReload { get; private set; }

        public CachedEntry(string repoRoot)
        {
            var properties = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
            {
                ["DesignTimeBuild"] = "true",
                ["BuildingInsideVisualStudio"] = "true",
                ["AlwaysCompileMarkupFilesInSeparateDomain"] = "false",
                ["RunAnalyzers"] = "false"
            };

            _workspace = MSBuildWorkspace.Create(properties);
            CurrentSolution = _workspace.CurrentSolution;

            _watcher = new FileSystemWatcher(repoRoot)
            {
                IncludeSubdirectories = true,
                NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.FileName | NotifyFilters.Size
            };

            _watcher.Changed += OnFileEvent;
            _watcher.Created += OnStructureEvent;
            _watcher.Deleted += OnStructureEvent;
            _watcher.Renamed += (_, e) => OnStructureEvent(null, e);
            _watcher.EnableRaisingEvents = true;
        }

        public async Task OpenAsync(string resolvedSln, CancellationToken ct) =>
            CurrentSolution = await _workspace.OpenSolutionAsync(resolvedSln, cancellationToken: ct);

        public void ApplyPendingIncrementalUpdates()
        {
            if (_dirtyCsFiles.IsEmpty)
                return;

            var updated = CurrentSolution;
            foreach (string filePath in _dirtyCsFiles.Keys.ToArray())
            {
                _dirtyCsFiles.TryRemove(filePath, out _);
                if (!File.Exists(filePath))
                    continue;

                var docIds = updated.GetDocumentIdsWithFilePath(filePath);
                if (docIds.IsEmpty)
                {
                    NeedsFullReload = true;
                    return;
                }

                string content;
                try
                {
                    content = File.ReadAllText(filePath, Encoding.UTF8);
                }
                catch (IOException)
                {
                    continue;
                }

                var sourceText = SourceText.From(content, Encoding.UTF8);
                foreach (var docId in docIds)
                {
                    updated = updated.WithDocumentText(docId, sourceText);
                }
            }

            CurrentSolution = updated;
        }

        private void OnFileEvent(object sender, FileSystemEventArgs e)
        {
            if (IsIgnoredPath(e.FullPath))
                return;

            if (e.FullPath.EndsWith(".cs", StringComparison.OrdinalIgnoreCase))
            {
                _dirtyCsFiles[e.FullPath] = 1;
            }
            else if (e.FullPath.EndsWith(".csproj", StringComparison.OrdinalIgnoreCase) ||
                     e.FullPath.EndsWith(".slnx", StringComparison.OrdinalIgnoreCase) ||
                     e.FullPath.EndsWith(".sln", StringComparison.OrdinalIgnoreCase))
            {
                NeedsFullReload = true;
            }
        }

        private void OnStructureEvent(object? sender, FileSystemEventArgs e)
        {
            if (IsIgnoredPath(e.FullPath))
                return;

            if (e.FullPath.EndsWith(".cs", StringComparison.OrdinalIgnoreCase) ||
                e.FullPath.EndsWith(".csproj", StringComparison.OrdinalIgnoreCase))
            {
                NeedsFullReload = true;
            }
        }

        private static bool IsIgnoredPath(string path) =>
            path.Contains($"{Path.DirectorySeparatorChar}bin{Path.DirectorySeparatorChar}", StringComparison.OrdinalIgnoreCase) ||
            path.Contains($"{Path.DirectorySeparatorChar}obj{Path.DirectorySeparatorChar}", StringComparison.OrdinalIgnoreCase) ||
            path.Contains($"{Path.DirectorySeparatorChar}Build{Path.DirectorySeparatorChar}", StringComparison.OrdinalIgnoreCase) ||
            path.Contains($"{Path.DirectorySeparatorChar}.git{Path.DirectorySeparatorChar}", StringComparison.OrdinalIgnoreCase);

        public void Dispose()
        {
            _watcher.Dispose();
            _workspace.Dispose();
        }
    }
}
