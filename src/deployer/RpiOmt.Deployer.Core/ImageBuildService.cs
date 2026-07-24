using System.Diagnostics.CodeAnalysis;
using System.Formats.Tar;
using System.Security.Cryptography;

namespace RpiOmt.Deployer.Core;

internal sealed class ImageBuildService(
    ICommandRunner commandRunner,
    ProgressRedactionService progress)
{
    public const string Arm64CheckImage =
        "debian:bookworm-slim@sha256:4724b8cc51e33e398f0e2e15e18d5ec2851ff0c2280647e1310bc1642182655d";
    public const string BinfmtImage =
        "tonistiigi/binfmt@sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0";

    private static readonly TimeSpan EmulatorTimeout = TimeSpan.FromSeconds(120);
    private static readonly TimeSpan PrerequisiteTimeout = TimeSpan.FromMinutes(5);
    private static readonly TimeSpan BuildTimeout = TimeSpan.FromHours(1);

    [ExcludeFromCodeCoverage(
        Justification = "Exercises privileged Docker/binfmt and local integration boundaries.")]
    public async Task InstallPrerequisitesAsync(
        string projectRoot,
        CancellationToken cancellationToken)
    {
        progress.SetStage(
            "prerequisites",
            "Installing Docker ARM64 emulation support...");
        DeploymentGuards.RequireExecutable("docker");
        CommandResult result = await commandRunner.RunAsync(
            ["docker", "run", "--privileged", "--rm", BinfmtImage, "--install", "arm64"],
            projectRoot,
            line => progress.Emit(line),
            PrerequisiteTimeout,
            cancellationToken).ConfigureAwait(false);
        DeploymentGuards.RequireSuccess(result);
        await VerifyEmulationAsync(projectRoot, cancellationToken).ConfigureAwait(false);
        progress.Emit("Docker ARM64 emulation is ready.");
    }

    [ExcludeFromCodeCoverage(
        Justification = "Exercises Docker buildx and atomic archive publication.")]
    public async Task BuildAsync(
        DeployOptions options,
        CancellationToken cancellationToken)
    {
        progress.SetStage("build-preflight", "Checking local build prerequisites...");
        DeploymentGuards.RequireExecutable("docker");
        await VerifyEmulationAsync(options.ProjectRoot, cancellationToken)
            .ConfigureAwait(false);
        string version = await new VersionDetector(commandRunner).DetectAsync(
            options.ProjectRoot,
            null,
            cancellationToken).ConfigureAwait(false);
        progress.SetStage("build", "Building ARM64 Docker image...");
        string stagedPath = Path.Combine(
            options.ProjectRoot,
            $".{options.TarballName}.{RandomNumberGenerator.GetHexString(16)}.tmp");
        try
        {
            CommandResult result = await commandRunner.RunAsync(
                [
                    "docker", "buildx", "build", "--platform", "linux/arm64",
                    "--build-arg", $"RPI_OMT_CLIENT_VERSION={version}",
                    "--output", $"type=docker,dest={stagedPath}",
                    "--file", "deploy/Dockerfile",
                    "-t", options.ImageName, ".",
                ],
                options.ProjectRoot,
                line => progress.Emit(line),
                BuildTimeout,
                cancellationToken).ConfigureAwait(false);
            DeploymentGuards.RequireSuccess(result);
            cancellationToken.ThrowIfCancellationRequested();
            VerifyTarArchive(stagedPath);
            await using (var flush = new FileStream(
                stagedPath,
                FileMode.Open,
                FileAccess.ReadWrite,
                FileShare.Read))
            {
                flush.Flush(flushToDisk: true);
            }

            File.Move(stagedPath, options.TarballPath, overwrite: true);
            progress.Emit($"Published verified artifact: {options.TarballName}");
        }
        finally
        {
            if (File.Exists(stagedPath))
            {
                File.Delete(stagedPath);
            }
        }
    }

    [ExcludeFromCodeCoverage(
        Justification = "Exercises the external ARM64 container runtime.")]
    private async Task VerifyEmulationAsync(
        string projectRoot,
        CancellationToken cancellationToken)
    {
        progress.SetStage("emulation-check", "Checking Docker ARM64 emulation...");
        CommandResult result = await commandRunner.RunAsync(
            [
                "docker", "run", "--rm", "--platform", "linux/arm64",
                "--entrypoint", "/bin/sh", Arm64CheckImage,
                "-c", "test \"$(uname -m)\" = \"aarch64\"",
            ],
            projectRoot,
            line => progress.Emit(line),
            EmulatorTimeout,
            cancellationToken).ConfigureAwait(false);
        if (!result.IsSuccess)
        {
            throw new DeploymentException(
                "Docker ARM64 emulation is not ready. Use Install Prerequisites, " +
                "ensure Docker Desktop's Linux engine is running, and retry.");
        }
    }

    [ExcludeFromCodeCoverage(
        Justification = "Only receives Docker archives and is covered by publish tests.")]
    private static void VerifyTarArchive(string path)
    {
        if (!ArtifactSnapshots.IsRegularFile(path) || new FileInfo(path).Length == 0)
        {
            throw new DeploymentException(
                "Docker reported success but did not produce a non-empty regular ARM64 artifact.");
        }

        try
        {
            using FileStream file = File.OpenRead(path);
            using TarReader reader = new(file);
            if (reader.GetNextEntry() is null)
            {
                throw new DeploymentException(
                    "Docker produced an empty ARM64 image archive.");
            }
        }
        catch (InvalidDataException exception)
        {
            throw new DeploymentException(
                $"Docker produced an invalid or incomplete ARM64 image archive: {exception.Message}");
        }
    }
}
