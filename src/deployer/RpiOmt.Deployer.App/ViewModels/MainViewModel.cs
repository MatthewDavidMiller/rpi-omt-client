using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Runtime.CompilerServices;
using Avalonia;
using Avalonia.Styling;
using Avalonia.Threading;
using RpiOmt.Deployer.Core;

namespace RpiOmt.Deployer.App.ViewModels;

/// <summary>Theme selection offered in the title bar. <see cref="AppTheme.System"/>
/// leaves the variant at <see cref="ThemeVariant.Default"/> so the window follows the
/// operating system.</summary>
public enum AppTheme
{
    System,
    Light,
    Dark,
}

/// <remarks>The level is exposed as booleans rather than a brush so the activity log
/// is coloured by the theme dictionaries and stays readable in light mode.</remarks>
public sealed record LogLine(string Message, string Level)
{
    public bool IsWarning => Level == "warning";

    public bool IsError => Level == "error";
}

public sealed class MainViewModel : INotifyPropertyChanged, IDisposable
{
    public const int MaximumLogLines = 5000;
    private readonly IDeploymentOperations _operations;
    private CancellationTokenSource? _cancellation;
    private bool _closeAfterAction;
    private AppTheme _theme = AppTheme.System;
    private readonly string _appVersion = BuildInformation.Version;
    private readonly string _copyrightNotice = BuildInformation.Copyright;
    private readonly string _projectLicense = BuildInformation.ProjectLicense;
    private readonly string _thirdPartyNotices = BuildInformation.ThirdPartyNotices;

    public MainViewModel(IDeploymentOperations operations, string projectRoot)
    {
        _operations = operations;
        Deployment = new DeploymentViewModel(projectRoot);
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

    public string ActivityLogText => string.Join(Environment.NewLine, ActivityLog.Select(line => line.Message));

    public ConnectionViewModel Connection { get; } = new();

    public DeploymentViewModel Deployment { get; }

    public WifiViewModel Wifi { get; } = new();

    public OperationStateViewModel OperationState { get; } = new();

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

    public string Host { get => Connection.Host; set => SetSection(Connection.Host, value, newValue => Connection.Host = newValue); }

    public string Username { get => Connection.Username; set => SetSection(Connection.Username, value, newValue => Connection.Username = newValue); }

    public string Port { get => Connection.Port; set => SetSection(Connection.Port, value, newValue => Connection.Port = newValue); }

    public string Password { get => Connection.Password; set => SetSection(Connection.Password, value, newValue => Connection.Password = newValue); }

    public string KeyPath { get => Connection.KeyPath; set => SetSection(Connection.KeyPath, value, newValue => Connection.KeyPath = newValue); }

    public string KeyPassphrase { get => Connection.KeyPassphrase; set => SetSection(Connection.KeyPassphrase, value, newValue => Connection.KeyPassphrase = newValue); }

    public string SudoPassword { get => Connection.SudoPassword; set => SetSection(Connection.SudoPassword, value, newValue => Connection.SudoPassword = newValue); }

    public bool IsKeyAuthentication
    {
        get => Connection.IsKeyAuthentication;
        set
        {
            if (SetSection(Connection.IsKeyAuthentication, value, newValue => Connection.IsKeyAuthentication = newValue))
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

    public string ProjectRoot { get => Deployment.ProjectRoot; set => SetSection(Deployment.ProjectRoot, value, newValue => Deployment.ProjectRoot = newValue); }

    public string RemoteDirectory { get => Deployment.RemoteDirectory; set => SetSection(Deployment.RemoteDirectory, value, newValue => Deployment.RemoteDirectory = newValue); }

    public bool BuildImage { get => Deployment.BuildImage; set => SetSection(Deployment.BuildImage, value, newValue => Deployment.BuildImage = newValue); }

    public string WifiSsid { get => Wifi.Ssid; set => SetSection(Wifi.Ssid, value, newValue => Wifi.Ssid = newValue); }

    public string WifiPassword { get => Wifi.Password; set => SetSection(Wifi.Password, value, newValue => Wifi.Password = newValue); }

    public bool WifiConnect { get => Wifi.Connect; set => SetSection(Wifi.Connect, value, newValue => Wifi.Connect = newValue); }

    public bool IsBusy { get => OperationState.IsBusy; private set { if (SetSection(OperationState.IsBusy, value, newValue => OperationState.IsBusy = newValue)) OnPropertyChanged(nameof(InputsEnabled)); } }

    public bool InputsEnabled => !IsBusy;

    public bool CanCancel { get => OperationState.CanCancel; private set { if (SetSection(OperationState.CanCancel, value, newValue => OperationState.CanCancel = newValue)) CancelCommand.RaiseCanExecuteChanged(); } }

    public string Activity { get => OperationState.Activity; private set => SetSection(OperationState.Activity, value, newValue => OperationState.Activity = newValue); }

    public string ValidationMessage { get => OperationState.ValidationMessage; private set => SetSection(OperationState.ValidationMessage, value, newValue => OperationState.ValidationMessage = newValue); }

    public IReadOnlyList<AppTheme> ThemeOptions { get; } = [AppTheme.System, AppTheme.Light, AppTheme.Dark];

    public AppTheme Theme
    {
        get => _theme;
        set
        {
            if (!Set(ref _theme, value))
            {
                return;
            }

            if (Application.Current is not null)
            {
                Application.Current.RequestedThemeVariant = value switch
                {
                    AppTheme.Light => ThemeVariant.Light,
                    AppTheme.Dark => ThemeVariant.Dark,
                    _ => ThemeVariant.Default,
                };
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

        if (!OperationState.Controller.Cancellable)
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

    public void ReportLogTransferError(string action, string detail) =>
        Append($"Could not {action} the activity log: {detail}", "error");

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
        if (!OperationState.Controller.Start())
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
            OperationState.Controller.Finish(RpiOmt.Deployer.Core.OperationState.Succeeded);
            Append("Action completed.");
        }
        catch (OperationCanceledException)
        {
            OperationState.Controller.Finish(RpiOmt.Deployer.Core.OperationState.Cancelled);
            Append("Operation cancelled by user.");
        }
        catch (Exception exception)
        {
            OperationState.Controller.Finish(RpiOmt.Deployer.Core.OperationState.Failed);
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
        return new ActionSnapshot(
            Connection.Capture(),
            Deployment.Capture(),
            Wifi.Capture());
    }

    private void RequestCancellation()
    {
        if (!OperationState.Controller.RequestCancellation() || _cancellation is null)
        {
            if (IsBusy && !OperationState.Controller.Cancellable)
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
        OperationState.Controller.Progress(progress);
        CanCancel = IsBusy &&
            progress.Cancellable &&
            OperationState.Controller.State != RpiOmt.Deployer.Core.OperationState.Cancelling;
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
        var state = OperationState.Controller.State.ToString();
        Activity = $"{state} · {OperationState.Controller.Stage.Replace('-', ' ')}";
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

    private bool SetSection<T>(
        T current,
        T value,
        Action<T> assign,
        [CallerMemberName] string? propertyName = null)
    {
        if (EqualityComparer<T>.Default.Equals(current, value))
        {
            return false;
        }

        assign(value);
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
