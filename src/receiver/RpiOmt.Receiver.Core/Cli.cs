using System.Globalization;

namespace RpiOmt.Receiver.Core;

public abstract record ReceiverCommand;

public sealed record VersionCommand : ReceiverCommand;

public sealed record DiscoverCommand(int WaitMilliseconds) : ReceiverCommand;

public sealed record ProbeCommand(string Target, int TimeoutMilliseconds) : ReceiverCommand;

public sealed record PlayCommand(
    string Target,
    string Connector,
    string StatusFile,
    int RetrySeconds) : ReceiverCommand;

public static class ReceiverCli
{
    private const int MaximumWaitMilliseconds = 60_000;

    public static ReceiverCommand Parse(IReadOnlyList<string> arguments)
    {
        if (arguments.Count == 1 && arguments[0] == "--version")
        {
            return new VersionCommand();
        }
        if (arguments.Count == 0)
        {
            throw new ArgumentException("A receiver command is required.");
        }

        string command = arguments[0];
        Dictionary<string, string?> options = ParseOptions(arguments.Skip(1));
        return command switch
        {
            "discover" => ParseDiscover(options),
            "probe" => ParseProbe(options),
            "play" => ParsePlay(options),
            _ => throw new ArgumentException($"Unknown receiver command: {command}"),
        };
    }

    private static DiscoverCommand ParseDiscover(Dictionary<string, string?> options)
    {
        EnsureAllowed(options, "--wait-ms", "--json");
        RequireFlag(options, "--json");
        return new(IntegerOption(options, "--wait-ms", 1_500, 0, MaximumWaitMilliseconds));
    }

    private static ProbeCommand ParseProbe(Dictionary<string, string?> options)
    {
        EnsureAllowed(options, "--target", "--timeout-ms", "--json");
        RequireFlag(options, "--json");
        string target = RequiredOption(options, "--target");
        TargetValidator.Validate(target);
        return new(
            target,
            IntegerOption(options, "--timeout-ms", 3_000, 1, MaximumWaitMilliseconds));
    }

    private static PlayCommand ParsePlay(Dictionary<string, string?> options)
    {
        EnsureAllowed(
            options,
            "--target",
            "--connector",
            "--status-file",
            "--retry-seconds");
        string target = RequiredOption(options, "--target");
        TargetValidator.Validate(target);
        string connector = options.GetValueOrDefault("--connector") ?? "auto";
        if (connector is not ("auto" or "HDMI-A-1" or "HDMI-A-2"))
        {
            throw new ArgumentException(
                "--connector must be auto, HDMI-A-1, or HDMI-A-2.");
        }
        return new(
            target,
            connector,
            RequiredOption(options, "--status-file"),
            IntegerOption(options, "--retry-seconds", 2, 1, 30));
    }

    private static Dictionary<string, string?> ParseOptions(IEnumerable<string> arguments)
    {
        string[] values = arguments.ToArray();
        Dictionary<string, string?> options = new(StringComparer.Ordinal);
        for (int index = 0; index < values.Length; index++)
        {
            string key = values[index];
            if (!key.StartsWith("--", StringComparison.Ordinal))
            {
                throw new ArgumentException($"Unexpected argument: {key}");
            }
            bool flag = key == "--json";
            string? value = null;
            if (!flag)
            {
                if (++index >= values.Length)
                {
                    throw new ArgumentException($"Missing value for {key}.");
                }
                value = values[index];
            }
            if (!options.TryAdd(key, value))
            {
                throw new ArgumentException($"Duplicate option: {key}");
            }
        }
        return options;
    }

    private static void EnsureAllowed(
        IReadOnlyDictionary<string, string?> options,
        params string[] allowed)
    {
        HashSet<string> allowedSet = new(allowed, StringComparer.Ordinal);
        string? unknown = options.Keys.FirstOrDefault(key => !allowedSet.Contains(key));
        if (unknown is not null)
        {
            throw new ArgumentException($"Option {unknown} is not valid for this command.");
        }
    }

    private static void RequireFlag(
        Dictionary<string, string?> options,
        string name)
    {
        if (!options.TryGetValue(name, out string? value) || value is not null)
        {
            throw new ArgumentException($"{name} is required.");
        }
    }

    private static string RequiredOption(
        Dictionary<string, string?> options,
        string name) =>
        options.TryGetValue(name, out string? value) && !string.IsNullOrEmpty(value)
            ? value
            : throw new ArgumentException($"{name} is required.");

    private static int IntegerOption(
        Dictionary<string, string?> options,
        string name,
        int defaultValue,
        int minimum,
        int maximum)
    {
        if (!options.TryGetValue(name, out string? raw))
        {
            return defaultValue;
        }
        if (!int.TryParse(raw, NumberStyles.None, CultureInfo.InvariantCulture, out int value) ||
            value < minimum || value > maximum)
        {
            throw new ArgumentException(
                $"{name} must be between {minimum} and {maximum}.");
        }
        return value;
    }
}
