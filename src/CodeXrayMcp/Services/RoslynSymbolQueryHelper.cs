using CodeXrayMcp.Workspace;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.FindSymbols;

namespace CodeXrayMcp.Services;

internal static class RoslynSymbolQueryHelper
{
    public static async Task<List<object>> CollectHop1CallersAsync(
        List<ISymbol> targets,
        Solution solution,
        string repoRoot,
        HashSet<string> impactedAssemblies,
        HashSet<ISymbol> hop1Symbols,
        CancellationToken ct)
    {
        var hop1Edges = new List<object>();
        foreach (var t in targets.Take(3))
        {
            var c1 = await SymbolFinder.FindCallersAsync(t, solution, ct);
            foreach (var caller in c1)
            {
                if (caller.CallingSymbol.ContainingAssembly?.Name is string asm1)
                    impactedAssemblies.Add(asm1);
                hop1Symbols.Add(caller.CallingSymbol);
                var loc = caller.Locations.FirstOrDefault(l => l.IsInSource)?.GetLineSpan();
                hop1Edges.Add(new
                {
                    symbol = caller.CallingSymbol.ToDisplayString(SymbolDisplayFormat.CSharpShortErrorMessageFormat),
                    assembly = caller.CallingSymbol.ContainingAssembly?.Name,
                    file = loc.HasValue ? SolutionWorkspaceCache.MakeRelativePath(repoRoot, loc.Value.Path) : "",
                    line = loc.HasValue ? loc.Value.StartLinePosition.Line + 1 : 0
                });
            }
        }
        return hop1Edges;
    }

    public static async Task<List<object>> CollectHop2CallersAsync(
        HashSet<ISymbol> hop1Symbols,
        Solution solution,
        string repoRoot,
        HashSet<string> impactedAssemblies,
        CancellationToken ct)
    {
        var hop2Edges = new List<object>();
        foreach (var h1 in hop1Symbols.Take(15))
        {
            var c2 = await SymbolFinder.FindCallersAsync(h1, solution, ct);
            foreach (var caller2 in c2.Take(10))
            {
                if (caller2.CallingSymbol.ContainingAssembly?.Name is string asm2)
                    impactedAssemblies.Add(asm2);
                var loc = caller2.Locations.FirstOrDefault(l => l.IsInSource)?.GetLineSpan();
                hop2Edges.Add(new
                {
                    via = h1.Name,
                    symbol = caller2.CallingSymbol.ToDisplayString(SymbolDisplayFormat.CSharpShortErrorMessageFormat),
                    assembly = caller2.CallingSymbol.ContainingAssembly?.Name,
                    file = loc.HasValue ? SolutionWorkspaceCache.MakeRelativePath(repoRoot, loc.Value.Path) : "",
                    line = loc.HasValue ? loc.Value.StartLinePosition.Line + 1 : 0
                });
            }
        }
        return hop2Edges;
    }

    public static async Task<List<INamedTypeSymbol>> FindNamedTypesAsync(Solution solution, string typeName, CancellationToken ct)
    {
        bool isQualified = typeName.Contains('.', StringComparison.Ordinal);
        string shortName = isQualified ? typeName[(typeName.LastIndexOf('.') + 1)..] : typeName;
        var results = new List<INamedTypeSymbol>();

        foreach (var project in solution.Projects)
        {
            var decls = await SymbolFinder.FindSourceDeclarationsAsync(project, shortName, ignoreCase: true, SymbolFilter.Type, ct);
            results.AddRange(decls.OfType<INamedTypeSymbol>()
                .Where(sym => !isQualified || sym.ToDisplayString().EndsWith(typeName, StringComparison.OrdinalIgnoreCase)));
        }

        return results.Distinct(SymbolEqualityComparer.Default).Cast<INamedTypeSymbol>().ToList();
    }

    public static async Task<List<ISymbol>> ResolveSymbolsAsync(Solution solution, string symbolName, string? containingType, CancellationToken ct)
    {
        var results = new List<ISymbol>();
        foreach (var project in solution.Projects)
        {
            var decls = await SymbolFinder.FindSourceDeclarationsAsync(project, symbolName, ignoreCase: true, SymbolFilter.TypeAndMember, ct);
            foreach (var sym in decls)
            {
                if (!string.IsNullOrWhiteSpace(containingType) &&
                    (sym.ContainingType is null || !sym.ContainingType.ToDisplayString().Contains(containingType, StringComparison.OrdinalIgnoreCase)))
                {
                    continue;
                }
                results.Add(sym);
            }
        }

        return results.Distinct(SymbolEqualityComparer.Default).ToList();
    }
}
