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
    private const int MaximumManifestBytes = 4096;
    private const int MaximumManifestEntries = 32;

    public static IReadOnlyList<string> LoadManifest(string path)
    {
        if (!IsRegularFile(path))
        {
            throw new ArtifactException($"Required deployment artifact manifest is missing: {path}");
        }

        var info = new FileInfo(path);
        if (info.Length > MaximumManifestBytes)
        {
            throw new ArtifactException("Deployment artifact manifest exceeds 4096 bytes.");
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

        var names = text.Split('\n', StringSplitOptions.RemoveEmptyEntries)
            .Select(name => name.TrimEnd('\r'))
            .ToArray();
        if (names.Length == 0)
        {
            throw new ArtifactException("Deployment artifact manifest is empty.");
        }

        if (names.Length > MaximumManifestEntries)
        {
            throw new ArtifactException("Deployment artifact manifest has too many entries.");
        }

        if (names.Distinct(StringComparer.Ordinal).Count() != names.Length ||
            names.Any(name => !ArtifactNamePattern().IsMatch(name)))
        {
            throw new ArtifactException("Deployment artifact manifest contains unsafe or duplicate names.");
        }

        return names;
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

    [GeneratedRegex("^[A-Za-z0-9._-]+$", RegexOptions.CultureInvariant)]
    private static partial Regex ArtifactNamePattern();
}
