using System.Globalization;
using System.Text;
using Avalonia.Controls;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Input.Platform;
using Avalonia.Interactivity;
using Avalonia.Platform.Storage;
using RpiOmt.Deployer.App.ViewModels;

namespace RpiOmt.Deployer.App.Views;

public sealed partial class MainWindow : Window
{
    private static readonly FilePickerFileType LogFileType = new("Text log")
    {
        Patterns = ["*.txt"],
        MimeTypes = ["text/plain"],
    };

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

    private async void CopyLog(object? sender, RoutedEventArgs eventArgs)
    {
        if (ViewModel is null)
        {
            return;
        }

        try
        {
            var clipboard = Clipboard ?? throw new InvalidOperationException("The clipboard is unavailable.");
            await clipboard.SetTextAsync(ViewModel.ActivityLogText);
        }
        catch (Exception exception)
        {
            ViewModel.ReportLogTransferError("copy", exception.Message);
        }
    }

    private async void ExportLog(object? sender, RoutedEventArgs eventArgs)
    {
        if (ViewModel is null)
        {
            return;
        }

        try
        {
            var file = await StorageProvider.SaveFilePickerAsync(new FilePickerSaveOptions
            {
                Title = "Export activity log",
                SuggestedFileName = $"rpi-omt-deployer-{DateTime.Now.ToString("yyyyMMdd-HHmmss", CultureInfo.InvariantCulture)}.txt",
                DefaultExtension = "txt",
                FileTypeChoices = [LogFileType],
                SuggestedFileType = LogFileType,
                ShowOverwritePrompt = true,
            });
            if (file is null)
            {
                return;
            }

            await using var stream = await file.OpenWriteAsync();
            if (!stream.CanSeek)
            {
                throw new IOException("The selected destination does not support replacing a file.");
            }

            stream.SetLength(0);
            await using var writer = new StreamWriter(stream, new UTF8Encoding(encoderShouldEmitUTF8Identifier: false));
            await writer.WriteAsync(ViewModel.ActivityLogText);
        }
        catch (Exception exception)
        {
            ViewModel.ReportLogTransferError("export", exception.Message);
        }
    }

    private void ShowActivity(object? sender, RoutedEventArgs eventArgs) =>
        TaskTabs.SelectedIndex = MainViewModel.ActivityTabIndex;

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
