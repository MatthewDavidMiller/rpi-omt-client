namespace RpiOmt.Deployer.Core;

public interface ICommandRunner
{
    Task<CommandResult> RunAsync(
        IReadOnlyList<string> arguments,
        string? workingDirectory,
        Action<string>? onOutput,
        TimeSpan timeout,
        CancellationToken cancellationToken);
}

public interface IRemoteClient : IAsyncDisposable
{
    Task ConnectAsync(PiConnection connection, CancellationToken cancellationToken);

    Task<CommandResult> RunAsync(
        string command,
        string input,
        Action<string>? onOutput,
        TimeSpan timeout,
        CancellationToken cancellationToken);

    Task UploadAsync(
        string localPath,
        string remotePath,
        TimeSpan timeout,
        Action<long, long>? onProgress,
        CancellationToken cancellationToken);
}

public interface IRemoteClientFactory
{
    IRemoteClient Create();
}

public interface IArtifactSnapshotProvider
{
    Task<ArtifactSnapshot> CaptureAsync(string path, CancellationToken cancellationToken);
}

public interface IDeploymentOperations
{
    event EventHandler<ProgressEventArgs>? Progress;

    Task DeployAsync(ActionSnapshot snapshot, CancellationToken cancellationToken);

    Task InstallPrerequisitesAsync(string projectRoot, CancellationToken cancellationToken);

    Task TestConnectionAsync(PiConnection connection, CancellationToken cancellationToken);

    Task<string> FetchStatusAsync(PiConnection connection, string remoteDirectory, CancellationToken cancellationToken);

    Task<string> FetchLogsAsync(PiConnection connection, string remoteDirectory, CancellationToken cancellationToken);

    Task<string> RestartServiceAsync(PiConnection connection, string remoteDirectory, CancellationToken cancellationToken);

    Task ApplyWifiAsync(PiConnection connection, WifiSettings settings, CancellationToken cancellationToken);
}

public sealed class ArtifactSnapshotProvider : IArtifactSnapshotProvider
{
    public Task<ArtifactSnapshot> CaptureAsync(string path, CancellationToken cancellationToken) =>
        ArtifactSnapshots.CaptureAsync(path, cancellationToken);
}

public sealed class CommandException(CommandResult result) : InvalidOperationException(
    string.IsNullOrWhiteSpace(result.StandardError)
        ? string.IsNullOrWhiteSpace(result.StandardOutput) ? $"Command failed: {result.Command}" : result.StandardOutput
        : result.StandardError)
{
    public CommandResult Result { get; } = result;
}

public sealed class DeploymentException(string message) : InvalidOperationException(message);
