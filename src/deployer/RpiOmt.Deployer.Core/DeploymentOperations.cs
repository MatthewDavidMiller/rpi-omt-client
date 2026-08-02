namespace RpiOmt.Deployer.Core;

/// <summary>Small application facade that composes the deployment domain services.</summary>
public sealed class DeploymentOperations : IDeploymentOperations
{
    public const string Arm64CheckImage = ImageBuildService.Arm64CheckImage;
    public const string BinfmtImage = ImageBuildService.BinfmtImage;

    private readonly IRemoteClientFactory _remoteClientFactory;
    private readonly ProgressRedactionService _progress;
    private readonly ImageBuildService _imageBuild;
    private readonly ArtifactPromotionService _artifacts;
    private readonly RemoteManagementService _remoteManagement;
    private readonly WifiMutationService _wifi;

    public DeploymentOperations(
        ICommandRunner commandRunner,
        IRemoteClientFactory remoteClientFactory,
        IArtifactSnapshotProvider snapshotProvider)
    {
        _remoteClientFactory = remoteClientFactory;
        _progress = new ProgressRedactionService(value => Progress?.Invoke(this, value));
        _imageBuild = new ImageBuildService(commandRunner, _progress);
        _artifacts = new ArtifactPromotionService(snapshotProvider, _progress);
        _remoteManagement = new RemoteManagementService(remoteClientFactory, _progress);
        _wifi = new WifiMutationService(remoteClientFactory, _progress);
    }

    public event EventHandler<ProgressEventArgs>? Progress;

    public async Task DeployAsync(
        ActionSnapshot snapshot,
        CancellationToken cancellationToken)
    {
        PiConnection connection = snapshot.Connection;
        DeployOptions options = snapshot.Options;
        _progress.Configure(connection, snapshot.Wifi.Password);
        _progress.SetStage("validation", "Validating deployment inputs...");
        DeploymentGuards.ValidateOrThrow(connection, options);
        ArtifactPromotionService.ValidateProjectArtifacts(options);
        if (options.BuildImage)
        {
            await _imageBuild.BuildAsync(options, cancellationToken).ConfigureAwait(false);
        }
        else
        {
            DeploymentGuards.RequireRegularFile(options.TarballPath);
        }

        _progress.SetStage("connect", "Connecting to Raspberry Pi...");
        cancellationToken.ThrowIfCancellationRequested();
        await using IRemoteClient remote = _remoteClientFactory.Create();
        await remote.ConnectAsync(connection, cancellationToken).ConfigureAwait(false);
        CommandResult architecture = _progress.Redact(await remote.RunAsync(
            RemoteManagementService.PlatformProbeCommand,
            string.Empty,
            null,
            RemoteManagementService.RemoteTimeout,
            cancellationToken).ConfigureAwait(false));
        DeploymentGuards.RequireSuccess(architecture);
        DeploymentGuards.RequireAlpinePi5(architecture);
        await _remoteManagement.PrepareDirectoryAsync(
            remote,
            connection,
            options,
            cancellationToken).ConfigureAwait(false);
        await _artifacts.UploadAndPromoteAsync(
            remote,
            options,
            cancellationToken).ConfigureAwait(false);
        await _remoteManagement.RunInstallerAsync(
            remote,
            connection,
            options,
            cancellationToken).ConfigureAwait(false);
        _progress.SetStage(
            "complete",
            "Deployment complete; use the installer URL shown above.");
    }

    public Task InstallPrerequisitesAsync(
        string projectRoot,
        CancellationToken cancellationToken)
    {
        _progress.Reset();
        return _imageBuild.InstallPrerequisitesAsync(projectRoot, cancellationToken);
    }

    public Task TestConnectionAsync(
        PiConnection connection,
        CancellationToken cancellationToken)
    {
        _progress.Configure(connection);
        return _remoteManagement.TestConnectionAsync(connection, cancellationToken);
    }

    public Task<string> FetchStatusAsync(
        PiConnection connection,
        string remoteDirectory,
        CancellationToken cancellationToken) =>
        ManageAsync(
            connection,
            remoteDirectory,
            "docker compose -f deploy/compose.yml ps",
            "Fetching container status...",
            cancellationToken);

    public Task<string> FetchLogsAsync(
        PiConnection connection,
        string remoteDirectory,
        CancellationToken cancellationToken) =>
        ManageAsync(
            connection,
            remoteDirectory,
            "docker compose -f deploy/compose.yml logs --tail=120",
            "Fetching recent logs...",
            cancellationToken);

    public Task<string> RestartServiceAsync(
        PiConnection connection,
        string remoteDirectory,
        CancellationToken cancellationToken) =>
        ManageAsync(
            connection,
            remoteDirectory,
            "docker compose -f deploy/compose.yml restart",
            "Restarting service...",
            cancellationToken);

    public Task ApplyWifiAsync(
        PiConnection connection,
        WifiSettings settings,
        CancellationToken cancellationToken)
    {
        _progress.Configure(connection, settings.Password);
        return _wifi.ApplyAsync(connection, settings, cancellationToken);
    }

    private Task<string> ManageAsync(
        PiConnection connection,
        string remoteDirectory,
        string action,
        string message,
        CancellationToken cancellationToken)
    {
        _progress.Configure(connection);
        return _remoteManagement.RunTextAsync(
            connection,
            remoteDirectory,
            action,
            message,
            cancellationToken);
    }
}
