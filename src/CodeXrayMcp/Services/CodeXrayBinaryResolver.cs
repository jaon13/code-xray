using System.Text.Json;

namespace CodeXrayMcp.Services;

/// <summary>
/// Resolves native CodeXray CLI executables across env overrides, ~/.agents/tooling.json,
/// deployment-relative paths, user bin fallbacks, and PATH.
/// </summary>
internal static class CodeXrayBinaryResolver
{
    public static string ExecutableName(string stem) =>
        OperatingSystem.IsWindows() ? $"{stem}.exe" : stem;

    public static string? TryResolveExecutable(string stem, string specificEnvVar)
    {
        string? explicitPath = Environment.GetEnvironmentVariable(specificEnvVar);
        if (!string.IsNullOrWhiteSpace(explicitPath) && File.Exists(explicitPath))
            return Path.GetFullPath(explicitPath);

        string binName = ExecutableName(stem);
        string? envHomeMatch = TryResolveUnderRoot(Environment.GetEnvironmentVariable("CODEXRAY_HOME"), binName);
        if (envHomeMatch is not null)
            return envHomeMatch;

        string? toolingMatch = TryResolveUnderRoot(ReadCodeXrayRootFromToolingJson(), binName);
        if (toolingMatch is not null)
            return toolingMatch;

        string? baseDirMatch = TryResolveRelativeToBaseDir(binName);
        if (baseDirMatch is not null)
            return baseDirMatch;

        string? userBinMatch = TryResolveUnderUserBins(binName);
        if (userBinMatch is not null)
            return userBinMatch;

        return TryResolveOnPath(binName);
    }

    public static string BuildMissingExecutableMessage(string stem, string specificEnvVar) =>
        $"{ExecutableName(stem)} not found. Set {specificEnvVar} or CODEXRAY_HOME, configure ~/.agents/tooling.json, " +
        "or build via: python scripts/publish_tools.py --register-mcp (or cargo build --release inside src/code-clone-xray).";

    public static string? ResolveToolingJsonPath()
    {
        string? toolingPath = Environment.GetEnvironmentVariable("TOOLING_JSON");
        if (!string.IsNullOrWhiteSpace(toolingPath))
            return toolingPath;

        string userProfile = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        return string.IsNullOrWhiteSpace(userProfile)
            ? null
            : Path.Combine(userProfile, ".agents", "tooling.json");
    }

    private static string? TryResolveUnderRoot(string? root, string binName)
    {
        if (string.IsNullOrWhiteSpace(root))
            return null;

        string[] candidates =
        [
            Path.Combine(root.Trim(), "bin", binName),
            Path.Combine(root.Trim(), binName)
        ];

        foreach (string candidate in candidates)
        {
            string full = Path.GetFullPath(candidate);
            if (File.Exists(full))
                return full;
        }
        return null;
    }

    private static string? TryResolveRelativeToBaseDir(string binName)
    {
        string baseDir = AppContext.BaseDirectory;
        string[] relativeCandidates =
        [
            Path.Combine(baseDir, binName),
            Path.Combine(baseDir, "..", binName),
            Path.Combine(baseDir, "..", "..", "bin", binName),
            Path.Combine(baseDir, "..", "..", "..", "..", "bin", binName),
            Path.Combine(baseDir, "..", "..", "..", "..", "..", "bin", binName),
            Path.Combine(baseDir, "..", "..", "..", "..", "code-clone-xray", "target", "release", binName),
            Path.Combine(baseDir, "..", "..", "..", "..", "..", "src", "code-clone-xray", "target", "release", binName)
        ];

        foreach (string candidate in relativeCandidates)
        {
            string full = Path.GetFullPath(candidate);
            if (File.Exists(full))
                return full;
        }
        return null;
    }

    private static string? TryResolveUnderUserBins(string binName)
    {
        string userProfile = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        if (string.IsNullOrWhiteSpace(userProfile))
            return null;

        string[] dirs =
        [
            Path.Combine(userProfile, ".agents", "bin"),
            Path.Combine(userProfile, ".local", "bin")
        ];
        foreach (string dir in dirs)
        {
            string candidate = Path.Combine(dir, binName);
            if (File.Exists(candidate))
                return candidate;
        }
        return null;
    }

    private static string? TryResolveOnPath(string binName)
    {
        string? pathEnv = Environment.GetEnvironmentVariable("PATH");
        if (string.IsNullOrEmpty(pathEnv))
            return null;

        foreach (string dir in pathEnv.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries))
        {
            string candidate = Path.Combine(dir.Trim(), binName);
            if (File.Exists(candidate))
                return candidate;
        }
        return null;
    }

    private static string? ReadCodeXrayRootFromToolingJson()
    {
        string? toolingPath = ResolveToolingJsonPath();
        if (string.IsNullOrWhiteSpace(toolingPath) || !File.Exists(toolingPath))
            return null;

        try
        {
            using FileStream stream = File.OpenRead(toolingPath);
            using JsonDocument doc = JsonDocument.Parse(stream);
            JsonElement root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object)
                return null;

            if (root.TryGetProperty("codeXrayRoot", out JsonElement cxProp) &&
                cxProp.ValueKind == JsonValueKind.String &&
                !string.IsNullOrWhiteSpace(cxProp.GetString()))
            {
                return cxProp.GetString();
            }

            if (root.TryGetProperty("agentToolingRoot", out JsonElement atProp) &&
                atProp.ValueKind == JsonValueKind.String &&
                !string.IsNullOrWhiteSpace(atProp.GetString()))
            {
                string sub = Path.Combine(atProp.GetString()!, "01.CodeXray");
                if (Directory.Exists(sub))
                    return sub;
            }
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or JsonException)
        {
            return null;
        }

        return null;
    }
}
