using System.Text.Json;
using CodeXrayMcp.Workspace;

namespace CodeXrayMcp.Services;

internal static class CloneReportEnricher
{
    public static CloneScanResult ParseAndEnrich(
        JsonElement rootEl,
        string repoRoot,
        IReadOnlyList<string> scanRoots,
        int minLines,
        string? pathFilter,
        int maxGroups)
    {
        string normRepoRoot = NormalizeSlash(repoRoot).TrimEnd('/') + "/";
        var enriched = new List<EnrichedCloneGroup>();

        if (rootEl.TryGetProperty("cloneGroups", out var groupsEl) && groupsEl.ValueKind == JsonValueKind.Array)
        {
            using var groupEnum = groupsEl.EnumerateArray();
            while (groupEnum.MoveNext())
            {
                var parsed = TryBuildCloneGroup(groupEnum.Current, repoRoot, normRepoRoot, pathFilter);
                if (parsed is not null)
                    enriched.Add(parsed);
            }
        }

        var ranked = enriched
            .OrderByDescending(x => x.ImpactScore)
            .Take(Math.Max(1, maxGroups))
            .ToList();

        return new CloneScanResult(
            Engine: "structural-clone (Rust tree-sitter + Rabin-Karp)",
            ScannedRoots: scanRoots,
            MinLines: minLines,
            TotalCloneGroupsFound: ranked.Count,
            CrossProjectGroups: ranked.Count(x => x.IsCrossProject),
            CloneGroups: ranked);
    }

    private static EnrichedCloneGroup? TryBuildCloneGroup(
        JsonElement g,
        string repoRoot,
        string normRepoRoot,
        string? pathFilter)
    {
        string format = g.TryGetProperty("format", out var fmt) ? (fmt.GetString() ?? "csharp") : "csharp";
        double similarity = g.TryGetProperty("similarity", out var sim) ? sim.GetDouble() : 100.0;
        string? fingerprint = g.TryGetProperty("fingerprint", out var fp) ? fp.GetString() : null;

        var parsedOccs = ParseOccurrences(g, repoRoot, normRepoRoot, string.Equals(format, "xaml", StringComparison.OrdinalIgnoreCase));
        if (!parsedOccs.TouchesActiveRepo || parsedOccs.Occurrences.Count < 2)
            return null;

        if (!MatchesPathFilter(parsedOccs.Occurrences, pathFilter))
            return null;

        bool isCrossProject = parsedOccs.Projects.Count > 1;
        var (target, recommendation) = ClonePromotionAdvisor.Evaluate(
            parsedOccs.Projects,
            format,
            parsedOccs.TouchesXaml,
            isCrossProject,
            parsedOccs.TouchesExternalSharedRoot);

        double impact = Math.Round(
            parsedOccs.MaxSpanLines * parsedOccs.Occurrences.Count * (similarity / 100.0)
            * (isCrossProject ? 2.0 : 1.0)
            * (parsedOccs.TouchesExternalSharedRoot ? 1.4 : 1.0),
            1);

        var sampledOccurrences = parsedOccs.Occurrences
            .OrderBy(o => o.File.StartsWith("src/", StringComparison.OrdinalIgnoreCase) ? 0 : 1)
            .ThenBy(o => o.Project, StringComparer.OrdinalIgnoreCase)
            .ThenBy(o => o.StartLine)
            .Take(10)
            .ToList();

        return new EnrichedCloneGroup(
            fingerprint,
            format,
            similarity,
            isCrossProject,
            parsedOccs.MaxSpanLines,
            parsedOccs.Occurrences.Count,
            impact,
            target,
            recommendation,
            sampledOccurrences);
    }

    private static bool MatchesPathFilter(List<EnrichedOccurrence> occurrences, string? pathFilter)
    {
        if (string.IsNullOrWhiteSpace(pathFilter))
            return true;

        return occurrences.Any(o =>
            o.File.Contains(pathFilter, StringComparison.OrdinalIgnoreCase) ||
            o.Project.Contains(pathFilter, StringComparison.OrdinalIgnoreCase));
    }

    private static ParsedOccurrenceBatch ParseOccurrences(
        JsonElement groupEl,
        string repoRoot,
        string normRepoRoot,
        bool initialTouchesXaml)
    {
        var acc = new OccurrenceAccumulator(initialTouchesXaml);

        if (!groupEl.TryGetProperty("occurrences", out var occs) || occs.ValueKind != JsonValueKind.Array)
            return acc.Build();

        using var occEnum = occs.EnumerateArray();
        while (occEnum.MoveNext())
            acc.Add(occEnum.Current, repoRoot, normRepoRoot);

        return acc.Build();
    }

    private static string NormalizeSlash(string path) => path.Replace('\\', '/');

    private sealed class OccurrenceAccumulator(bool initialTouchesXaml)
    {
        private readonly List<EnrichedOccurrence> _occurrences = [];
        private readonly HashSet<string> _projects = new(StringComparer.OrdinalIgnoreCase);
        private bool _touchesActiveRepo;
        private bool _touchesExternalSharedRoot;
        private bool _touchesXaml = initialTouchesXaml;
        private int _maxSpanLines;

        public void Add(JsonElement occ, string repoRoot, string normRepoRoot)
        {
            string rawPath = occ.TryGetProperty("path", out var p) ? (p.GetString() ?? "") : "";
            string normPath = NormalizeSlash(rawPath);
            int startLine = occ.TryGetProperty("startLine", out var sl) ? sl.GetInt32() : 0;
            int endLine = occ.TryGetProperty("endLine", out var el) ? el.GetInt32() : startLine;
            _maxSpanLines = Math.Max(_maxSpanLines, Math.Max(1, endLine - startLine + 1));

            bool inActiveRepo = normPath.StartsWith(normRepoRoot, StringComparison.OrdinalIgnoreCase);
            _touchesActiveRepo |= inActiveRepo;
            if (!inActiveRepo)
                _touchesExternalSharedRoot = true;

            string displayFile = inActiveRepo
                ? normPath[normRepoRoot.Length..]
                : SolutionWorkspaceCache.MakeRelativePath(repoRoot, rawPath);

            string project = occ.TryGetProperty("projectHint", out var ph) && !string.IsNullOrWhiteSpace(ph.GetString())
                ? ph.GetString()!
                : GuessProjectName(rawPath);
            _projects.Add(project);

            if (normPath.EndsWith(".xaml", StringComparison.OrdinalIgnoreCase))
                _touchesXaml = true;

            _occurrences.Add(new EnrichedOccurrence(project, displayFile, startLine, endLine));
        }

        public ParsedOccurrenceBatch Build() =>
            new(_occurrences, _projects, _touchesActiveRepo, _touchesExternalSharedRoot, _touchesXaml, _maxSpanLines);

        private static string GuessProjectName(string path)
        {
            try
            {
                var dir = new DirectoryInfo(Path.GetDirectoryName(path) ?? path);
                while (dir is not null)
                {
                    if (File.Exists(Path.Combine(dir.FullName, "Cargo.toml"))
                        || File.Exists(Path.Combine(dir.FullName, "package.json"))
                        || File.Exists(Path.Combine(dir.FullName, "pyproject.toml")))
                    {
                        return dir.Name;
                    }

                    var csproj = dir.EnumerateFiles("*.csproj")
                        .FirstOrDefault(f => !f.Name.Contains("_wpftmp", StringComparison.OrdinalIgnoreCase));
                    if (csproj is not null)
                        return Path.GetFileNameWithoutExtension(csproj.Name);

                    dir = dir.Parent;
                }
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
                // Ignore transient inaccessible directories during project hint lookup.
            }
            return Path.GetFileName(Path.GetDirectoryName(path) ?? "Unknown") ?? "Unknown";
        }
    }

    private sealed record ParsedOccurrenceBatch(
        List<EnrichedOccurrence> Occurrences,
        HashSet<string> Projects,
        bool TouchesActiveRepo,
        bool TouchesExternalSharedRoot,
        bool TouchesXaml,
        int MaxSpanLines);
}
