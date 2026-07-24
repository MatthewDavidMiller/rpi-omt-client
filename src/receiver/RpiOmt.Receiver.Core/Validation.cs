using System.Buffers;
using System.Globalization;
using System.Net;
using System.Text;

namespace RpiOmt.Receiver.Core;

public static class TargetValidator
{
    private static readonly UTF8Encoding StrictUtf8 = new(false, true);

    public static void Validate(string value)
    {
        if (value.StartsWith("omt://", StringComparison.Ordinal))
        {
            if (!IsValidDirectTarget(value))
            {
                throw new ArgumentException("Invalid OMT direct target.");
            }
            return;
        }
        if (!IsValidDiscoveredName(value))
        {
            throw new ArgumentException("Invalid OMT source name.");
        }
    }

    public static bool IsValidDiscoveredName(string value)
    {
        if (string.IsNullOrEmpty(value) || value != value.Trim())
        {
            return false;
        }
        try
        {
            if (!value.IsNormalized(NormalizationForm.FormC))
            {
                return false;
            }
        }
        catch (ArgumentException)
        {
            return false;
        }
        try
        {
            if (StrictUtf8.GetByteCount(value) > 63)
            {
                return false;
            }
        }
        catch (EncoderFallbackException)
        {
            return false;
        }

        ReadOnlySpan<char> remaining = value.AsSpan();
        while (!remaining.IsEmpty)
        {
            OperationStatus status = Rune.DecodeFromUtf16(
                remaining,
                out Rune rune,
                out int consumed);
            if (status != OperationStatus.Done)
            {
                return false;
            }
            UnicodeCategory category = Rune.GetUnicodeCategory(rune);
            if (category is
                UnicodeCategory.Control or
                UnicodeCategory.Format or
                UnicodeCategory.LineSeparator or
                UnicodeCategory.ParagraphSeparator or
                UnicodeCategory.Surrogate)
            {
                return false;
            }
            remaining = remaining[consumed..];
        }
        return true;
    }

    public static bool IsValidDirectTarget(string value)
    {
        if (string.IsNullOrEmpty(value) ||
            value.Length > 512 ||
            !value.StartsWith("omt://", StringComparison.Ordinal) ||
            value.Any(character => character > 0x7f || char.IsControl(character)))
        {
            return false;
        }
        string authority = value[6..];
        if (authority.Length == 0 ||
            authority.Contains('@') ||
            authority.Contains('/') ||
            authority.Contains('?') ||
            authority.Contains('#'))
        {
            return false;
        }

        string host;
        string portText;
        if (authority.StartsWith('['))
        {
            int close = authority.IndexOf(']');
            if (close <= 1 || close + 1 >= authority.Length || authority[close + 1] != ':')
            {
                return false;
            }
            host = authority[1..close];
            portText = authority[(close + 2)..];
            if (!IPAddress.TryParse(host, out IPAddress? address) ||
                address.AddressFamily != System.Net.Sockets.AddressFamily.InterNetworkV6)
            {
                return false;
            }
        }
        else
        {
            int separator = authority.LastIndexOf(':');
            if (separator <= 0 || separator == authority.Length - 1)
            {
                return false;
            }
            host = authority[..separator];
            portText = authority[(separator + 1)..];
            if (host.Contains(':') || !IsValidHost(host))
            {
                return false;
            }
        }
        return int.TryParse(
                portText,
                NumberStyles.None,
                CultureInfo.InvariantCulture,
                out int port) &&
            port is >= 1 and <= 65535;
    }

    private static bool IsValidHost(string host)
    {
        if (host.Length > 253)
        {
            return false;
        }
        string[] labels = host.Split('.');
        return labels.All(label =>
            label.Length is >= 1 and <= 63 &&
            IsAsciiAlphaNumeric(label[0]) &&
            IsAsciiAlphaNumeric(label[^1]) &&
            label.All(character => IsAsciiAlphaNumeric(character) || character == '-'));
    }

    private static bool IsAsciiAlphaNumeric(char value) =>
        value is >= 'a' and <= 'z' or
            >= 'A' and <= 'Z' or
            >= '0' and <= '9';
}

public static class FormatPolicy
{
    public static bool IsSupported(int width, int height, double frameRate) =>
        width is >= 1 and <= 1920 &&
        height is >= 1 and <= 1080 &&
        double.IsFinite(frameRate) &&
        frameRate > 0 &&
        frameRate <= 60.0;
}

public static class StatusSanitizer
{
    public static string Sanitize(string value)
    {
        StringBuilder result = new();
        foreach (Rune rune in value.EnumerateRunes())
        {
            if (result.Length >= 512)
            {
                break;
            }
            if (!Rune.IsControl(rune))
            {
                result.Append(rune);
            }
        }
        return result.ToString().Trim();
    }
}
