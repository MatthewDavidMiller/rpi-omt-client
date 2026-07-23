using System.Text;
using System.Text.RegularExpressions;

namespace RpiOmt.Deployer.Core;

public static partial class Shell
{
    public static string Quote(string value) => $"'{value.Replace("'", "'\"'\"'", StringComparison.Ordinal)}'";

    [GeneratedRegex("^v?[0-9]+(\\.[0-9]+){1,2}([._-][0-9A-Za-z][0-9A-Za-z._-]*)?$", RegexOptions.CultureInvariant)]
    internal static partial Regex VersionPattern();

    [GeneratedRegex("(?:^|[-_])(v?[0-9]+(\\.[0-9]+){1,2}([._-][0-9A-Za-z][0-9A-Za-z._-]*)?)$", RegexOptions.CultureInvariant)]
    internal static partial Regex SourceDirectoryVersionPattern();
}

public sealed class SecretRedactor(IEnumerable<string> values)
{
    private readonly string[] _values = values
        .Where(value => !string.IsNullOrEmpty(value))
        .Distinct(StringComparer.Ordinal)
        .OrderByDescending(value => value.Length)
        .ToArray();

    public string Redact(string value)
    {
        foreach (var secret in _values)
        {
            value = value.Replace(secret, "<redacted>", StringComparison.Ordinal);
        }

        return value;
    }

    public CommandResult Redact(CommandResult result) => result with
    {
        Command = Redact(result.Command),
        StandardOutput = Redact(result.StandardOutput),
        StandardError = Redact(result.StandardError),
    };
}

public sealed class BoundedTextBuffer(int maximumBytes)
{
    private readonly StringBuilder _builder = new();
    private int _retainedBytes;
    private long _totalBytes;

    public bool Truncated { get; private set; }

    public void Append(ReadOnlySpan<char> value)
    {
        var bytes = Encoding.UTF8.GetBytes(value.ToString());
        _totalBytes += bytes.Length;
        var available = Math.Max(0, maximumBytes - _retainedBytes);
        if (available > 0)
        {
            var count = Math.Min(available, bytes.Length);
            while (count > 0 && (bytes[count - 1] & 0xc0) == 0x80)
            {
                count--;
            }

            _builder.Append(Encoding.UTF8.GetString(bytes, 0, count));
            _retainedBytes += count;
        }

        Truncated |= bytes.Length > available;
    }

    public override string ToString()
    {
        if (!Truncated)
        {
            return _builder.ToString();
        }

        return $"{_builder}\n[output truncated: retained {maximumBytes} of {_totalBytes} bytes]\n";
    }
}
