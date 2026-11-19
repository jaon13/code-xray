using System.Collections.Concurrent;
using CodeXrayMcp.Workspace;
using Microsoft.CodeAnalysis;

namespace CodeXrayMcp.Services;

public sealed record ReusableCandidate(
    string SymbolKind,
    string ShortName,
    string Signature,
    string FullText,
    string Assembly,
    bool IsSharedLibrary,
    string? BaseType,
    IReadOnlyList<string> Interfaces,
    string? ReturnType,
    bool IsConverter,
    bool IsExtensionMethod,
    string File);

public sealed class ReusableApiIndexCache(SolutionWorkspaceCache workspaceCache) : IDisposable
{
    private static readonly string[] IgnoredPrefixes =
    [
        "System", "Microsoft", "mscorlib", "netstandard", "WindowsBase", "WindowsFormsIntegration",
        "PresentationCore", "PresentationFramework", "PresentationUI", "ReachFramework", "Accessibility",
        "UIAutomation", "Interop.UIAutomation", "DirectWriteForwarder", "WinRT",
        "xunit", "FsCheck", "ArchUnitNET", "Verify", "Google.Protobuf", "YamlDotNet", "FlaUI",
        "Dapper", "Grpc", "MySqlConnector", "Npgsql", "SQLitePCLRaw", "Serilog", "Tomlyn", "ZstdSharp",
        "CommunityToolkit"
    ];

    private readonly SemaphoreSlim _gate = new(1, 1);
    private readonly ConcurrentDictionary<string, CachedCatalog> _catalogs = new(StringComparer.OrdinalIgnoreCase);

    public sealed record CachedCatalog(VersionStamp Version, string RepoRoot, IReadOnlyList<string> ScannedAssemblies, IReadOnlyList<ReusableCandidate> Candidates);

    public async Task<(IReadOnlyList<string> ScannedAssemblies, List<object> Matches)> SearchAsync(
        string solutionPath,
        string query,
        string? kind,
        string? implementsOrReturnsType,
        bool includeReferencedAssemblies,
        int maxResults,
        CancellationToken ct)
    {
        var catalog = await GetOrBuildCatalogAsync(solutionPath, includeReferencedAssemblies, ct);
        string[] terms = query == "*"
            ? []
            : query.Split(' ', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);

        var scored = new List<(int Score, object Payload)>();
        foreach (var item in catalog.Candidates)
        {
            if (!MatchesKind(item, kind) || !MatchesTypeFilter(item, implementsOrReturnsType))
                continue;

            int score = ScoreCandidate(item, terms);
            if (score <= 0)
                continue;

            object payload = item.SymbolKind is "Method" or "ExtensionMethod"
                ? new { symbolKind = item.SymbolKind, assembly = item.Assembly, isSharedLibrary = item.IsSharedLibrary, signature = item.Signature, file = item.File }
                : new { symbolKind = item.SymbolKind, assembly = item.Assembly, isSharedLibrary = item.IsSharedLibrary, signature = item.Signature, baseType = item.BaseType, interfaces = item.Interfaces, file = item.File };
            scored.Add((score, payload));
        }

        var matches = scored.OrderByDescending(s => s.Score).Take(Math.Clamp(maxResults, 1, 100)).Select(s => s.Payload).ToList();
        return (catalog.ScannedAssemblies, matches);
    }

    private async Task<CachedCatalog> GetOrBuildCatalogAsync(string solutionPath, bool includeReferencedAssemblies, CancellationToken ct)
    {
        var (solution, repoRoot) = await workspaceCache.GetSolutionAsync(solutionPath, ct);
        string key = $"{repoRoot}::{includeReferencedAssemblies}";
        if (_catalogs.TryGetValue(key, out var cached) && cached.Version == solution.Version)
            return cached;

        await _gate.WaitAsync(ct);
        try
        {
            if (_catalogs.TryGetValue(key, out cached) && cached.Version == solution.Version)
                return cached;

            var assemblies = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            var list = new List<ReusableCandidate>();

            foreach (var project in solution.Projects)
                await PopulateProjectCatalogAsync(project, includeReferencedAssemblies, repoRoot, assemblies, list, ct);

            var built = new CachedCatalog(solution.Version, repoRoot, [.. assemblies.Order(StringComparer.OrdinalIgnoreCase)], list);
            _catalogs[key] = built;
            return built;
        }
        finally
        {
            _gate.Release();
        }
    }

    private static async Task PopulateProjectCatalogAsync(
        Project project,
        bool includeReferencedAssemblies,
        string repoRoot,
        HashSet<string> assemblies,
        List<ReusableCandidate> list,
        CancellationToken ct)
    {
        if (project.Name.EndsWith(".Tests", StringComparison.OrdinalIgnoreCase))
            return;

        var compilation = await project.GetCompilationAsync(ct);
        if (compilation is null)
            return;

        string asmName = compilation.AssemblyName ?? project.Name;
        if (assemblies.Add(asmName))
            CollectFromNamespace(compilation.GlobalNamespace, asmName, false, repoRoot, list);

        if (!includeReferencedAssemblies)
            return;

        foreach (var reference in compilation.References)
        {
            if (compilation.GetAssemblyOrModuleSymbol(reference) is not IAssemblySymbol refAsm)
                continue;
            if (IsIgnored(refAsm.Name) || !assemblies.Add(refAsm.Name))
                continue;
            CollectFromNamespace(refAsm.GlobalNamespace, refAsm.Name, true, repoRoot, list);
        }
    }

    private static void CollectFromNamespace(INamespaceSymbol ns, string asm, bool isShared, string repoRoot, List<ReusableCandidate> output)
    {
        foreach (var childNs in ns.GetNamespaceMembers())
            CollectFromNamespace(childNs, asm, isShared, repoRoot, output);

        foreach (var type in ns.GetTypeMembers())
            CollectTypeAndMethods(type, asm, isShared, repoRoot, output);
    }

    private static void CollectTypeAndMethods(INamedTypeSymbol type, string asm, bool isShared, string repoRoot, List<ReusableCandidate> output)
    {
        if (type.DeclaredAccessibility != Accessibility.Public && isShared) return;
        if (type.DeclaredAccessibility is not (Accessibility.Public or Accessibility.Internal)) return;
        if (type.Name.StartsWith('<')) return;

        bool isConverter = type.AllInterfaces.Any(i => i.Name.Contains("ValueConverter", StringComparison.OrdinalIgnoreCase));
        string fullType = type.ToDisplayString();
        var loc = type.Locations.FirstOrDefault(l => l.IsInSource)?.GetLineSpan();
        string file = loc.HasValue ? SolutionWorkspaceCache.MakeRelativePath(repoRoot, loc.Value.Path) : "(compiled-assembly)";
        var ifaces = type.AllInterfaces.Select(i => i.ToDisplayString()).ToList();

        output.Add(new ReusableCandidate(
            isConverter ? "ValueConverter" : type.TypeKind.ToString(),
            type.Name,
            fullType,
            fullType,
            asm,
            isShared,
            type.BaseType?.ToDisplayString(),
            [.. ifaces.Take(5)],
            null,
            isConverter,
            false,
            file));

        CollectPublicMethods(type, asm, isShared, repoRoot, output);
    }

    private static void CollectPublicMethods(INamedTypeSymbol type, string asm, bool isShared, string repoRoot, List<ReusableCandidate> output)
    {
        foreach (var method in type.GetMembers().OfType<IMethodSymbol>())
        {
            if (method.MethodKind != MethodKind.Ordinary || method.DeclaredAccessibility != Accessibility.Public)
                continue;
            string minSig = $"{type.Name}.{method.ToDisplayString(SymbolDisplayFormat.MinimallyQualifiedFormat)}";
            string fqSig = $"{type.ContainingNamespace}.{minSig}";
            var mLoc = method.Locations.FirstOrDefault(l => l.IsInSource)?.GetLineSpan();
            output.Add(new ReusableCandidate(
                method.IsExtensionMethod ? "ExtensionMethod" : "Method",
                method.Name,
                fqSig,
                minSig,
                asm,
                isShared,
                null,
                [],
                method.ReturnType.ToDisplayString(),
                false,
                method.IsExtensionMethod,
                mLoc.HasValue ? SolutionWorkspaceCache.MakeRelativePath(repoRoot, mLoc.Value.Path) : "(compiled-assembly)"));
        }
    }

    public void Dispose() => _gate.Dispose();

    private static bool MatchesKind(ReusableCandidate c, string? kind) => kind switch
    {
        null or "" => true,
        "type" => c.SymbolKind is not ("Method" or "ExtensionMethod"),
        "converter" => c.IsConverter,
        "extension" => c.IsExtensionMethod,
        "method" => c.SymbolKind is "Method" or "ExtensionMethod",
        _ => true
    };

    private static bool MatchesTypeFilter(ReusableCandidate c, string? typeFilter)
    {
        if (string.IsNullOrWhiteSpace(typeFilter))
            return true;
        if (c.SymbolKind is "Method" or "ExtensionMethod")
            return c.ReturnType?.Contains(typeFilter, StringComparison.OrdinalIgnoreCase) == true;
        return c.BaseType?.Contains(typeFilter, StringComparison.OrdinalIgnoreCase) == true ||
               c.Interfaces.Any(i => i.Contains(typeFilter, StringComparison.OrdinalIgnoreCase));
    }

    private static int ScoreCandidate(ReusableCandidate c, string[] terms)
    {
        if (terms.Length == 0)
            return c.IsSharedLibrary ? 20 : 10;

        int score = 0;
        foreach (string t in terms)
        {
            if (c.ShortName.Equals(t, StringComparison.OrdinalIgnoreCase)) score += 40;
            else if (c.ShortName.Contains(t, StringComparison.OrdinalIgnoreCase)) score += 25;
            else if (c.FullText.Contains(t, StringComparison.OrdinalIgnoreCase) || c.Signature.Contains(t, StringComparison.OrdinalIgnoreCase)) score += 10;
            else return 0;
        }
        return score + (c.IsSharedLibrary ? 8 : 0) + (c.IsExtensionMethod ? 5 : 0);
    }

    private static bool IsIgnored(string assemblyName) =>
        IgnoredPrefixes.Any(p => assemblyName.StartsWith(p, StringComparison.OrdinalIgnoreCase));
}
