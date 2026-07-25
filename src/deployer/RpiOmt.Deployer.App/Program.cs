using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.LogicalTree;
using RpiOmt.Deployer.App.ViewModels;
using RpiOmt.Deployer.App.Views;

namespace RpiOmt.Deployer.App;

internal static class Program
{
    [STAThread]
    public static int Main(string[] args)
    {
        if (args.Length == 1 && string.Equals(args[0], "--self-test", StringComparison.Ordinal))
        {
            return RunSelfTest();
        }

        BuildAvaloniaApp().StartWithClassicDesktopLifetime(args, ShutdownMode.OnMainWindowClose);
        return 0;
    }

    public static AppBuilder BuildAvaloniaApp() =>
        AppBuilder.Configure<App>()
            .UsePlatformDetect()
            .WithInterFont()
            .LogToTrace();

    private static int RunSelfTest()
    {
        try
        {
            BuildAvaloniaApp().SetupWithoutStarting();
            var window = new MainWindow { DataContext = new MainViewModel(new SelfTestOperations(), Environment.CurrentDirectory) };
            var tabs = window.GetLogicalDescendants().OfType<TabItem>()
                .Select(tab => tab.Header?.ToString())
                .ToArray();
            var requiredTabs = new[] { "Deploy", "Manage", "Wi-Fi", "About" };
            if (!requiredTabs.SequenceEqual(tabs, StringComparer.Ordinal))
            {
                throw new InvalidOperationException("The Deploy, Manage, Wi-Fi, and About tabs were not constructed.");
            }

            var cancel = window.FindControl<Button>("CancelButton");
            if (cancel?.IsEffectivelyEnabled != false)
            {
                throw new InvalidOperationException("Cancel must be disabled while idle.");
            }

            Console.WriteLine("Raspberry Pi OMT Client Deployer self-test passed.");
            window.Close();
            return 0;
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine($"Self-test failed: {exception.Message}");
            return 1;
        }
    }
}
