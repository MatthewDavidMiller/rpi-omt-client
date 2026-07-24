using Avalonia.Controls;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Interactivity;
using Avalonia.Platform.Storage;
using RpiOmt.Deployer.App.ViewModels;

namespace RpiOmt.Deployer.App.Views;

public sealed partial class MainWindow : Window
{
    public MainWindow()
    {
        InitializeComponent();
        DataContextChanged += (_, _) => AttachViewModel();
        Closed += (_, _) => ViewModel?.Dispose();
    }

    private MainViewModel? ViewModel => DataContext as MainViewModel;

    private async void ChooseKey(object? sender, RoutedEventArgs eventArgs)
    {
        var files = await StorageProvider.OpenFilePickerAsync(new FilePickerOpenOptions
        {
            AllowMultiple = false,
            Title = "Select SSH private key",
        });
        var path = files.Count == 0 ? null : files[0].TryGetLocalPath();
        if (path is not null && ViewModel is not null)
        {
            ViewModel.KeyPath = path;
        }
    }

    private async void ChooseProject(object? sender, RoutedEventArgs eventArgs)
    {
        var folders = await StorageProvider.OpenFolderPickerAsync(new FolderPickerOpenOptions
        {
            AllowMultiple = false,
            Title = "Select project root",
        });
        var path = folders.Count == 0 ? null : folders[0].TryGetLocalPath();
        if (path is not null && ViewModel is not null)
        {
            ViewModel.ProjectRoot = path;
        }
    }

    private void OnClosing(object? sender, WindowClosingEventArgs eventArgs)
    {
        if (ViewModel is not null && !ViewModel.RequestWindowClose())
        {
            eventArgs.Cancel = true;
        }
    }

    private void AttachViewModel()
    {
        if (ViewModel is not null)
        {
            ViewModel.CloseRequested -= CloseRequested;
            ViewModel.CloseRequested += CloseRequested;
        }
    }

    private void CloseRequested(object? sender, EventArgs eventArgs) => Close();
}
