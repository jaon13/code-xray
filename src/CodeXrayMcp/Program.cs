using CodeXrayMcp.Services;
using CodeXrayMcp.Tools;
using CodeXrayMcp.Workspace;
using Microsoft.Build.Locator;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Hosting;
using Microsoft.Extensions.Logging;

if (!MSBuildLocator.IsRegistered)
{
    MSBuildLocator.RegisterDefaults();
}

await RunServerAsync(args);

static async Task RunServerAsync(string[] args)
{
    var builder = Host.CreateApplicationBuilder(args);

    // MCP stdio requires stdout to remain strictly pure JSON-RPC frames
    builder.Logging.ClearProviders();
    builder.Logging.AddConsole(options =>
    {
        options.LogToStandardErrorThreshold = LogLevel.Trace;
    });
    builder.Logging.SetMinimumLevel(LogLevel.Warning);

    builder.Services.AddSingleton<SolutionWorkspaceCache>();
    builder.Services.AddSingleton<XrayIndexService>();
    builder.Services.AddSingleton<ReusableApiIndexCache>();
    builder.Services
        .AddMcpServer()
        .WithStdioServerTransport()
        .WithTools<RoslynNavigationTools>()
        .WithTools<RoslynReuseTools>()
        .WithTools<XrayIndexTools>()
        .WithTools<XrayGrepTools>();

    using var host = builder.Build();
    await host.RunAsync();
}
