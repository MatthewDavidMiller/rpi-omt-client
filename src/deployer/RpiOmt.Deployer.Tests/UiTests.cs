using Avalonia;
using Avalonia.Controls;
using Avalonia.Headless;
using Avalonia.Headless.XUnit;
using Avalonia.LogicalTree;
using Avalonia.Styling;
using RpiOmt.Deployer.App;
using RpiOmt.Deployer.App.ViewModels;
using RpiOmt.Deployer.App.Views;
using RpiOmt.Deployer.Core;

[assembly: AvaloniaTestApplication(typeof(RpiOmt.Deployer.Tests.TestAppBuilder))]

namespace RpiOmt.Deployer.Tests;

public static class TestAppBuilder
{
    public static AppBuilder BuildAvaloniaApp() => AppBuilder
        .Configure<RpiOmt.Deployer.App.App>()
        .UseHeadless(new AvaloniaHeadlessPlatformOptions());
}

public sealed class AvaloniaUiTests
{
    [AvaloniaFact]
    public void RealControlTreeContainsTabsActionsAndIdleCancellation()
    {
        using var viewModel = new MainViewModel(new FakeOperations(), Environment.CurrentDirectory);
        var window = new MainWindow { DataContext = viewModel };
        window.Show();
        var tabItems = window.GetLogicalDescendants().OfType<TabItem>().ToArray();
        Assert.Equal(["Deploy", "Manage", "Wi-Fi", "About"], tabItems.Select(item => item.Header?.ToString() ?? string.Empty));
        Assert.IsType<ScrollViewer>(tabItems.Single(item => item.Header?.ToString() == "Wi-Fi").Content);
        Assert.IsType<ScrollViewer>(tabItems.Single(item => item.Header?.ToString() == "About").Content);
        Assert.Contains("Matthew David Miller", viewModel.CopyrightNotice, StringComparison.Ordinal);
        Assert.Contains("MIT License", viewModel.ProjectLicense, StringComparison.Ordinal);
        Assert.Contains("THIRD-PARTY", viewModel.ThirdPartyNotices, StringComparison.Ordinal);
        var splitters = window.GetLogicalDescendants().OfType<GridSplitter>().ToArray();
        Assert.Equal(2, splitters.Length);
        Assert.All(splitters, splitter =>
        {
            Assert.Equal(GridResizeDirection.Rows, splitter.ResizeDirection);
            Assert.Equal(GridResizeBehavior.PreviousAndNext, splitter.ResizeBehavior);
        });
        foreach (var name in new[]
        {
            "InstallPrerequisitesButton", "DeployButton", "TestSshButton", "StatusButton",
            "LogsButton", "RestartButton", "ApplyWifiButton",
        })
        {
            Assert.NotNull(window.FindControl<Button>(name));
        }

        Assert.False(window.FindControl<Button>("CancelButton")!.IsEffectivelyEnabled);
        Assert.StartsWith("Idle", viewModel.Activity, StringComparison.Ordinal);

        // Feedback surfaces stay out of the way until they have something to say.
        Assert.False(window.FindControl<Border>("ValidationInfoBar")!.IsVisible);
        var progress = window.GetLogicalDescendants().OfType<ProgressBar>().Single();
        Assert.False(progress.IsVisible);
        Assert.True(progress.IsIndeterminate);

        viewModel.DeployCommand.Execute(null);
        Assert.NotEmpty(viewModel.ValidationMessage);
        Assert.True(window.FindControl<Border>("ValidationInfoBar")!.IsVisible);
        window.Close();
    }

    [AvaloniaFact]
    public async Task ActionUsesImmutableSnapshotAndMutualExclusion()
    {
        var operations = new FakeOperations { BlockDeploy = true };
        using var viewModel = ValidViewModel(operations);
        viewModel.Host = "original.local";
        viewModel.DeployCommand.Execute(null);
        await operations.Started.Task.WaitAsync(TimeSpan.FromSeconds(2), TestContext.Current.CancellationToken);
        viewModel.Host = "changed.local";
        viewModel.DeployCommand.Execute(null);
        Assert.True(viewModel.IsBusy);
        Assert.False(viewModel.InputsEnabled);
        Assert.Equal("original.local", operations.Snapshots.Single().Connection.Host);
        operations.Release.TrySetResult();
        await WaitUntilAsync(() => !viewModel.IsBusy);
        Assert.Single(operations.Snapshots);
        Assert.Contains(viewModel.ActivityLog, line => line.Message == "Action completed.");
    }

    [AvaloniaFact]
    public async Task NonCancellableProgressDisablesCancelAndClose()
    {
        var operations = new FakeOperations { BlockDeploy = true };
        using var viewModel = ValidViewModel(operations);
        viewModel.DeployCommand.Execute(null);
        await operations.Started.Task.WaitAsync(TimeSpan.FromSeconds(2), TestContext.Current.CancellationToken);
        operations.Report(new ProgressEventArgs("Installing", stage: "installer", cancellable: false));
        Assert.False(viewModel.CanCancel);
        Assert.False(viewModel.RequestWindowClose());
        var warning = Assert.Single(viewModel.ActivityLog, line => line.Message.Contains("close is disabled", StringComparison.Ordinal));
        Assert.True(warning.IsWarning);
        Assert.False(warning.IsError);
        operations.Release.TrySetResult();
        await WaitUntilAsync(() => !viewModel.IsBusy);
    }

    [AvaloniaFact]
    public async Task CancellationAndCloseWaitForSafeActionToFinish()
    {
        var operations = new FakeOperations { WaitForCancellation = true };
        using var viewModel = ValidViewModel(operations);
        var closeRequested = false;
        viewModel.CloseRequested += (_, _) => closeRequested = true;
        viewModel.DeployCommand.Execute(null);
        await operations.Started.Task.WaitAsync(TimeSpan.FromSeconds(2), TestContext.Current.CancellationToken);
        Assert.False(viewModel.RequestWindowClose());
        await WaitUntilAsync(() => !viewModel.IsBusy);
        Assert.True(closeRequested);
        Assert.Contains(viewModel.ActivityLog, line => line.Message.Contains("cancelled", StringComparison.OrdinalIgnoreCase));
    }

    [AvaloniaFact]
    public async Task ValidationThemeAndActivityLogStateAreVisibleAndBounded()
    {
        var operations = new FakeOperations();
        using var viewModel = new MainViewModel(operations, "/missing");
        viewModel.DeployCommand.Execute(null);
        Assert.NotEmpty(viewModel.ValidationMessage);
        Assert.Equal([AppTheme.System, AppTheme.Light, AppTheme.Dark], viewModel.ThemeOptions);
        Assert.Equal(AppTheme.System, viewModel.Theme);
        viewModel.Theme = AppTheme.Dark;
        Assert.Equal(ThemeVariant.Dark, Application.Current!.RequestedThemeVariant);
        viewModel.Theme = AppTheme.Light;
        Assert.Equal(ThemeVariant.Light, Application.Current.RequestedThemeVariant);
        viewModel.Theme = AppTheme.System;
        Assert.Equal(ThemeVariant.Default, Application.Current.RequestedThemeVariant);
        using var valid = ValidViewModel(operations);
        operations.ProgressCount = 5005;
        valid.TestConnectionCommand.Execute(null);
        await WaitUntilAsync(() => !valid.IsBusy);
        Assert.Equal(MainViewModel.MaximumLogLines, valid.ActivityLog.Count);
        valid.ClearLogCommand.Execute(null);
        Assert.Empty(valid.ActivityLog);
    }

    private static MainViewModel ValidViewModel(FakeOperations operations)
    {
        var viewModel = new MainViewModel(operations, Environment.CurrentDirectory)
        {
            Host = "pi.local",
            Username = "pi",
            Password = "ssh-password",
            BuildImage = false,
        };
        return viewModel;
    }

    private static async Task WaitUntilAsync(Func<bool> condition)
    {
        var deadline = DateTime.UtcNow + TimeSpan.FromSeconds(3);
        while (!condition() && DateTime.UtcNow < deadline)
        {
            await Task.Delay(10);
        }

        Assert.True(condition());
    }
}

internal sealed class FakeOperations : IDeploymentOperations
{
    public event EventHandler<ProgressEventArgs>? Progress;
    public List<ActionSnapshot> Snapshots { get; } = [];
    public TaskCompletionSource Started { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);
    public TaskCompletionSource Release { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);
    public bool BlockDeploy { get; init; }
    public bool WaitForCancellation { get; init; }
    public int ProgressCount { get; set; }

    public async Task DeployAsync(ActionSnapshot snapshot, CancellationToken cancellationToken)
    {
        Snapshots.Add(snapshot);
        Started.TrySetResult();
        if (WaitForCancellation)
        {
            await Task.Delay(Timeout.InfiniteTimeSpan, cancellationToken);
        }
        else if (BlockDeploy)
        {
            await Release.Task;
        }
    }

    public Task InstallPrerequisitesAsync(string projectRoot, CancellationToken cancellationToken) => Task.CompletedTask;

    public Task TestConnectionAsync(PiConnection connection, CancellationToken cancellationToken)
    {
        for (var index = 0; index < ProgressCount; index++)
        {
            Report(new ProgressEventArgs($"line-{index}", stage: "test"));
        }

        return Task.CompletedTask;
    }

    public Task<string> FetchStatusAsync(PiConnection connection, string remoteDirectory, CancellationToken cancellationToken) => Task.FromResult("status");
    public Task<string> FetchLogsAsync(PiConnection connection, string remoteDirectory, CancellationToken cancellationToken) => Task.FromResult("logs");
    public Task<string> RestartServiceAsync(PiConnection connection, string remoteDirectory, CancellationToken cancellationToken) => Task.FromResult("restart");
    public Task ApplyWifiAsync(PiConnection connection, WifiSettings settings, CancellationToken cancellationToken) => Task.CompletedTask;
    public void Report(ProgressEventArgs value) => Progress?.Invoke(this, value);
}
