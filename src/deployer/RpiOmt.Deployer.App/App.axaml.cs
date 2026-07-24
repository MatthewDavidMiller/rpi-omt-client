using Avalonia;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Markup.Xaml;
using RpiOmt.Deployer.App.ViewModels;
using RpiOmt.Deployer.App.Views;

namespace RpiOmt.Deployer.App;

public sealed partial class App : Application
{
    public override void Initialize() => AvaloniaXamlLoader.Load(this);

    public override void OnFrameworkInitializationCompleted()
    {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            desktop.MainWindow = new MainWindow
            {
                DataContext = MainViewModel.CreateDefault(Environment.CurrentDirectory),
            };
        }

        base.OnFrameworkInitializationCompleted();
    }
}
