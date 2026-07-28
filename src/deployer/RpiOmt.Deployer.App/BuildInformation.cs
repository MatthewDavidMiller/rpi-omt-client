using System.Reflection;

namespace RpiOmt.Deployer.App;

public static class BuildInformation
{
    public const string Copyright =
        "Copyright (c) 2026 Matthew David Miller";

    public static string Version { get; } = ResolveVersion();

    public static string ProjectLicense { get; } = ReadResource("RpiOmt.ProjectLicense.txt");

    public static string ThirdPartyNotices { get; } =
        ReadResource("RpiOmt.ThirdPartyNotices.txt") +
        "\n\nMICROSOFT .NET THIRD-PARTY NOTICES\n" +
        "----------------------------------\n\n" +
        ReadResource("RpiOmt.DotNetThirdPartyNotices.txt") +
        "\n\nSKIASHARP / HARFBUZZ NATIVE THIRD-PARTY NOTICES\n" +
        "------------------------------------------------\n\n" +
        ReadResource("RpiOmt.SkiaHarfBuzzThirdPartyNotices.txt");

    private static string ResolveVersion()
    {
        var assembly = typeof(BuildInformation).Assembly;
        var informational = assembly
            .GetCustomAttribute<AssemblyInformationalVersionAttribute>()?
            .InformationalVersion;
        return string.IsNullOrWhiteSpace(informational)
            ? assembly.GetName().Version?.ToString() ?? "unknown"
            : informational;
    }

    private static string ReadResource(string name)
    {
        using var stream = typeof(BuildInformation).Assembly.GetManifestResourceStream(name)
            ?? throw new InvalidOperationException($"Required legal resource is missing: {name}");
        using var reader = new StreamReader(stream);
        return reader.ReadToEnd();
    }
}
