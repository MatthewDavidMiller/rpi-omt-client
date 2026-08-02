namespace RpiOmt.Deployer.Core;

internal sealed class RemoteManagementService(
    IRemoteClientFactory remoteClientFactory,
    ProgressRedactionService progress)
{
    public static readonly TimeSpan RemoteTimeout = TimeSpan.FromSeconds(60);
    public const string PlatformProbeCommand =
        "uname -m && . /etc/os-release && printf '%s\\n' \"$ID\" && " +
        "cat /etc/alpine-release && tr -d '\\000' < /proc/device-tree/model && " +
        "printf '\\n' && hostname";
    private static readonly TimeSpan InstallerTimeout = TimeSpan.FromMinutes(30);

    public async Task TestConnectionAsync(
        PiConnection connection,
        CancellationToken cancellationToken)
    {
        DeploymentGuards.ValidateConnectionOrThrow(connection);
        progress.SetStage("test-connection", "Testing SSH connection...");
        await using IRemoteClient remote = remoteClientFactory.Create();
        await remote.ConnectAsync(connection, cancellationToken).ConfigureAwait(false);
        CommandResult result = progress.Redact(await remote.RunAsync(
            PlatformProbeCommand,
            string.Empty,
            null,
            RemoteTimeout,
            cancellationToken).ConfigureAwait(false));
        DeploymentGuards.RequireSuccess(result);
        DeploymentGuards.RequireAlpinePi5(result);
        progress.Emit("SSH connection succeeded.");
    }

    public async Task PrepareDirectoryAsync(
        IRemoteClient remote,
        PiConnection connection,
        DeployOptions options,
        CancellationToken cancellationToken)
    {
        progress.SetStage(
            "remote-prepare",
            "Preparing the remote deployment directory...");
        string sudoPassword = DeploymentGuards.SudoPassword(connection);
        string sudo = string.IsNullOrEmpty(sudoPassword)
            ? "sudo -n"
            : "sudo -S -p ''";
        string command =
            $"{sudo} install -d -m 755 -o \"$(id -u)\" -g \"$(id -g)\" " +
            Shell.Quote(options.RemoteDirectory);
        CommandResult result = progress.Redact(await remote.RunAsync(
            command,
            string.IsNullOrEmpty(sudoPassword) ? string.Empty : $"{sudoPassword}\n",
            null,
            RemoteTimeout,
            cancellationToken).ConfigureAwait(false));
        DeploymentGuards.RequireSuccess(result);
    }

    public async Task RunInstallerAsync(
        IRemoteClient remote,
        PiConnection connection,
        DeployOptions options,
        CancellationToken cancellationToken)
    {
        cancellationToken.ThrowIfCancellationRequested();
        string directory = options.RemoteDirectory.TrimEnd('/');
        string sudoPassword = DeploymentGuards.SudoPassword(connection);
        string sudo = string.IsNullOrEmpty(sudoPassword)
            ? "sudo -n"
            : "sudo -S -p ''";
        string installer = $"{directory}/deploy/host/install.sh";
        string[] executablePaths =
        [
            installer,
            $"{directory}/deploy/host/uninstall.sh",
            $"{directory}/deploy/host/host-diagnostics.sh",
            $"{directory}/deploy/host/host-event-watcher.sh",
            $"{directory}/deploy/host/host-reboot.sh",
            $"{directory}/deploy/transaction.sh",
        ];
        string installCommand = $"printf 'n\\n' | {Shell.Quote(installer)}";
        string command =
            $"chmod +x {string.Join(' ', executablePaths.Select(Shell.Quote))} && " +
            $"{sudo} sh -c {Shell.Quote(installCommand)}";
        progress.SetStage(
            "installer",
            "Running the privileged remote installer; " +
            "cancellation is disabled until it finishes.",
            false);
        CommandResult result = progress.Redact(await remote.RunAsync(
            command,
            string.IsNullOrEmpty(sudoPassword) ? string.Empty : $"{sudoPassword}\n",
            line => progress.Emit(line),
            InstallerTimeout,
            CancellationToken.None).ConfigureAwait(false));
        DeploymentGuards.RequireSuccess(result);
    }

    public async Task<string> RunTextAsync(
        PiConnection connection,
        string remoteDirectory,
        string action,
        string message,
        CancellationToken cancellationToken)
    {
        DeploymentGuards.ValidateConnectionOrThrow(connection);
        if (!InputValidation.IsValidRemoteDirectory(remoteDirectory))
        {
            throw new DeploymentException(
                "Remote install directory is not a normalized safe path: " +
                remoteDirectory);
        }

        progress.SetStage("remote-read", message);
        await using IRemoteClient remote = remoteClientFactory.Create();
        await remote.ConnectAsync(connection, cancellationToken).ConfigureAwait(false);
        CommandResult result = progress.Redact(await remote.RunAsync(
            $"cd {Shell.Quote(remoteDirectory)} && {action}",
            string.Empty,
            line => progress.Emit(line),
            RemoteTimeout,
            cancellationToken).ConfigureAwait(false));
        DeploymentGuards.RequireSuccess(result);
        return result.StandardOutput;
    }
}
