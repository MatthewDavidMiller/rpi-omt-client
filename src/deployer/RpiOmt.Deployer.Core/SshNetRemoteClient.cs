using Renci.SshNet;
using Renci.SshNet.Common;
using System.Security.Cryptography;
using System.Text;

namespace RpiOmt.Deployer.Core;

public sealed class SshNetRemoteClientFactory(ICommandRunner commandRunner) : IRemoteClientFactory
{
    public IRemoteClient Create() => new SshNetRemoteClient(commandRunner);
}

public sealed class SshNetRemoteClient(ICommandRunner commandRunner) : IRemoteClient
{
    public const int MaximumRemoteOutputBytes = 4 * 1024 * 1024;
    private static readonly TimeSpan ConnectTimeout = TimeSpan.FromSeconds(15);
    private SshClient? _sshClient;
    private SftpClient? _sftpClient;

    public async Task ConnectAsync(PiConnection connection, CancellationToken cancellationToken)
    {
        var errors = InputValidation.ValidateConnection(connection);
        if (errors.Count > 0)
        {
            throw new DeploymentException(string.Join(Environment.NewLine, errors));
        }

        var knownKeys = await new KnownHostsVerifier(commandRunner).LoadAsync(
            connection.Host,
            connection.Port,
            null,
            cancellationToken).ConfigureAwait(false);
        var connectionInfo = CreateConnectionInfo(connection);
        _sshClient = new SshClient(connectionInfo);
        _sftpClient = new SftpClient(connectionInfo);
        AttachStrictHostCheck(_sshClient, knownKeys);
        AttachStrictHostCheck(_sftpClient, knownKeys);
        try
        {
            await ConnectAsync(_sshClient, cancellationToken).ConfigureAwait(false);
            await ConnectAsync(_sftpClient, cancellationToken).ConfigureAwait(false);
        }
        catch
        {
            await DisposeAsync().ConfigureAwait(false);
            throw;
        }
    }

    public async Task<CommandResult> RunAsync(
        string command,
        string input,
        Action<string>? onOutput,
        TimeSpan timeout,
        CancellationToken cancellationToken)
    {
        var client = _sshClient ?? throw new InvalidOperationException("SSH client is not connected.");
        using var remoteCommand = client.CreateCommand(command);
        remoteCommand.CommandTimeout = timeout;
        using var timeoutSource = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeoutSource.CancelAfter(timeout);
        var stdoutBuffer = new BoundedTextBuffer(MaximumRemoteOutputBytes);
        var stderrBuffer = new BoundedTextBuffer(MaximumRemoteOutputBytes);
        var execute = remoteCommand.ExecuteAsync(timeoutSource.Token);
        var stdout = DrainAsync(remoteCommand.OutputStream, stdoutBuffer, timeoutSource.Token);
        var stderr = DrainAsync(remoteCommand.ExtendedOutputStream, stderrBuffer, timeoutSource.Token);
        var inputBytes = Encoding.UTF8.GetBytes(input);
        try
        {
            using (var inputStream = remoteCommand.CreateInputStream())
            {
                await inputStream.WriteAsync(inputBytes, timeoutSource.Token).ConfigureAwait(false);
            }

            await execute.ConfigureAwait(false);
            await Task.WhenAll(stdout, stderr).ConfigureAwait(false);
        }
        catch (OperationCanceledException exception) when (!cancellationToken.IsCancellationRequested)
        {
            client.Disconnect();
            throw new TimeoutException(
                $"Remote command timed out after {timeout.TotalSeconds:g} seconds: {command}",
                exception);
        }
        catch (OperationCanceledException)
        {
            client.Disconnect();
            throw;
        }
        catch (SshOperationTimeoutException exception)
        {
            client.Disconnect();
            throw new TimeoutException(
                $"Remote command timed out after {timeout.TotalSeconds:g} seconds: {command}",
                exception);
        }
        finally
        {
            CryptographicOperations.ZeroMemory(inputBytes);
        }

        var standardOutput = stdoutBuffer.ToString();
        var standardError = stderrBuffer.ToString();
        if (onOutput is not null)
        {
            foreach (var line in (standardOutput + standardError).Split('\n', StringSplitOptions.RemoveEmptyEntries))
            {
                onOutput(line.TrimEnd('\r'));
            }
        }

        return new CommandResult(command, remoteCommand.ExitStatus ?? -1, standardOutput, standardError);
    }

    public async Task UploadAsync(
        string localPath,
        string remotePath,
        TimeSpan timeout,
        Action<long, long>? onProgress,
        CancellationToken cancellationToken)
    {
        var client = _sftpClient ?? throw new InvalidOperationException("SFTP client is not connected.");
        await using var source = new FileStream(
            localPath,
            FileMode.Open,
            FileAccess.Read,
            FileShare.Read,
            1024 * 1024,
            FileOptions.Asynchronous | FileOptions.SequentialScan);
        var total = source.Length;
        var upload = Task.Run(
            () => client.UploadFile(source, remotePath, true, uploaded =>
            {
                cancellationToken.ThrowIfCancellationRequested();
                onProgress?.Invoke(checked((long)uploaded), total);
            }),
            CancellationToken.None);
        try
        {
            await upload.WaitAsync(timeout, cancellationToken).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            client.Disconnect();
            throw;
        }
        catch (TimeoutException exception)
        {
            client.Disconnect();
            throw new TimeoutException(
                $"SFTP upload timed out after {timeout.TotalSeconds:g} seconds: {Path.GetFileName(localPath)}",
                exception);
        }
    }

    public ValueTask DisposeAsync()
    {
        _sftpClient?.Dispose();
        _sshClient?.Dispose();
        _sftpClient = null;
        _sshClient = null;
        return ValueTask.CompletedTask;
    }

    private static ConnectionInfo CreateConnectionInfo(PiConnection connection)
    {
        AuthenticationMethod authentication = connection.AuthMethod switch
        {
            AuthMethod.Password => new PasswordAuthenticationMethod(connection.Username, connection.Password),
            AuthMethod.Key => new PrivateKeyAuthenticationMethod(
                connection.Username,
                string.IsNullOrEmpty(connection.KeyPassphrase)
                    ? new PrivateKeyFile(connection.KeyPath!)
                    : new PrivateKeyFile(connection.KeyPath!, connection.KeyPassphrase)),
            _ => throw new ArgumentOutOfRangeException(nameof(connection)),
        };
        return new ConnectionInfo(connection.Host, connection.Port, connection.Username, authentication)
        {
            Timeout = ConnectTimeout,
        };
    }

    private static void AttachStrictHostCheck(BaseClient client, IReadOnlyList<KnownHostKey> knownKeys)
    {
        client.HostKeyReceived += (_, eventArgs) =>
        {
            eventArgs.CanTrust = KnownHostsVerifier.Matches(
                knownKeys,
                eventArgs.HostKeyName,
                eventArgs.HostKey);
        };
    }

    private static async Task ConnectAsync(BaseClient client, CancellationToken cancellationToken)
    {
        var connect = Task.Run(client.Connect, CancellationToken.None);
        try
        {
            await connect.WaitAsync(ConnectTimeout, cancellationToken).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            client.Dispose();
            throw;
        }
        catch (TimeoutException exception)
        {
            client.Dispose();
            throw new TimeoutException("SSH connection timed out after 15 seconds.", exception);
        }
    }

    private static async Task DrainAsync(Stream source, BoundedTextBuffer destination, CancellationToken cancellationToken)
    {
        var buffer = new byte[16 * 1024];
        while (true)
        {
            var count = await source.ReadAsync(buffer, cancellationToken).ConfigureAwait(false);
            if (count == 0)
            {
                return;
            }

            // The buffer decodes: a read boundary can fall inside a UTF-8
            // scalar, and only it carries the decoder state across chunks.
            destination.Append(buffer.AsSpan(0, count));
        }
    }
}
