using System.Text;

namespace RpiOmt.Deployer.Core;

public sealed record KnownHostKey(string Algorithm, byte[] Key);

public sealed class KnownHostsVerifier(ICommandRunner commandRunner)
{
    private static readonly TimeSpan LookupTimeout = TimeSpan.FromSeconds(10);

    public async Task<IReadOnlyList<KnownHostKey>> LoadAsync(
        string host,
        int port,
        string? knownHostsPath,
        CancellationToken cancellationToken)
    {
        knownHostsPath ??= Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
            ".ssh",
            "known_hosts");
        if (!File.Exists(knownHostsPath))
        {
            throw new DeploymentException(
                $"OpenSSH known_hosts was not found at {knownHostsPath}. Connect with ssh first and verify the Raspberry Pi host key.");
        }

        if (!ProcessCommandRunner.IsExecutableAvailable("ssh-keygen"))
        {
            throw new FileNotFoundException("Required executable not found on PATH: ssh-keygen");
        }

        var lookup = port == 22 ? host : $"[{host}]:{port}";
        var result = await commandRunner.RunAsync(
            ["ssh-keygen", "-F", lookup, "-f", knownHostsPath],
            null,
            null,
            LookupTimeout,
            cancellationToken).ConfigureAwait(false);
        if (result.ExitCode == 1)
        {
            throw new DeploymentException(
                $"SSH host key is unknown for {lookup}. Connect with OpenSSH first and verify the key before using the deployer.");
        }

        if (!result.IsSuccess)
        {
            throw new CommandException(result);
        }

        var keys = new List<KnownHostKey>();
        foreach (var line in result.StandardOutput.Split('\n', StringSplitOptions.RemoveEmptyEntries))
        {
            if (line.StartsWith('#'))
            {
                continue;
            }

            var fields = line.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);
            if (fields.Length < 3)
            {
                continue;
            }

            try
            {
                keys.Add(new KnownHostKey(fields[1], Convert.FromBase64String(fields[2])));
            }
            catch (FormatException)
            {
                // ssh-keygen may surface unrelated malformed lines; none are trusted.
            }
        }

        return keys.Count > 0
            ? keys
            : throw new DeploymentException($"No usable OpenSSH host keys were found for {lookup}.");
    }

    public static bool Matches(IEnumerable<KnownHostKey> knownKeys, string algorithm, ReadOnlySpan<byte> presentedKey)
    {
        foreach (var key in knownKeys)
        {
            if (string.Equals(key.Algorithm, algorithm, StringComparison.Ordinal) && presentedKey.SequenceEqual(key.Key))
            {
                return true;
            }
        }

        return false;
    }
}
