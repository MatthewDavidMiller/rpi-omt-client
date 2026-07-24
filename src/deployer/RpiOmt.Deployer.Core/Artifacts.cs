using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;

namespace RpiOmt.Deployer.Core;

public sealed class ArtifactException(string message) : IOException(message);

public sealed record ArtifactIdentity(long Length, DateTime CreationTimeUtc, DateTime LastWriteTimeUtc);

public sealed record ArtifactSnapshot(string Path, ArtifactIdentity Identity, string Sha256)
{
    public async Task<bool> IsUnchangedAsync(CancellationToken cancellationToken)
    {
        if (!ArtifactSnapshots.IsRegularFile(Path))
        {
            return false;
        }

        var current = new FileInfo(Path);
        current.Refresh();
        if (Identity != ArtifactSnapshots.IdentityOf(current))
        {
            return false;
        }

        await using var stream = new FileStream(
            Path,
            FileMode.Open,
            FileAccess.Read,
            FileShare.Read,
            1024 * 1024,
            FileOptions.Asynchronous | FileOptions.SequentialScan);
        var digest = await SHA256.HashDataAsync(stream, cancellationToken).ConfigureAwait(false);
        current.Refresh();
        return Identity == ArtifactSnapshots.IdentityOf(current) &&
            string.Equals(Sha256, Convert.ToHexStringLower(digest), StringComparison.Ordinal);
    }
}

public static partial class ArtifactSnapshots
{
    private const int MaximumManifestBytes = 32768;
    private const int MaximumManifestEntries = 128;
    private const int MaximumRelativePathLength = 240;

    public static IReadOnlyList<string> LoadManifest(string path)
    {
        if (!IsRegularFile(path))
        {
            throw new ArtifactException($"Required deployment artifact manifest is missing: {path}");
        }

        var info = new FileInfo(path);
        if (info.Length > MaximumManifestBytes)
        {
            throw new ArtifactException("Deployment artifact manifest exceeds 32768 bytes.");
        }

        string text;
        try
        {
            var bytes = File.ReadAllBytes(path);
            text = Encoding.GetEncoding(
                "us-ascii",
                EncoderFallback.ExceptionFallback,
                DecoderFallback.ExceptionFallback).GetString(bytes);
        }
        catch (Exception exception) when (exception is IOException or DecoderFallbackException)
        {
            throw new ArtifactException($"Unable to read deployment artifact manifest: {exception.Message}");
        }

        var lines = text.Split('\n')
            .Select(name => name.TrimEnd('\r'))
            .ToArray();
        if (lines.Length > 0 && lines[^1].Length == 0)
        {
            lines = lines[..^1];
        }

        if (lines.Length == 0 || !string.Equals(lines[0], "version=2", StringComparison.Ordinal))
        {
            throw new ArtifactException("Deployment artifact manifest must begin with version=2.");
        }

        var names = lines[1..];
        if (names.Length == 0)
        {
            throw new ArtifactException("Deployment artifact manifest is empty.");
        }

        if (names.Length > MaximumManifestEntries)
        {
            throw new ArtifactException("Deployment artifact manifest has too many entries.");
        }

        if (names.Distinct(StringComparer.Ordinal).Count() != names.Length ||
            names.Any(name => !IsSafeRelativePath(name)))
        {
            throw new ArtifactException("Deployment artifact manifest contains unsafe, empty, or duplicate paths.");
        }

        return names;
    }

    public static bool IsSafeRelativePath(string path)
    {
        if (path.Length is 0 or > MaximumRelativePathLength ||
            path[0] == '/' ||
            path[^1] == '/' ||
            path.Contains("//", StringComparison.Ordinal) ||
            !ArtifactPathPattern().IsMatch(path))
        {
            return false;
        }

        return path.Split('/').All(component => component is not ("" or "." or ".."));
    }

    public static async Task<ArtifactSnapshot> CaptureAsync(string path, CancellationToken cancellationToken)
    {
        if (!IsRegularFile(path))
        {
            throw new ArtifactException($"Required file is missing: {path}");
        }

        var before = new FileInfo(path);
        before.Refresh();
        var beforeIdentity = IdentityOf(before);
        await using var stream = new FileStream(
            path,
            FileMode.Open,
            FileAccess.Read,
            FileShare.Read,
            1024 * 1024,
            FileOptions.Asynchronous | FileOptions.SequentialScan);
        var digest = await SHA256.HashDataAsync(stream, cancellationToken).ConfigureAwait(false);
        var after = new FileInfo(path);
        after.Refresh();
        var afterIdentity = IdentityOf(after);
        if (beforeIdentity != afterIdentity || !IsRegularFile(path))
        {
            throw new ArtifactException($"Deployment artifact changed while it was hashed: {System.IO.Path.GetFileName(path)}");
        }

        return new ArtifactSnapshot(path, afterIdentity, Convert.ToHexStringLower(digest));
    }

    internal static ArtifactIdentity IdentityOf(FileInfo info) =>
        new(info.Length, info.CreationTimeUtc, info.LastWriteTimeUtc);

    internal static bool IsRegularFile(string path)
    {
        var info = new FileInfo(path);
        info.Refresh();
        return info.Exists && info.LinkTarget is null &&
            (info.Attributes & (FileAttributes.Directory | FileAttributes.ReparsePoint)) == 0;
    }

    [GeneratedRegex("^[A-Za-z0-9._/-]+$", RegexOptions.CultureInvariant)]
    private static partial Regex ArtifactPathPattern();
}
