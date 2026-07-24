namespace RpiOmt.Deployer.Core;

internal sealed class ProgressRedactionService(Action<ProgressEventArgs> publish)
{
    private SecretRedactor _redactor = new([]);
    private string _stage = "idle";
    private bool _cancellable = true;

    public void Reset()
    {
        _redactor = new SecretRedactor([]);
        _stage = "idle";
        _cancellable = true;
    }

    public void Configure(PiConnection connection, params string[] additional) =>
        _redactor = new SecretRedactor(
            [connection.Password, connection.KeyPassphrase, connection.SudoPassword, .. additional]);

    public void SetStage(string stage, string message, bool cancellable = true)
    {
        _stage = stage;
        _cancellable = cancellable;
        Emit(message);
    }

    public void Emit(string message, string level = "info")
    {
        if (message.Length > 0)
        {
            publish(new ProgressEventArgs(
                _redactor.Redact(message),
                level,
                _stage,
                _cancellable));
        }
    }

    public CommandResult Redact(CommandResult result) => _redactor.Redact(result);
}
