namespace CodeXrayMcp.Services;

public static class ClonePromotionAdvisor
{
    public static (string Target, string Recommendation) Evaluate(
        IReadOnlySet<string> projects,
        string format,
        bool touchesXaml,
        bool isCrossProject,
        bool touchesExternalSharedRoot)
    {
        ArgumentNullException.ThrowIfNull(projects);
        string joined = string.Join(", ", projects.OrderBy(x => x, StringComparer.OrdinalIgnoreCase));

        if (touchesExternalSharedRoot)
        {
            return (
                "External Shared Root",
                $"Matches existing shared library code across [{joined}] — reuse or consolidate directly in the configured shared root.");
        }

        string fmt = (format ?? "csharp").Trim().ToLowerInvariant();
        if (TryEvaluateNonDotNetTarget(fmt, joined, isCrossProject) is { } nonDotNet)
            return nonDotNet;

        bool isUi = touchesXaml || projects.Any(p =>
            p.Contains(".UI", StringComparison.OrdinalIgnoreCase) ||
            p.Contains("Wpf", StringComparison.OrdinalIgnoreCase));

        if (!isCrossProject)
        {
            return (
                touchesXaml ? "Intra-project (ResourceDictionary / DataTemplate)" : "Intra-project (Service / Helper)",
                touchesXaml
                    ? $"Repeated XAML structure inside [{joined}] — extract reusable Style or DataTemplate into Themes."
                    : $"Intra-project structural clone inside [{joined}] — extract dedicated single-responsibility helper or service.");
        }

        return isUi
            ? ("Shared UI Library", $"Cross-project UI/XAML clone across [{joined}] — promote shared control, style, or converter into a shared UI library.")
            : ("Shared Core Library", $"Cross-project C# clone across [{joined}] — promote domain-independent logic into a shared core library.");
    }

    private static (string Target, string Recommendation)? TryEvaluateNonDotNetTarget(string fmt, string joined, bool isCrossProject) => fmt switch
    {
        "rust" => isCrossProject
            ? ("Shared Rust Workspace Crate", $"Cross-crate Rust structural clone across [{joined}] — extract shared utility crate or workspace module.")
            : ("Intra-crate Rust Module", $"Intra-crate Rust structural clone inside [{joined}] — consolidate into a focused Rust module."),
        "typescript" or "javascript" or "svelte" => isCrossProject
            ? ("Shared Web / TS Package", $"Cross-package {fmt} clone across [{joined}] — promote shared helpers or Svelte components to a shared package.")
            : ("Shared Web Module ($lib / utils)", $"Repeated {fmt} block inside [{joined}] — extract reusable module under lib/ or components/."),
        "python" => isCrossProject
            ? ("Shared Python Utility Package", $"Cross-package Python clone across [{joined}] — extract shared Python helper package.")
            : ("Shared Python Module", $"Repeated Python logic inside [{joined}] — extract cohesive shared helper module."),
        _ => null
    };
}

public sealed record EnrichedOccurrence(string Project, string File, int StartLine, int EndLine);

public sealed record EnrichedCloneGroup(
    string? Fingerprint,
    string Format,
    double SimilarityPct,
    bool IsCrossProject,
    int Lines,
    int InstanceCount,
    double ImpactScore,
    string PromotionTarget,
    string Recommendation,
    IReadOnlyList<EnrichedOccurrence> Occurrences);

public sealed record CloneScanResult(
    string Engine,
    IReadOnlyList<string> ScannedRoots,
    int MinLines,
    int TotalCloneGroupsFound,
    int CrossProjectGroups,
    IReadOnlyList<EnrichedCloneGroup> CloneGroups);
