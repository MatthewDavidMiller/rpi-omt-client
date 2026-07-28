namespace RpiOmt.Receiver.Core;

/// <summary>
/// Publishes one private file through a uniquely named, write-through stage.
/// </summary>
public static class AtomicFilePublisher
{
    private const UnixFileMode PrivateMode =
        UnixFileMode.UserRead | UnixFileMode.UserWrite;

    public static void Replace(string path, ReadOnlySpan<byte> content)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(path);
        string? directory = Path.GetDirectoryName(path);
        if (string.IsNullOrEmpty(directory))
        {
            throw new ArgumentException("Published file path must have a directory.", nameof(path));
        }

        Directory.CreateDirectory(directory);
        string temporary = Path.Combine(
            directory,
            $".{Path.GetFileName(path)}.{Environment.ProcessId}.{Guid.NewGuid():N}");
        FileStreamOptions options = new()
        {
            Mode = FileMode.CreateNew,
            Access = FileAccess.Write,
            Share = FileShare.None,
            Options = FileOptions.WriteThrough,
        };
        if (OperatingSystem.IsLinux() || OperatingSystem.IsMacOS() || OperatingSystem.IsFreeBSD())
        {
            options.UnixCreateMode = PrivateMode;
        }

        bool committed = false;
        try
        {
            using (FileStream stream = new(temporary, options))
            {
                stream.Write(content);
                stream.Flush(true);
            }
            File.Move(temporary, path, true);
            committed = true;
        }
        finally
        {
            // Only an uncommitted stage needs removing. A committed one no longer
            // exists under this name, so deleting it unconditionally spends an
            // extra unlink on every publish -- and the receiver publishes on each
            // state change plus a 500 ms heartbeat for as long as it plays.
            if (!committed)
            {
                File.Delete(temporary);
            }
        }
    }
}
