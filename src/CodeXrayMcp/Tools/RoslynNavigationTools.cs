using System.ComponentModel;
using System.Text.Json;
using CodeXrayMcp.Services;
using CodeXrayMcp.Workspace;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.FindSymbols;
using ModelContextProtocol.Server;

namespace CodeXrayMcp.Tools;

[McpServerToolType]
public sealed class RoslynNavigationTools(SolutionWorkspaceCache cache, XrayIndexService indexService)
{
    private static readonly JsonSerializerOptions JsonOptions = new() { WriteIndented = true };

    [McpServerTool(Name = "get_type_overview")]
    [Description("Gets a unified symbol/type/component/module overview (partial declarations, base types, and member signatures). Queries multi-language .xray/index.db across C#, XAML, Rust, TS/JS/Svelte, and Python first; uses Roslyn MSBuildWorkspace when loaded or preferSemantic is true on .NET solutions.")]
    public async Task<string> GetTypeOverviewAsync(
        [Description("Absolute path to the repository root directory or .slnx / .sln file.")] string solutionPath,
        [Description("Exact or partial type/struct/component/module name (e.g. 'OrderProcessor', 'OrderPanel', or 'Core.Services.OrderService').")] string typeName,
        [Description("Maximum members to include in response (default 120, up to 500).")] int maxMembers = 120,
        [Description("Force full Roslyn MSBuildWorkspace semantic resolution on .NET solutions (default false).")] bool preferSemantic = false,
        CancellationToken ct = default)
    {
        bool hasSolution = SolutionWorkspaceCache.TryResolveSolutionPath(solutionPath, out _);
        if (!preferSemantic && !cache.TryGetCachedSolution(solutionPath, out _))
        {
            string? fastOverview = await indexService.TryGetTypeOverviewFromDbAsync(solutionPath, typeName, maxMembers, ct);
            if (fastOverview is not null)
                return fastOverview;

            if (!hasSolution)
            {
                return JsonSerializer.Serialize(new
                {
                    found = false,
                    source = "xray-index-sqlite",
                    typeName
                }, JsonOptions);
            }
        }

        if (!hasSolution)
        {
            return JsonSerializer.Serialize(new
            {
                found = false,
                semanticSupported = false,
                typeName,
                reason = "No .slnx or .sln file found; Roslyn semantic analysis applies only to .NET solutions."
            }, JsonOptions);
        }

        return await BuildRoslynTypeOverviewAsync(solutionPath, typeName, maxMembers, ct);
    }

    private async Task<string> BuildRoslynTypeOverviewAsync(string solutionPath, string typeName, int maxMembers, CancellationToken ct)
    {
        var (solution, repoRoot) = await cache.GetSolutionAsync(solutionPath, ct);
        var matches = await RoslynSymbolQueryHelper.FindNamedTypesAsync(solution, typeName, ct);

        if (matches.Count == 0)
            return JsonSerializer.Serialize(new { found = false, typeName }, JsonOptions);

        var target = matches[0];
        var partialFiles = target.DeclaringSyntaxReferences
            .Select(r =>
            {
                var span = r.SyntaxTree.GetLineSpan(r.Span, ct);
                return new
                {
                    file = SolutionWorkspaceCache.MakeRelativePath(repoRoot, r.SyntaxTree.FilePath),
                    startLine = span.StartLinePosition.Line + 1,
                    endLine = span.EndLinePosition.Line + 1,
                    totalFileLines = r.SyntaxTree.GetText(ct).Lines.Count
                };
            })
            .DistinctBy(x => $"{x.file}:{x.startLine}", StringComparer.Ordinal)
            .OrderBy(x => x.file, StringComparer.Ordinal)
            .ToList();

        var allMembers = target.GetMembers()
            .Where(m => !m.IsImplicitlyDeclared && m.Kind is not SymbolKind.NamedType)
            .Where(m => m is not IMethodSymbol ms || ms.MethodKind is MethodKind.Ordinary or MethodKind.Constructor)
            .Select(m => new
            {
                kind = m.Kind.ToString(),
                accessibility = m.DeclaredAccessibility.ToString(),
                isStatic = m.IsStatic,
                signature = m.ToDisplayString(SymbolDisplayFormat.MinimallyQualifiedFormat)
            })
            .ToList();
        var members = allMembers.Take(Math.Clamp(maxMembers, 1, 500)).ToList();

        return JsonSerializer.Serialize(new
        {
            found = true,
            source = "roslyn-semantic",
            fullName = target.ToDisplayString(),
            assembly = target.ContainingAssembly?.Name,
            typeKind = target.TypeKind.ToString(),
            baseType = target.BaseType?.ToDisplayString(),
            interfaces = target.AllInterfaces.Select(i => i.ToDisplayString()).ToList(),
            partialDeclarationCount = partialFiles.Count,
            declarations = partialFiles,
            totalMembers = allMembers.Count,
            memberCount = members.Count,
            members
        }, JsonOptions);
    }

    [McpServerTool(Name = "find_callers")]
    [Description("Finds compiler-verified callers of a C# method, property, or constructor across a .NET solution (including calls via interfaces/virtual dispatch).")]
    public async Task<string> FindCallersAsync(
        [Description("Absolute path to the .slnx / .sln file or .NET repository root directory.")] string solutionPath,
        [Description("Method or property name (e.g. 'SaveAsync' or 'ProcessOrder').")] string symbolName,
        [Description("Optional containing type filter (e.g. 'IOrderRepository' or 'OrderService').")] string? containingType = null,
        [Description("Maximum caller entries to return (default 40).")] int maxResults = 40,
        CancellationToken ct = default)
    {
        if (!SolutionWorkspaceCache.TryResolveSolutionPath(solutionPath, out _))
            return BuildSemanticUnsupportedJson("find_callers", symbolName);

        var (solution, repoRoot) = await cache.GetSolutionAsync(solutionPath, ct);
        var symbols = await RoslynSymbolQueryHelper.ResolveSymbolsAsync(solution, symbolName, containingType, ct);

        if (symbols.Count == 0)
            return JsonSerializer.Serialize(new { found = false, symbolName, containingType }, JsonOptions);

        var callerItems = new List<object>();
        foreach (var sym in symbols.Take(4))
        {
            var callers = await SymbolFinder.FindCallersAsync(sym, solution, ct);
            foreach (var c in callers)
            {
                foreach (var loc in c.Locations.Where(l => l.IsInSource))
                {
                    var span = loc.GetLineSpan();
                    callerItems.Add(new
                    {
                        targetSymbol = sym.ToDisplayString(SymbolDisplayFormat.MinimallyQualifiedFormat),
                        callingSymbol = c.CallingSymbol.ToDisplayString(SymbolDisplayFormat.CSharpShortErrorMessageFormat),
                        assembly = c.CallingSymbol.ContainingAssembly?.Name,
                        isDirect = c.IsDirect,
                        file = SolutionWorkspaceCache.MakeRelativePath(repoRoot, span.Path),
                        line = span.StartLinePosition.Line + 1
                    });
                    if (callerItems.Count >= maxResults)
                        break;
                }
                if (callerItems.Count >= maxResults)
                    break;
            }
        }

        return JsonSerializer.Serialize(new
        {
            found = true,
            matchedTargets = symbols.Take(4).Select(s => s.ToDisplayString()).ToList(),
            callerCount = callerItems.Count,
            callers = callerItems
        }, JsonOptions);
    }

    [McpServerTool(Name = "find_implementations")]
    [Description("Finds all concrete implementations and derived classes of a C# interface, abstract class, or virtual member across a .NET solution.")]
    public async Task<string> FindImplementationsAsync(
        [Description("Absolute path to the .slnx / .sln file or .NET repository root directory.")] string solutionPath,
        [Description("Interface, base class, or member name (e.g. 'IOrderRepository').")] string symbolName,
        [Description("Maximum implementations to return (default 40).")] int maxResults = 40,
        CancellationToken ct = default)
    {
        if (!SolutionWorkspaceCache.TryResolveSolutionPath(solutionPath, out _))
            return BuildSemanticUnsupportedJson("find_implementations", symbolName);

        var (solution, repoRoot) = await cache.GetSolutionAsync(solutionPath, ct);
        var symbols = await RoslynSymbolQueryHelper.ResolveSymbolsAsync(solution, symbolName, null, ct);

        if (symbols.Count == 0)
            return JsonSerializer.Serialize(new { found = false, symbolName }, JsonOptions);

        var results = new List<object>();
        foreach (var sym in symbols.Take(3))
        {
            var impls = await SymbolFinder.FindImplementationsAsync(sym, solution, cancellationToken: ct);
            IEnumerable<ISymbol> combined = impls;
            if (sym is INamedTypeSymbol namedType && namedType.TypeKind == TypeKind.Class)
            {
                var derived = await SymbolFinder.FindDerivedClassesAsync(namedType, solution, cancellationToken: ct);
                combined = combined.Concat(derived);
            }

            foreach (var impl in combined.Distinct(SymbolEqualityComparer.Default).Take(maxResults))
            {
                var loc = impl.Locations.FirstOrDefault(l => l.IsInSource);
                var span = loc?.GetLineSpan();
                results.Add(new
                {
                    contract = sym.ToDisplayString(SymbolDisplayFormat.MinimallyQualifiedFormat),
                    implementation = impl.ToDisplayString(),
                    assembly = impl.ContainingAssembly?.Name,
                    file = span.HasValue ? SolutionWorkspaceCache.MakeRelativePath(repoRoot, span.Value.Path) : "(metadata)",
                    line = span.HasValue ? span.Value.StartLinePosition.Line + 1 : 0
                });
            }
        }

        return JsonSerializer.Serialize(new { found = true, count = results.Count, implementations = results }, JsonOptions);
    }

    [McpServerTool(Name = "get_symbol_blast_radius")]
    [Description("Computes a 2-hop transitive caller and impacted-project blast radius before modifying a C# contract, type, or method.")]
    public async Task<string> GetSymbolBlastRadiusAsync(
        [Description("Absolute path to the .slnx / .sln file or .NET repository root directory.")] string solutionPath,
        [Description("Symbol name to evaluate impact for.")] string symbolName,
        [Description("Optional containing type filter.")] string? containingType = null,
        CancellationToken ct = default)
    {
        if (!SolutionWorkspaceCache.TryResolveSolutionPath(solutionPath, out _))
            return BuildSemanticUnsupportedJson("get_symbol_blast_radius", symbolName);

        var (solution, repoRoot) = await cache.GetSolutionAsync(solutionPath, ct);
        var targets = await RoslynSymbolQueryHelper.ResolveSymbolsAsync(solution, symbolName, containingType, ct);
        if (targets.Count == 0)
            return JsonSerializer.Serialize(new { found = false, symbolName, containingType }, JsonOptions);

        var impactedAssemblies = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var hop1Symbols = new HashSet<ISymbol>(SymbolEqualityComparer.Default);
        var hop1Edges = await RoslynSymbolQueryHelper.CollectHop1CallersAsync(targets, solution, repoRoot, impactedAssemblies, hop1Symbols, ct);
        var hop2Edges = await RoslynSymbolQueryHelper.CollectHop2CallersAsync(hop1Symbols, solution, repoRoot, impactedAssemblies, ct);

        return JsonSerializer.Serialize(new
        {
            found = true,
            targetSymbols = targets.Take(3).Select(s => s.ToDisplayString()).ToList(),
            impactedAssemblies = impactedAssemblies.Order(StringComparer.OrdinalIgnoreCase).ToList(),
            hop1Count = hop1Edges.Count,
            hop1Callers = hop1Edges.Take(30),
            hop2Count = hop2Edges.Count,
            hop2Callers = hop2Edges.Take(30)
        }, JsonOptions);
    }

    private static string BuildSemanticUnsupportedJson(string toolName, string symbolName) =>
        JsonSerializer.Serialize(new
        {
            found = false,
            semanticSupported = false,
            tool = toolName,
            symbolName,
            reason = "No .slnx or .sln file found. Use get_feature_slice, get_architecture_links, or grep_code for Rust, TypeScript, JavaScript, Svelte, and Python repositories."
        }, JsonOptions);
}
