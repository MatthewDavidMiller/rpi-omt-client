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
    private readonly Decoder _decoder = Encoding.UTF8.GetDecoder();
    private char[] _decoded = [];
    private int _retainedBytes;
    private long _totalBytes;

    public bool Truncated { get; private set; }

    /// <summary>
    /// Appends one chunk of a UTF-8 byte stream. A chunk boundary may fall
    /// inside a scalar, so the decoder state is kept across calls and the
    /// split character is completed by the next chunk instead of decoding to
    /// U+FFFD. A sequence still incomplete when the stream ends is dropped.
    /// </summary>
    public void Append(ReadOnlySpan<byte> value)
    {
        var required = Encoding.UTF8.GetMaxCharCount(value.Length);
        if (_decoded.Length < required)
        {
            _decoded = new char[required];
        }

        var count = _decoder.GetChars(value, _decoded, false);
        if (count > 0)
        {
            Append(_decoded.AsSpan(0, count));
        }
    }

    public void Append(ReadOnlySpan<char> value)
    {
        var bytes = Encoding.UTF8.GetBytes(value.ToString());
        _totalBytes += bytes.Length;
        var available = Math.Max(0, maximumBytes - _retainedBytes);
        var count = Math.Min(available, bytes.Length);

        // bytes[count] is the first byte that will be dropped. A continuation
        // byte there means the limit fell inside a scalar, so retreat to that
        // scalar's lead byte: retaining a partial sequence would decode to
        // U+FFFD and corrupt the last character an operator gets to see.
        while (count > 0 && count < bytes.Length && (bytes[count] & 0xc0) == 0x80)
        {
            count--;
        }

        if (count > 0)
        {
            _builder.Append(Encoding.UTF8.GetString(bytes, 0, count));
            _retainedBytes += count;
        }

        Truncated |= count < bytes.Length;
    }

    public override string ToString()
    {
        if (!Truncated)
        {
            return _builder.ToString();
        }

        return $"{_builder}\n[output truncated: retained {_retainedBytes} of {_totalBytes} bytes]\n";
    }
}
