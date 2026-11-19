using System.Globalization;
using System.Text.Json;
using Microsoft.Data.Sqlite;

namespace CodeXrayMcp.Services;

internal static class XrayIndexSqliteQueryService
{
    private static readonly JsonSerializerOptions JsonOptions = new() { WriteIndented = true };

    internal static async Task<string?> QueryTypeOverviewAsync(
        string dbPath,
        string typeName,
        int maxMembers,
        CancellationToken ct)
    {
        if (!File.Exists(dbPath))
            return null;

        await using var conn = new SqliteConnection($"Data Source={dbPath};Mode=ReadOnly");
        await conn.OpenAsync(ct);

        string target = typeName.Trim();
        string? fullName = await ResolveFullTypeNameAsync(conn, target, ct);
        if (fullName is null)
            return null;

        var declarations = await LoadTypeDeclarationsAsync(conn, fullName, ct);
        var (totalMembers, members) = await LoadTypeMembersAsync(conn, fullName, Math.Clamp(maxMembers, 1, 500), ct);
        var baseTypes = declarations.SelectMany(d => d.BaseTypes).Distinct(StringComparer.Ordinal).ToList();

        return JsonSerializer.Serialize(new
        {
            found = true,
            source = "xray-index-sqlite",
            fullName,
            project = declarations.FirstOrDefault()?.Project,
            typeKind = declarations.FirstOrDefault()?.Kind,
            baseTypes,
            partialDeclarationCount = declarations.Count,
            declarations = declarations.Select(d => new { file = d.File, d.StartLine, d.EndLine, totalFileLines = d.LineCount }),
            totalMembers,
            memberCount = members.Count,
            members
        }, JsonOptions);
    }

    internal static async Task<string> QueryPublicSymbolsAsync(
        string dbPath,
        string query,
        string? kind,
        int maxResults,
        CancellationToken ct)
    {
        if (!File.Exists(dbPath))
        {
            return JsonSerializer.Serialize(new
            {
                query,
                kind,
                source = "xray-index-sqlite",
                matchCount = 0,
                reusableApis = Array.Empty<object>()
            }, JsonOptions);
        }

        await using var conn = new SqliteConnection($"Data Source={dbPath};Mode=ReadOnly");
        await conn.OpenAsync(ct);

        string qTrim = (query ?? "*").Trim();
        int matchAll = qTrim == "*" ? 1 : 0;
        string likePat = $"%{qTrim}%";
        int cap = Math.Clamp(maxResults, 1, 100);
        var results = new List<object>();

        if (!string.Equals(kind, "method", StringComparison.OrdinalIgnoreCase))
            await AppendPublicTypeMatchesAsync(conn, matchAll, likePat, cap, results, ct);

        if (results.Count < cap && !string.Equals(kind, "type", StringComparison.OrdinalIgnoreCase))
            await AppendPublicMemberMatchesAsync(conn, matchAll, likePat, cap - results.Count, results, ct);

        return JsonSerializer.Serialize(new
        {
            query,
            kind,
            source = "xray-index-sqlite",
            matchCount = results.Count,
            reusableApis = results
        }, JsonOptions);
    }

    private static async Task AppendPublicTypeMatchesAsync(
        SqliteConnection conn, int matchAll, string likePat, int cap, List<object> results, CancellationToken ct)
    {
        await using var cmd = conn.CreateCommand();
        cmd.CommandText = """
            SELECT t.kind, f.project_name, t.full_name, t.base_types, f.rel_path, t.start_line
            FROM types t JOIN files f ON f.id = t.file_id
            WHERE t.accessibility = 'public' AND t.kind != 'xaml_view'
              AND ($matchAll = 1 OR LOWER(t.full_name) LIKE LOWER($pat) OR LOWER(t.name) LIKE LOWER($pat))
            ORDER BY t.full_name LIMIT $cap
            """;
        cmd.Parameters.AddWithValue("$matchAll", matchAll);
        cmd.Parameters.AddWithValue("$pat", likePat);
        cmd.Parameters.AddWithValue("$cap", cap);

        await using var r = await cmd.ExecuteReaderAsync(ct);
        while (await r.ReadAsync(ct))
        {
            results.Add(new
            {
                symbolKind = r.GetString(0),
                project = r.GetString(1),
                signature = r.GetString(2),
                baseTypes = r.GetString(3),
                file = r.GetString(4),
                line = r.GetInt32(5)
            });
        }
    }

    private static async Task AppendPublicMemberMatchesAsync(
        SqliteConnection conn, int matchAll, string likePat, int cap, List<object> results, CancellationToken ct)
    {
        await using var cmd = conn.CreateCommand();
        cmd.CommandText = """
            SELECT m.kind, f.project_name, m.owner_type, m.signature, f.rel_path, m.start_line
            FROM members m JOIN files f ON f.id = m.file_id
            WHERE m.kind IN ('function','method')
              AND ($matchAll = 1 OR LOWER(m.name) LIKE LOWER($pat) OR LOWER(m.signature) LIKE LOWER($pat))
            ORDER BY m.owner_type, m.start_line LIMIT $cap
            """;
        cmd.Parameters.AddWithValue("$matchAll", matchAll);
        cmd.Parameters.AddWithValue("$pat", likePat);
        cmd.Parameters.AddWithValue("$cap", cap);

        await using var r = await cmd.ExecuteReaderAsync(ct);
        while (await r.ReadAsync(ct))
        {
            results.Add(new
            {
                symbolKind = r.GetString(0),
                project = r.GetString(1),
                owner = r.GetString(2),
                signature = r.GetString(3),
                file = r.GetString(4),
                line = r.GetInt32(5)
            });
        }
    }

    private static async Task<string?> ResolveFullTypeNameAsync(SqliteConnection conn, string query, CancellationToken ct)
    {
        await using var cmd = conn.CreateCommand();
        cmd.CommandText = """
            SELECT full_name FROM types
            WHERE kind != 'xaml_view' AND (LOWER(full_name) = LOWER($q) OR LOWER(name) = LOWER($q) OR LOWER(full_name) LIKE LOWER($like))
            ORDER BY CASE WHEN LOWER(full_name) = LOWER($q) THEN 0 WHEN LOWER(name) = LOWER($q) THEN 1 ELSE 2 END, LENGTH(full_name)
            LIMIT 1
            """;
        cmd.Parameters.AddWithValue("$q", query);
        cmd.Parameters.AddWithValue("$like", $"%{query}%");
        object? scalar = await cmd.ExecuteScalarAsync(ct);
        return scalar as string;
    }

    private static async Task<List<TypeDeclRow>> LoadTypeDeclarationsAsync(SqliteConnection conn, string fullName, CancellationToken ct)
    {
        await using var cmd = conn.CreateCommand();
        cmd.CommandText = """
            SELECT t.kind, t.base_types, f.project_name, f.rel_path, f.line_count, t.start_line, t.end_line
            FROM types t JOIN files f ON f.id = t.file_id
            WHERE t.full_name = $fn AND t.kind != 'xaml_view'
            ORDER BY f.rel_path, t.start_line
            """;
        cmd.Parameters.AddWithValue("$fn", fullName);
        var list = new List<TypeDeclRow>();
        await using var r = await cmd.ExecuteReaderAsync(ct);
        while (await r.ReadAsync(ct))
        {
            string basesRaw = r.GetString(1);
            string[] bases = string.IsNullOrWhiteSpace(basesRaw)
                ? []
                : basesRaw.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
            list.Add(new TypeDeclRow(r.GetString(0), bases, r.GetString(2), r.GetString(3), r.GetInt32(4), r.GetInt32(5), r.GetInt32(6)));
        }
        return list;
    }

    private static async Task<(int Total, List<object> Items)> LoadTypeMembersAsync(
        SqliteConnection conn, string fullName, int cap, CancellationToken ct)
    {
        await using var countCmd = conn.CreateCommand();
        countCmd.CommandText = "SELECT COUNT(*) FROM members WHERE owner_type = $fn";
        countCmd.Parameters.AddWithValue("$fn", fullName);
        int total = Convert.ToInt32(await countCmd.ExecuteScalarAsync(ct) ?? 0, CultureInfo.InvariantCulture);

        await using var cmd = conn.CreateCommand();
        cmd.CommandText = "SELECT kind, signature, return_type, start_line FROM members WHERE owner_type = $fn ORDER BY start_line LIMIT $cap";
        cmd.Parameters.AddWithValue("$fn", fullName);
        cmd.Parameters.AddWithValue("$cap", cap);
        var items = new List<object>();
        await using var r = await cmd.ExecuteReaderAsync(ct);
        while (await r.ReadAsync(ct))
            items.Add(new { kind = r.GetString(0), signature = r.GetString(1), returnType = r.GetString(2), line = r.GetInt32(3) });
        return (total, items);
    }

    private sealed record TypeDeclRow(string Kind, string[] BaseTypes, string Project, string File, int LineCount, int StartLine, int EndLine);
}
