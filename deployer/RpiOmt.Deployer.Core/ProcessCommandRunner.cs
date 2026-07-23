using System.ComponentModel;
using System.Diagnostics;
using System.Text;

namespace RpiOmt.Deployer.Core;

public sealed class ProcessCommandRunner : ICommandRunner
{
    public const int MaximumOutputBytes = 4 * 1024 * 1024;
    private const int MaximumPendingCharacters = 65_536;

    public async Task<CommandResult> RunAsync(
        IReadOnlyList<string> arguments,
        string? workingDirectory,
        Action<string>? onOutput,
        TimeSpan timeout,
        CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(arguments);
        if (arguments.Count == 0)
        {
            throw new ArgumentException("A command is required.", nameof(arguments));
        }

        var startInfo = new ProcessStartInfo
        {
            FileName = arguments[0],
            WorkingDirectory = workingDirectory ?? string.Empty,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            RedirectStandardInput = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        foreach (var argument in arguments.Skip(1))
        {
            startInfo.ArgumentList.Add(argument);
        }

        using var process = new Process { StartInfo = startInfo };
        try
        {
            if (!process.Start())
            {
                throw new InvalidOperationException($"Unable to start command: {arguments[0]}");
            }
        }
        catch (Win32Exception exception)
        {
            throw new FileNotFoundException($"Required executable not found on PATH: {arguments[0]}", exception);
        }

        process.StandardInput.Close();
        var stdout = new BoundedTextBuffer(MaximumOutputBytes);
        var stderr = new BoundedTextBuffer(MaximumOutputBytes);
        var outputTask = DrainAsync(process.StandardOutput, stdout, onOutput, CancellationToken.None);
        var errorTask = DrainAsync(process.StandardError, stderr, onOutput, CancellationToken.None);
        using var timeoutSource = new CancellationTokenSource(timeout);
        using var linkedSource = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken, timeoutSource.Token);

        try
        {
            await process.WaitForExitAsync(linkedSource.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            KillTree(process);
            await process.WaitForExitAsync(CancellationToken.None).ConfigureAwait(false);
            if (cancellationToken.IsCancellationRequested)
            {
                cancellationToken.ThrowIfCancellationRequested();
            }

            throw new TimeoutException($"Local command timed out after {timeout.TotalSeconds:g} seconds: {FormatCommand(arguments)}");
        }

        var drains = Task.WhenAll(outputTask, errorTask);
        if (await Task.WhenAny(drains, Task.Delay(TimeSpan.FromSeconds(2), CancellationToken.None)).ConfigureAwait(false) != drains)
        {
            KillTree(process);
            throw new InvalidOperationException(
                $"Local command exited while a descendant kept its output stream open: {FormatCommand(arguments)}");
        }

        await drains.ConfigureAwait(false);
        return new CommandResult(FormatCommand(arguments), process.ExitCode, stdout.ToString(), stderr.ToString());
    }

    public static bool IsExecutableAvailable(string name)
    {
        var path = Environment.GetEnvironmentVariable("PATH") ?? string.Empty;
        var extensions = OperatingSystem.IsWindows()
            ? (Environment.GetEnvironmentVariable("PATHEXT") ?? ".EXE;.CMD;.BAT").Split(';')
            : [string.Empty];
        return path.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries)
            .SelectMany(directory => extensions.Select(extension => Path.Combine(directory, name + extension)))
            .Any(File.Exists);
    }

    private static async Task DrainAsync(
        StreamReader reader,
        BoundedTextBuffer retained,
        Action<string>? onOutput,
        CancellationToken cancellationToken)
    {
        var buffer = new char[4096];
        var pending = new StringBuilder();
        while (true)
        {
            var count = await reader.ReadAsync(buffer.AsMemory(), cancellationToken).ConfigureAwait(false);
            if (count == 0)
            {
                break;
            }

            retained.Append(buffer.AsSpan(0, count));
            if (onOutput is null)
            {
                continue;
            }

            pending.Append(buffer, 0, count);
            EmitCompleteLines(pending, onOutput);
        }

        if (pending.Length > 0)
        {
            onOutput?.Invoke(pending.ToString().TrimEnd('\r', '\n'));
        }
    }

    private static void EmitCompleteLines(StringBuilder pending, Action<string> onOutput)
    {
        while (true)
        {
            var text = pending.ToString();
            var newline = text.IndexOfAny(['\r', '\n']);
            if (newline < 0)
            {
                if (pending.Length < MaximumPendingCharacters)
                {
                    return;
                }

                onOutput(text[..MaximumPendingCharacters]);
                pending.Remove(0, MaximumPendingCharacters);
                continue;
            }

            onOutput(text[..newline]);
            var remove = newline + 1;
            if (text[newline] == '\r' && remove < text.Length && text[remove] == '\n')
            {
                remove++;
            }

            pending.Remove(0, remove);
        }
    }

    private static void KillTree(Process process)
    {
        try
        {
            if (!process.HasExited)
            {
                process.Kill(entireProcessTree: true);
            }
        }
        catch (InvalidOperationException)
        {
            // The process exited between the check and termination.
        }
    }

    private static string FormatCommand(IEnumerable<string> arguments) => string.Join(' ', arguments);
}
