using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Globalization;
using System.Runtime.CompilerServices;
using Avalonia;
using Avalonia.Styling;
using Avalonia.Threading;
using Avalonia.Media;
using RpiOmt.Deployer.Core;

namespace RpiOmt.Deployer.App.ViewModels;

public sealed record LogLine(string Message, string Level)
{
    public IBrush Color => Brush.Parse(Level switch
    {
        "warning" => "#F1BD61",
        "error" => "#FF8A8A",
        _ => "#E5E7EB",
    });
}

public sealed class MainViewModel : INotifyPropertyChanged, IDisposable
{
    public const int MaximumLogLines = 5000;
    private readonly IDeploymentOperations _operations;
    private readonly ActionController _operation = new();
    private CancellationTokenSource? _cancellation;
    private bool _closeAfterAction;
    private string _host = string.Empty;
    private string _username = "pi";
    private string _port = "22";
    private string _password = string.Empty;
    private string _keyPath = string.Empty;
    private string _keyPassphrase = string.Empty;
    private string _sudoPassword = string.Empty;
    private bool _keyAuthentication;
    private string _projectRoot;
    private string _remoteDirectory = "/opt/omt-client";
    private bool _buildImage = true;
    private string _wifiSsid = string.Empty;
    private string _wifiPassword = string.Empty;
    private bool _wifiConnect = true;
    private bool _isBusy;
    private bool _canCancel;
    private bool _isDark;
    private string _activity = "Idle · idle";
    private string _validationMessage = string.Empty;
    private readonly string _appVersion = BuildInformation.Version;
    private readonly string _copyrightNotice = BuildInformation.Copyright;
    private readonly string _projectLicense = BuildInformation.ProjectLicense;
    private readonly string _thirdPartyNotices = BuildInformation.ThirdPartyNotices;

    public MainViewModel(IDeploymentOperations operations, string projectRoot)
    {
        _operations = operations;
        _projectRoot = projectRoot;
        _operations.Progress += OnProgress;
        DeployCommand = ActionCommand(() => RunAsync((snapshot, token) => _operations.DeployAsync(snapshot, token)));
        InstallPrerequisitesCommand = ActionCommand(() => RunAsync(
            (_, token) => _operations.InstallPrerequisitesAsync(ProjectRoot, token),
            validateConnection: false,
            validateOptions: false));
        TestConnectionCommand = ActionCommand(() => RunAsync((snapshot, token) => _operations.TestConnectionAsync(snapshot.Connection, token), validateOptions: false));
        StatusCommand = ActionCommand(() => RunTextAsync((snapshot, token) => _operations.FetchStatusAsync(snapshot.Connection, snapshot.Options.RemoteDirectory, token)));
        LogsCommand = ActionCommand(() => RunTextAsync((snapshot, token) => _operations.FetchLogsAsync(snapshot.Connection, snapshot.Options.RemoteDirectory, token)));
        RestartCommand = ActionCommand(() => RunTextAsync((snapshot, token) => _operations.RestartServiceAsync(snapshot.Connection, snapshot.Options.RemoteDirectory, token)));
        ApplyWifiCommand = ActionCommand(() => RunAsync(
            (snapshot, token) => _operations.ApplyWifiAsync(snapshot.Connection, snapshot.Wifi, token),
            validateOptions: false,
            validateWifi: true));
        CancelCommand = new DelegateCommand(RequestCancellation, () => CanCancel);
        ClearLogCommand = new DelegateCommand(ActivityLog.Clear);
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    public event EventHandler? CloseRequested;

    public ObservableCollection<LogLine> ActivityLog { get; } = [];

    public string AppVersion => _appVersion;

    public string CopyrightNotice => _copyrightNotice;

    public string ProjectLicense => _projectLicense;

    public string ThirdPartyNotices => _thirdPartyNotices;

    public AsyncCommand DeployCommand { get; }

    public AsyncCommand InstallPrerequisitesCommand { get; }

    public AsyncCommand TestConnectionCommand { get; }

    public AsyncCommand StatusCommand { get; }

    public AsyncCommand LogsCommand { get; }

    public AsyncCommand RestartCommand { get; }

    public AsyncCommand ApplyWifiCommand { get; }

    public DelegateCommand CancelCommand { get; }

    public DelegateCommand ClearLogCommand { get; }

    public string Host { get => _host; set => Set(ref _host, value); }

    public string Username { get => _username; set => Set(ref _username, value); }

    public string Port { get => _port; set => Set(ref _port, value); }

    public string Password { get => _password; set => Set(ref _password, value); }

    public string KeyPath { get => _keyPath; set => Set(ref _keyPath, value); }

    public string KeyPassphrase { get => _keyPassphrase; set => Set(ref _keyPassphrase, value); }

    public string SudoPassword { get => _sudoPassword; set => Set(ref _sudoPassword, value); }

    public bool IsKeyAuthentication
    {
        get => _keyAuthentication;
        set
        {
            if (Set(ref _keyAuthentication, value))
            {
                OnPropertyChanged(nameof(IsPasswordAuthentication));
            }
        }
    }

    public bool IsPasswordAuthentication
    {
        get => !IsKeyAuthentication;
        set
        {
            if (value)
            {
                IsKeyAuthentication = false;
            }
        }
    }

    public string ProjectRoot { get => _projectRoot; set => Set(ref _projectRoot, value); }

    public string RemoteDirectory { get => _remoteDirectory; set => Set(ref _remoteDirectory, value); }

    public bool BuildImage { get => _buildImage; set => Set(ref _buildImage, value); }

    public string WifiSsid { get => _wifiSsid; set => Set(ref _wifiSsid, value); }

    public string WifiPassword { get => _wifiPassword; set => Set(ref _wifiPassword, value); }

    public bool WifiConnect { get => _wifiConnect; set => Set(ref _wifiConnect, value); }

    public bool IsBusy { get => _isBusy; private set { if (Set(ref _isBusy, value)) OnPropertyChanged(nameof(InputsEnabled)); } }

    public bool InputsEnabled => !IsBusy;

    public bool CanCancel { get => _canCancel; private set { if (Set(ref _canCancel, value)) CancelCommand.RaiseCanExecuteChanged(); } }

    public string Activity { get => _activity; private set => Set(ref _activity, value); }

    public string ValidationMessage { get => _validationMessage; private set => Set(ref _validationMessage, value); }

    public bool IsDark
    {
        get => _isDark;
        set
        {
            if (!Set(ref _isDark, value))
            {
                return;
            }

            if (Application.Current is not null)
            {
                Application.Current.RequestedThemeVariant = value ? ThemeVariant.Dark : ThemeVariant.Light;
            }
        }
    }

    public static MainViewModel CreateDefault(string projectRoot)
    {
        var runner = new ProcessCommandRunner();
        return new MainViewModel(
            new DeploymentOperations(runner, new SshNetRemoteClientFactory(runner), new ArtifactSnapshotProvider()),
            projectRoot);
    }

    public bool RequestWindowClose()
    {
        if (!IsBusy)
        {
            return true;
        }

        if (!_operation.Cancellable)
        {
            Append("Window close is disabled while the privileged installer or Wi-Fi mutation is running.", "warning");
            return false;
        }

        _closeAfterAction = true;
        RequestCancellation();
        return false;
    }

    public void Dispose()
    {
        _operations.Progress -= OnProgress;
        _cancellation?.Cancel();
        _cancellation?.Dispose();
        _cancellation = null;
    }

    private AsyncCommand ActionCommand(Func<Task> action) => new(action, () => !IsBusy);

    private async Task RunTextAsync(Func<ActionSnapshot, CancellationToken, Task<string>> action)
    {
        await RunAsync(async (snapshot, token) =>
        {
            var value = await action(snapshot, token).ConfigureAwait(false);
            if (!string.IsNullOrWhiteSpace(value))
            {
                Post(() => Append(RedactSnapshot(snapshot, value)));
            }
        }).ConfigureAwait(true);
    }

    private async Task RunAsync(
        Func<ActionSnapshot, CancellationToken, Task> action,
        bool validateConnection = true,
        bool validateOptions = true,
        bool validateWifi = false)
    {
        if (IsBusy)
        {
            return;
        }

        ActionSnapshot snapshot;
        try
        {
            snapshot = CaptureSnapshot();
        }
        catch (FormatException)
        {
            ValidationMessage = "SSH port must be an ASCII number between 1 and 65535.";
            return;
        }

        var errors = new List<string>();
        if (validateConnection)
        {
            errors.AddRange(InputValidation.ValidateConnection(snapshot.Connection));
        }

        if (validateOptions)
        {
            errors.AddRange(InputValidation.ValidateOptions(snapshot.Options));
        }

        if (validateWifi)
        {
            var wifiError = InputValidation.WifiSsidError(snapshot.Wifi.Ssid) ?? InputValidation.WifiPasswordError(snapshot.Wifi.Password);
            if (wifiError is not null)
            {
                errors.Add(wifiError);
            }
        }

        if (errors.Count > 0)
        {
            ValidationMessage = string.Join(Environment.NewLine, errors);
            return;
        }

        ValidationMessage = string.Empty;
        if (!_operation.Start())
        {
            return;
        }

        _cancellation = new CancellationTokenSource();
        IsBusy = true;
        CanCancel = true;
        RaiseActionCommands();
        ShowActivity();
        Append("Starting...");
        try
        {
            await action(snapshot, _cancellation.Token).ConfigureAwait(true);
            _operation.Finish(OperationState.Succeeded);
            Append("Action completed.");
        }
        catch (OperationCanceledException)
        {
            _operation.Finish(OperationState.Cancelled);
            Append("Operation cancelled by user.");
        }
        catch (Exception exception)
        {
            _operation.Finish(OperationState.Failed);
            Append(RedactSnapshot(snapshot, exception.Message), "error");
        }
        finally
        {
            _cancellation.Dispose();
            _cancellation = null;
            IsBusy = false;
            CanCancel = false;
            RaiseActionCommands();
            ShowActivity();
            if (_closeAfterAction)
            {
                CloseRequested?.Invoke(this, EventArgs.Empty);
            }
        }
    }

    private ActionSnapshot CaptureSnapshot()
    {
        if (Port.Length == 0 || !Port.All(character => character is >= '0' and <= '9'))
        {
            throw new FormatException();
        }

        var port = int.Parse(Port, NumberStyles.None, CultureInfo.InvariantCulture);
        var connection = new PiConnection(
            Host.Trim(),
            Username.Trim(),
            port,
            IsKeyAuthentication ? AuthMethod.Key : AuthMethod.Password,
            IsKeyAuthentication ? string.Empty : Password,
            string.IsNullOrWhiteSpace(KeyPath) ? null : KeyPath,
            IsKeyAuthentication ? KeyPassphrase : string.Empty,
            SudoPassword);
        return new ActionSnapshot(
            connection,
            new DeployOptions(ProjectRoot, RemoteDirectory.Trim(), BuildImage: BuildImage),
            new WifiSettings(WifiSsid, WifiPassword, WifiConnect));
    }

    private void RequestCancellation()
    {
        if (!_operation.RequestCancellation() || _cancellation is null)
        {
            if (IsBusy && !_operation.Cancellable)
            {
                Append("Cancellation is disabled while the privileged installer or Wi-Fi mutation is running.", "warning");
            }

            return;
        }

        _cancellation.Cancel();
        CanCancel = false;
        ShowActivity();
        Append("Cancellation requested; stopping the active safe stage...");
    }

    private void OnProgress(object? sender, ProgressEventArgs progress) => Post(() =>
    {
        _operation.Progress(progress);
        CanCancel = IsBusy && progress.Cancellable && _operation.State != OperationState.Cancelling;
        ShowActivity();
        Append(progress.Message, progress.Level);
    });

    private void Append(string message, string level = "info")
    {
        foreach (var line in message.Replace("\r\n", "\n", StringComparison.Ordinal).Split('\n'))
        {
            ActivityLog.Add(new LogLine(line, level));
        }

        while (ActivityLog.Count > MaximumLogLines)
        {
            ActivityLog.RemoveAt(0);
        }
    }

    private void ShowActivity()
    {
        var state = _operation.State.ToString();
        Activity = $"{state} · {_operation.Stage.Replace('-', ' ')}";
    }

    private void RaiseActionCommands()
    {
        DeployCommand.RaiseCanExecuteChanged();
        InstallPrerequisitesCommand.RaiseCanExecuteChanged();
        TestConnectionCommand.RaiseCanExecuteChanged();
        StatusCommand.RaiseCanExecuteChanged();
        LogsCommand.RaiseCanExecuteChanged();
        RestartCommand.RaiseCanExecuteChanged();
        ApplyWifiCommand.RaiseCanExecuteChanged();
    }

    private static string RedactSnapshot(ActionSnapshot snapshot, string value) =>
        new SecretRedactor([
            snapshot.Connection.Password,
            snapshot.Connection.KeyPassphrase,
            snapshot.Connection.SudoPassword,
            snapshot.Wifi.Password,
        ]).Redact(value);

    private static void Post(Action action)
    {
        if (Dispatcher.UIThread.CheckAccess())
        {
            action();
        }
        else
        {
            Dispatcher.UIThread.Post(action);
        }
    }

    private bool Set<T>(ref T field, T value, [CallerMemberName] string? propertyName = null)
    {
        if (EqualityComparer<T>.Default.Equals(field, value))
        {
            return false;
        }

        field = value;
        OnPropertyChanged(propertyName);
        return true;
    }

    private void OnPropertyChanged([CallerMemberName] string? propertyName = null) =>
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
}

internal sealed class SelfTestOperations : IDeploymentOperations
{
    public event EventHandler<ProgressEventArgs>? Progress;

    public Task DeployAsync(ActionSnapshot snapshot, CancellationToken cancellationToken) => Task.CompletedTask;
    public Task InstallPrerequisitesAsync(string projectRoot, CancellationToken cancellationToken) => Task.CompletedTask;
    public Task TestConnectionAsync(PiConnection connection, CancellationToken cancellationToken) => Task.CompletedTask;
    public Task<string> FetchStatusAsync(PiConnection connection, string remoteDirectory, CancellationToken cancellationToken) => Task.FromResult(string.Empty);
    public Task<string> FetchLogsAsync(PiConnection connection, string remoteDirectory, CancellationToken cancellationToken) => Task.FromResult(string.Empty);
    public Task<string> RestartServiceAsync(PiConnection connection, string remoteDirectory, CancellationToken cancellationToken) => Task.FromResult(string.Empty);
    public Task ApplyWifiAsync(PiConnection connection, WifiSettings settings, CancellationToken cancellationToken) => Task.CompletedTask;

    internal void Report(ProgressEventArgs value) => Progress?.Invoke(this, value);
}
