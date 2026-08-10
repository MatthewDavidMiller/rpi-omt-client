//! What the operator's own workstation must provide before a deployment.
//!
//! The deployer builds the appliance image on the machine it runs on, which
//! needs local tooling the operator may not have. That gap is sharpest on
//! Windows, where a stock install has neither a POSIX shell nor a container
//! engine, and where the old code assumed `make` and reported nothing but
//! "program not found" when it was absent.
//!
//! Every rule here is a pure function over probed values, so the Windows
//! answers can be tested on a Linux workstation -- the only machine this
//! project's gates run on. The probes themselves are the thin part.

use crate::{ProcessResult, run_process};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// The same pinned probe image `scripts/check-arm64-emulation.sh` uses. Two
/// copies of the digest would be one too many chances to drift, but the script
/// cannot be called on a host that has no shell yet, which is exactly the host
/// this module exists for.
const EMULATION_PROBE_IMAGE: &str = "docker.io/library/debian:bookworm-slim@sha256:4724b8cc51e33e398f0e2e15e18d5ec2851ff0c2280647e1310bc1642182655d";

/// Whether this binary is running on Windows.
///
/// A runtime constant rather than `#[cfg]` around each rule: the rules below
/// then compile on every host and can be tested with either answer.
pub const ON_WINDOWS: bool = cfg!(windows);

/// The winget package that supplies a missing Windows prerequisite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Package {
    /// The exact winget package identifier.
    pub id: &'static str,
    /// The name an operator recognizes it by.
    pub name: &'static str,
}

/// Git for Windows: the POSIX shell and coreutils the image build runs in.
pub const GIT_FOR_WINDOWS: Package = Package {
    id: "Git.Git",
    name: "Git for Windows",
};
/// Docker Desktop: the container engine and its bundled ARM64 emulation.
pub const DOCKER_DESKTOP: Package = Package {
    id: "Docker.DockerDesktop",
    name: "Docker Desktop",
};
/// Python, used only to read the canonical version out of `pyproject.toml`.
pub const PYTHON: Package = Package {
    id: "Python.Python.3.13",
    name: "Python 3.13",
};

/// One thing the workstation must provide, as probed.
#[derive(Clone, Debug)]
pub struct Prerequisite {
    /// What it is called in the Setup view.
    pub name: &'static str,
    /// Why a deployment needs it.
    pub purpose: &'static str,
    /// Whether a local image build fails without it.
    pub required: bool,
    /// Whether the probe found it.
    pub satisfied: bool,
    /// What the probe found, or why it did not.
    pub detail: String,
    /// What the operator should do when it is missing.
    pub remedy: String,
    /// The winget package that supplies it, where one does.
    pub package: Option<Package>,
}

impl Prerequisite {
    /// Whether this row is a missing prerequisite a deployment needs.
    pub fn blocking(&self) -> bool {
        self.required && !self.satisfied
    }
}

/// How the image build is invoked on this workstation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl BuildPlan {
    /// The command as it would be typed, for the activity log.
    pub fn summary(&self) -> String {
        let mut summary = self.program.display().to_string();
        for argument in &self.args {
            summary.push(' ');
            summary.push_str(argument);
        }
        summary
    }
}

/// The file-name suffixes an executable may carry on this platform.
///
/// Windows resolves a bare `docker` through `PATHEXT`, so a search that only
/// tried the literal name would miss every tool on the machine. The empty
/// suffix is always first: a name that already carries its extension, and
/// every POSIX host, resolve without one.
fn executable_suffixes(windows: bool, pathext: Option<&str>) -> Vec<String> {
    let mut suffixes = vec![String::new()];
    if !windows {
        return suffixes;
    }
    let configured = pathext.unwrap_or_default();
    for entry in configured.split(';').map(str::trim).filter(|entry| {
        entry.starts_with('.') && entry.len() > 1 && !entry.contains(['/', '\\', ' '])
    }) {
        let entry = entry.to_owned();
        if !suffixes
            .iter()
            .any(|known| known.eq_ignore_ascii_case(&entry))
        {
            suffixes.push(entry);
        }
    }
    if suffixes.len() == 1 {
        for fallback in [".COM", ".EXE", ".BAT", ".CMD"] {
            suffixes.push(fallback.to_owned());
        }
    }
    suffixes
}

/// Whether `path` is a file this process could execute.
fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The first executable named `program` in `directories`.
fn find_in(directories: &[PathBuf], program: &str, suffixes: &[String]) -> Option<PathBuf> {
    directories.iter().find_map(|directory| {
        if directory.as_os_str().is_empty() {
            return None;
        }
        suffixes.iter().find_map(|suffix| {
            let candidate = directory.join(format!("{program}{suffix}"));
            is_executable_file(&candidate).then_some(candidate)
        })
    })
}

fn search_path() -> Vec<PathBuf> {
    env::var_os("PATH")
        .map(|value| env::split_paths(&value).collect())
        .unwrap_or_default()
}

/// Locate `program` on `PATH`.
///
/// A name that already contains a separator is taken as a path and only
/// checked for being executable, which is what lets `CONTAINER_ENGINE`-style
/// overrides name an interpreter outside `PATH`.
pub fn find_executable(program: &str) -> Option<PathBuf> {
    let suffixes = executable_suffixes(ON_WINDOWS, env::var("PATHEXT").ok().as_deref());
    let literal = Path::new(program);
    if literal.components().count() > 1 {
        return is_executable_file(literal).then(|| literal.to_path_buf());
    }
    find_in(&search_path(), program, &suffixes)
}

/// Whether a `bash.exe` is Windows' WSL launcher rather than a real shell.
///
/// `C:\Windows\System32\bash.exe` starts a Linux distribution whose file
/// system is not the one the deployer just handed it a path in, so a build
/// launched through it fails on a directory it cannot enter. Git for Windows
/// is the shell this project's scripts are written for, so a `System32` hit is
/// skipped rather than preferred by being earlier on `PATH`.
fn is_wsl_launcher(path: &Path) -> bool {
    let lowered = path
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace('\\', "/");
    lowered.contains("/windows/system32/") || lowered.contains("/windows/sysnative/")
}

/// Where Git for Windows and MSYS2 put their shell, in preference order.
///
/// Probed by path rather than through `PATH` because a fresh Git for Windows
/// install does not put `bash` on the `PATH` of a process that was already
/// running -- including the deployer that just installed it.
fn windows_bash_candidates(variable: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut push = |root: PathBuf| {
        candidates.push(root.join("bin").join("bash.exe"));
        candidates.push(root.join("usr").join("bin").join("bash.exe"));
    };
    for program_files in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Some(value) = variable(program_files) {
            push(PathBuf::from(value).join("Git"));
        }
    }
    if let Some(value) = variable("LOCALAPPDATA") {
        push(PathBuf::from(value).join("Programs").join("Git"));
    }
    // `SystemDrive` is `C:` with no separator, and a bare `C:` joined with a
    // relative path names the drive's *current* directory rather than its root.
    let system_drive = variable("SystemDrive").map_or_else(
        || PathBuf::from("C:\\"),
        |value| {
            let mut root = value;
            root.push("\\");
            PathBuf::from(root)
        },
    );
    push(system_drive.join("msys64"));
    push(system_drive.join("Git"));
    candidates
}

/// Locate a POSIX shell the appliance build can run in.
pub fn find_bash() -> Option<PathBuf> {
    if ON_WINDOWS {
        let candidates =
            windows_bash_candidates(&|name| env::var_os(name).filter(|v| !v.is_empty()));
        if let Some(found) = candidates.iter().find(|path| is_executable_file(path)) {
            return Some(found.clone());
        }
    }
    find_executable("bash").filter(|path| !ON_WINDOWS || !is_wsl_launcher(path))
}

/// The container engine this workstation would build with, as a path and kind.
///
/// Docker first, then Podman: the same order `scripts/docker-test-env.sh`
/// resolves in, so the deployer and the shell gates cannot pick differently.
pub fn find_container_engine() -> Option<(PathBuf, &'static str)> {
    find_executable("docker")
        .map(|path| (path, "docker"))
        .or_else(|| find_executable("podman").map(|path| (path, "podman")))
}

/// The image build command for a workstation with `make` and `bash` as probed.
///
/// Windows goes through `bash` even when GNU Make is installed. The Makefile
/// recipe is a call to `scripts/build-arm64.sh`, so make without a POSIX shell
/// hands that script to `cmd.exe` and fails; make with one adds nothing the
/// shell does not already do. That makes Git for Windows the only shell
/// prerequisite on Windows, and GNU Make none at all.
fn plan_from(
    make: Option<PathBuf>,
    bash: Option<PathBuf>,
    tarball_name: &str,
    windows: bool,
) -> Result<BuildPlan, String> {
    let through_bash = |bash: PathBuf| BuildPlan {
        program: bash,
        args: vec!["scripts/build-arm64.sh".to_owned()],
        env: vec![("ARM64_TARBALL".to_owned(), tarball_name.to_owned())],
    };
    if windows {
        return bash.map(through_bash).ok_or_else(|| {
            format!(
                "no POSIX shell was found, so the ARM64 appliance image cannot be built on this \
                 Windows machine. Install {} from the Setup view, or clear \"Build the appliance \
                 image\" on the Deploy view and deploy a {tarball_name} built elsewhere.",
                GIT_FOR_WINDOWS.name
            )
        });
    }
    if let Some(make) = make {
        return Ok(BuildPlan {
            program: make,
            args: vec![
                "build-arm64".to_owned(),
                format!("ARM64_TARBALL={tarball_name}"),
            ],
            env: Vec::new(),
        });
    }
    bash.map(through_bash).ok_or_else(|| {
        "neither GNU Make nor bash is on PATH, so the ARM64 appliance image cannot be built. \
         Install them with `make install`, or clear \"Build the appliance image\" and deploy an \
         archive built elsewhere."
            .to_owned()
    })
}

/// How this workstation would build the appliance image.
pub fn image_build_plan(tarball_name: &str) -> io::Result<BuildPlan> {
    plan_from(
        find_executable("make"),
        find_bash(),
        tarball_name,
        ON_WINDOWS,
    )
    .map_err(|message| io::Error::new(io::ErrorKind::NotFound, message))
}

fn present(
    name: &'static str,
    purpose: &'static str,
    required: bool,
    detail: String,
) -> Prerequisite {
    Prerequisite {
        name,
        purpose,
        required,
        satisfied: true,
        detail,
        remedy: String::new(),
        package: None,
    }
}

fn missing(
    name: &'static str,
    purpose: &'static str,
    required: bool,
    detail: &str,
    remedy: String,
    package: Option<Package>,
) -> Prerequisite {
    Prerequisite {
        name,
        purpose,
        required,
        satisfied: false,
        detail: detail.to_owned(),
        remedy,
        package: package.filter(|_| ON_WINDOWS),
    }
}

/// The remedy for a tool that has no winget package on this platform.
fn manual_remedy(windows: &str, unix: &str) -> String {
    if ON_WINDOWS { windows } else { unix }.to_owned()
}

fn shell_row() -> Prerequisite {
    match find_bash() {
        Some(path) => present(
            "POSIX shell",
            "runs the pinned ARM64 image build",
            ON_WINDOWS,
            path.display().to_string(),
        ),
        None => missing(
            "POSIX shell",
            "runs the pinned ARM64 image build",
            ON_WINDOWS,
            "no bash was found",
            manual_remedy(
                "Install Git for Windows. Its bash and coreutils are what the image build runs in.",
                "Install bash through this system's package manager.",
            ),
            Some(GIT_FOR_WINDOWS),
        ),
    }
}

fn engine_row(cancellation: &Arc<AtomicBool>) -> Prerequisite {
    let Some((path, kind)) = find_container_engine() else {
        return missing(
            "Container engine",
            "builds and exports the ARM64 appliance image",
            true,
            "neither docker nor podman was found",
            manual_remedy(
                "Install Docker Desktop and start it with the Linux engine selected.",
                "Install Docker or Podman through this system's package manager.",
            ),
            Some(DOCKER_DESKTOP),
        );
    };
    // Installed is not the same as running: Docker Desktop is the common case
    // of a `docker` on PATH whose engine is stopped, and a build started
    // against it fails several minutes later rather than now.
    match run_process(
        &path,
        &["info".to_owned()],
        &engine_probe_directory(),
        &[],
        Arc::clone(cancellation),
    ) {
        Ok(result) if result.exit_code == 0 => present(
            "Container engine",
            "builds and exports the ARM64 appliance image",
            true,
            format!("{kind} at {} is running", path.display()),
        ),
        Ok(_) | Err(_) => missing(
            "Container engine",
            "builds and exports the ARM64 appliance image",
            true,
            &format!(
                "{kind} at {} is installed but not responding",
                path.display()
            ),
            manual_remedy(
                "Start Docker Desktop and wait for it to report that the engine is running.",
                "Start the container engine's service, or add this account to its group.",
            ),
            None,
        ),
    }
}

/// Where the engine probe runs. Its own directory is irrelevant to `info`, but
/// a process cannot be spawned into one that no longer exists.
fn engine_probe_directory() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Python is reported as found wherever it resolves, including through a
/// Windows App Execution Alias. A stub alias that only opens the Microsoft
/// Store cannot be told apart from a working Store install by anything this
/// can read, and guessing wrong would advertise an install the machine already
/// has. Nothing breaks either way: `scripts/detect-version.sh` discards a
/// failed interpreter and falls back to the Git tag, which is what this row
/// already says happens without Python at all.
fn python_row() -> Prerequisite {
    match ["python3", "python"].into_iter().find_map(find_executable) {
        Some(path) => present(
            "Python 3",
            "stamps the exact release version into the image",
            false,
            path.display().to_string(),
        ),
        None => missing(
            "Python 3",
            "stamps the exact release version into the image",
            false,
            "no python3 was found",
            manual_remedy(
                "Optional. Without it the build falls back to the Git tag for its version.",
                "Optional. Without it the build falls back to the Git tag for its version.",
            ),
            Some(PYTHON),
        ),
    }
}

fn project_rows(project_root: &Path, tarball_name: &str) -> Vec<Prerequisite> {
    let manifest = project_root.join("deploy/manifest-v3.txt");
    let source = if manifest.is_file() {
        present(
            "Project source tree",
            "supplies the manifest-v3 capsule that is uploaded",
            true,
            project_root.display().to_string(),
        )
    } else {
        missing(
            "Project source tree",
            "supplies the manifest-v3 capsule that is uploaded",
            true,
            &format!("{} is not there", manifest.display()),
            "Point Project root at the checkout of this repository.".to_owned(),
            None,
        )
    };

    let archive = project_root.join(tarball_name);
    let bytes = fs::metadata(&archive)
        .ok()
        .filter(fs::Metadata::is_file)
        .map(|metadata| metadata.len());
    let image = match bytes {
        Some(bytes) => present(
            "Appliance image archive",
            "the ARM64 image the Pi loads",
            false,
            format!("{tarball_name}, {} MiB", bytes / (1024 * 1024)),
        ),
        None => missing(
            "Appliance image archive",
            "the ARM64 image the Pi loads",
            false,
            &format!("{tarball_name} has not been built yet"),
            "Leave \"Build the appliance image\" set on the Deploy view, or copy an archive \
             built elsewhere into the project root."
                .to_owned(),
            None,
        ),
    };
    vec![source, image]
}

/// Probe everything a deployment needs from this workstation.
///
/// Tools only: nothing here reaches the network or pulls an image, so the
/// Setup view answers immediately. ARM64 emulation is the one prerequisite
/// that cannot be established without running a container, and it has its own
/// entry point for that reason.
pub fn prerequisites(
    project_root: &Path,
    tarball_name: &str,
    cancellation: &Arc<AtomicBool>,
) -> Vec<Prerequisite> {
    let mut rows = vec![engine_row(cancellation), shell_row()];
    if !ON_WINDOWS {
        rows.push(match find_executable("make") {
            Some(path) => present(
                "GNU Make",
                "the documented entry point for the image build",
                false,
                path.display().to_string(),
            ),
            None => missing(
                "GNU Make",
                "the documented entry point for the image build",
                false,
                "make is not on PATH",
                "Optional. Without it the build calls scripts/build-arm64.sh directly.".to_owned(),
                None,
            ),
        });
    }
    rows.push(python_row());
    rows.extend(project_rows(project_root, tarball_name));
    rows
}

/// The winget packages that would satisfy the missing rows in `rows`.
pub fn missing_packages(rows: &[Prerequisite]) -> Vec<Package> {
    let mut packages: Vec<Package> = Vec::new();
    for package in rows
        .iter()
        .filter(|row| !row.satisfied)
        .filter_map(|row| row.package)
    {
        if !packages.contains(&package) {
            packages.push(package);
        }
    }
    packages
}

/// winget's "this package is already installed" result.
///
/// `APPINSTALLER_CLI_ERROR_UPDATE_NOT_APPLICABLE` (0x8A15002B) is what an
/// install of an up-to-date package reports, and treating it as a failure
/// would turn a satisfied prerequisite into an error the operator has to
/// interpret.
const WINGET_ALREADY_INSTALLED: i32 = 0x8A15_002B_u32.cast_signed();

/// Install `packages` with winget, reporting each step through `progress`.
///
/// Windows only, and deliberately non-silent: Docker Desktop installs
/// machine-wide, so winget raises a UAC prompt the operator has to approve.
pub fn install_packages(
    packages: &[Package],
    cancellation: &Arc<AtomicBool>,
    progress: &mut dyn FnMut(&str),
) -> io::Result<()> {
    if !ON_WINDOWS {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "automatic prerequisite installation is Windows-only; on this system run \
             `make install`",
        ));
    }
    if packages.is_empty() {
        progress("Every prerequisite with a winget package is already installed.");
        return Ok(());
    }
    let winget = find_executable("winget").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "winget was not found. Install \"App Installer\" from the Microsoft Store, then \
             re-check prerequisites.",
        )
    })?;
    let directory = engine_probe_directory();
    progress("Windows will ask to approve each installer. Approve the UAC prompt to continue.");
    for package in packages {
        progress(&format!("Installing {} ({})...", package.name, package.id));
        let arguments = [
            "install",
            "--exact",
            "--id",
            package.id,
            "--source",
            "winget",
            "--accept-package-agreements",
            "--accept-source-agreements",
        ]
        .map(str::to_owned);
        let result = run_process(
            &winget,
            &arguments,
            &directory,
            &[],
            Arc::clone(cancellation),
        )?;
        report_install(package, &result, progress)?;
    }
    progress(
        "Prerequisites installed. Docker Desktop must be started once, and a sign-out or \
         restart may be needed before new tools appear on PATH.",
    );
    Ok(())
}

fn report_install(
    package: &Package,
    result: &ProcessResult,
    progress: &mut dyn FnMut(&str),
) -> io::Result<()> {
    match result.exit_code {
        0 => {
            progress(&format!("Installed {}.", package.name));
            Ok(())
        }
        WINGET_ALREADY_INSTALLED => {
            progress(&format!("{} is already installed.", package.name));
            Ok(())
        }
        code => Err(io::Error::other(format!(
            "installing {} failed (winget exit {code}):\n{}",
            package.name,
            result.output.trim()
        ))),
    }
}

/// Confirm the container engine can execute ARM64 containers here.
///
/// The appliance image installs packages during its own build, so this is a
/// hard requirement rather than a nicety. It is not part of `prerequisites`
/// because answering it means pulling and running a small pinned image.
pub fn check_arm64_emulation(
    cancellation: &Arc<AtomicBool>,
    progress: &mut dyn FnMut(&str),
) -> io::Result<()> {
    let (engine, kind) = find_container_engine().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no container engine is installed, so ARM64 emulation cannot be checked",
        )
    })?;
    let directory = engine_probe_directory();
    if kind == "docker" {
        let arguments = ["buildx", "version"].map(str::to_owned);
        let buildx = run_process(
            &engine,
            &arguments,
            &directory,
            &[],
            Arc::clone(cancellation),
        )?;
        if buildx.exit_code != 0 {
            return Err(io::Error::other(
                "Docker buildx is not available, so the ARM64 image cannot be exported",
            ));
        }
    }
    progress("Running a pinned ARM64 container to confirm emulation is registered...");
    let arguments = [
        "run",
        "--rm",
        "--platform",
        "linux/arm64",
        "--entrypoint",
        "/bin/sh",
        EMULATION_PROBE_IMAGE,
        "-c",
        "test \"$(uname -m)\" = aarch64",
    ]
    .map(str::to_owned);
    let result = run_process(
        &engine,
        &arguments,
        &directory,
        &[],
        Arc::clone(cancellation),
    )?;
    if result.exit_code == 0 {
        progress("ARM64 emulation is registered on this workstation.");
        return Ok(());
    }
    Err(io::Error::other(format!(
        "{kind} cannot execute linux/arm64 containers here.\n{}\n{}",
        if ON_WINDOWS {
            "Docker Desktop provides this once the Linux engine is running; make sure it is \
             started and not in Windows-container mode."
        } else {
            "Register it once with `make setup-arm64-emulation`, then re-check."
        },
        result.output.trim()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(make: Option<&str>, bash: Option<&str>, windows: bool) -> Result<BuildPlan, String> {
        plan_from(
            make.map(PathBuf::from),
            bash.map(PathBuf::from),
            "omt-client-arm64.tar.gz",
            windows,
        )
    }

    /// The reported failure: a Windows workstation with no `make`, where the
    /// only message was "program not found". A shell is enough to build, and
    /// when there is not one the error has to name the package that supplies
    /// it and the switch that skips the build entirely.
    #[test]
    fn windows_builds_through_bash_and_never_needs_make() {
        let built = plan(None, Some("C:\\Program Files\\Git\\bin\\bash.exe"), true)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            built.program,
            PathBuf::from("C:\\Program Files\\Git\\bin\\bash.exe")
        );
        assert_eq!(built.args, ["scripts/build-arm64.sh"]);
        assert_eq!(
            built.env,
            [(
                "ARM64_TARBALL".to_owned(),
                "omt-client-arm64.tar.gz".to_owned()
            )]
        );

        // GNU Make on Windows hands scripts/build-arm64.sh to cmd.exe, so it
        // is not a substitute for the shell and must not be picked.
        let make_only = plan(Some("C:\\make.exe"), None, true);
        let message = make_only.err().unwrap_or_default();
        assert!(message.contains("Git for Windows"), "{message}");
        assert!(message.contains("Build the appliance image"), "{message}");
        assert_eq!(
            plan(Some("C:\\make.exe"), Some("C:\\Git\\bin\\bash.exe"), true)
                .map(|value| value.program),
            Ok(PathBuf::from("C:\\Git\\bin\\bash.exe"))
        );
    }

    #[test]
    fn unix_prefers_make_and_falls_back_to_the_script() {
        let with_make = plan(Some("/usr/bin/make"), Some("/bin/bash"), false)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(with_make.program, PathBuf::from("/usr/bin/make"));
        assert_eq!(
            with_make.args,
            ["build-arm64", "ARM64_TARBALL=omt-client-arm64.tar.gz"]
        );
        assert!(with_make.env.is_empty());

        let without_make =
            plan(None, Some("/bin/bash"), false).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(without_make.args, ["scripts/build-arm64.sh"]);
        assert!(plan(None, None, false).is_err());
    }

    /// The archive name is an option, so a plan that ignored it would build
    /// one file and upload another.
    #[test]
    fn the_plan_carries_the_requested_archive_name() {
        let named = plan_from(Some("/usr/bin/make".into()), None, "custom.tar.gz", false)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            named
                .args
                .contains(&"ARM64_TARBALL=custom.tar.gz".to_owned())
        );
        let scripted = plan_from(None, Some("/bin/bash".into()), "custom.tar.gz", false)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            scripted.env,
            [("ARM64_TARBALL".to_owned(), "custom.tar.gz".to_owned())]
        );
    }

    #[test]
    fn a_plan_summarizes_the_command_it_would_run() {
        let summary = plan(Some("/usr/bin/make"), None, false)
            .map(|value| value.summary())
            .unwrap_or_default();
        assert_eq!(
            summary,
            "/usr/bin/make build-arm64 ARM64_TARBALL=omt-client-arm64.tar.gz"
        );
    }

    /// `PATHEXT` is how Windows resolves a bare `docker`. The empty suffix
    /// stays first so a name that already carries its extension, and every
    /// POSIX host, resolve unchanged.
    #[test]
    fn windows_executable_names_follow_pathext() {
        let suffixes = executable_suffixes(true, Some(".COM;.EXE;.BAT;.CMD;.VBS"));
        assert_eq!(suffixes.first().map(String::as_str), Some(""));
        assert!(suffixes.iter().any(|value| value == ".EXE"));
        assert!(suffixes.iter().any(|value| value == ".CMD"));
        assert_eq!(executable_suffixes(false, Some(".EXE")), [String::new()]);

        // An unset or unusable PATHEXT still has to find docker.exe.
        for broken in [None, Some(""), Some(";;"), Some("exe"), Some(". exe")] {
            let recovered = executable_suffixes(true, broken);
            assert!(
                recovered.iter().any(|value| value == ".EXE"),
                "PATHEXT {broken:?} lost the default suffixes"
            );
        }
        // Duplicates in PATHEXT must not multiply the file-system probes.
        let deduplicated = executable_suffixes(true, Some(".EXE;.exe;.EXE"));
        assert_eq!(deduplicated.len(), 2);
    }

    /// `C:\Windows\System32\bash.exe` is the WSL launcher. It runs a shell in
    /// another file system, where the Windows project path the build is handed
    /// does not exist.
    #[test]
    fn the_wsl_launcher_is_not_a_build_shell() {
        assert!(is_wsl_launcher(Path::new(
            "C:\\Windows\\System32\\bash.exe"
        )));
        assert!(is_wsl_launcher(Path::new(
            "c:\\windows\\sysnative\\bash.exe"
        )));
        assert!(!is_wsl_launcher(Path::new(
            "C:\\Program Files\\Git\\bin\\bash.exe"
        )));
        assert!(!is_wsl_launcher(Path::new("/bin/bash")));
    }

    /// A fresh Git for Windows install is not on the `PATH` of the process
    /// that installed it, so the shell has to be findable by location.
    #[test]
    fn git_for_windows_is_found_where_it_installs() {
        let variable = |name: &str| match name {
            "ProgramFiles" => Some(OsString::from("C:\\Program Files")),
            "LOCALAPPDATA" => Some(OsString::from("C:\\Users\\op\\AppData\\Local")),
            "SystemDrive" => Some(OsString::from("C:")),
            _ => None,
        };
        // Built by joining rather than written out: this gate runs on a Linux
        // host, whose path separator is not the one a literal would carry.
        let candidates = windows_bash_candidates(&variable);
        let expected = [
            PathBuf::from("C:\\Program Files").join("Git").join("bin"),
            PathBuf::from("C:\\Users\\op\\AppData\\Local")
                .join("Programs")
                .join("Git")
                .join("bin"),
            PathBuf::from("C:\\").join("msys64").join("usr").join("bin"),
        ];
        for directory in expected {
            let shell = directory.join("bash.exe");
            assert!(
                candidates.contains(&shell),
                "{} is not probed",
                shell.display()
            );
        }
        // No environment at all still probes the default system drive.
        assert!(candidates.len() > 4);
        assert_eq!(
            windows_bash_candidates(&|_| None),
            windows_bash_candidates(&|name| (name == "SystemDrive").then(|| OsString::from("C:")))
        );
    }

    /// Only the rows an installer can actually fix are offered, once each.
    #[test]
    fn winget_packages_come_from_unsatisfied_rows_only() {
        let row = |satisfied, package| Prerequisite {
            name: "row",
            purpose: "purpose",
            required: true,
            satisfied,
            detail: String::new(),
            remedy: String::new(),
            package,
        };
        let rows = [
            row(true, Some(DOCKER_DESKTOP)),
            row(false, Some(GIT_FOR_WINDOWS)),
            row(false, Some(GIT_FOR_WINDOWS)),
            row(false, None),
        ];
        assert_eq!(missing_packages(&rows), [GIT_FOR_WINDOWS]);
        assert!(missing_packages(&[]).is_empty());
    }

    #[test]
    fn a_missing_required_row_blocks_and_an_optional_one_does_not() {
        let row = |required, satisfied| Prerequisite {
            name: "row",
            purpose: "purpose",
            required,
            satisfied,
            detail: String::new(),
            remedy: String::new(),
            package: None,
        };
        assert!(row(true, false).blocking());
        assert!(!row(true, true).blocking());
        assert!(!row(false, false).blocking());
    }

    /// winget reports an up-to-date package as a failure code. Treating it as
    /// one turns a satisfied prerequisite into an error to interpret.
    #[test]
    fn an_already_installed_package_is_not_an_installation_failure() {
        let mut lines = Vec::new();
        let mut progress = |line: &str| lines.push(line.to_owned());
        let result = |exit_code| ProcessResult {
            exit_code,
            output: String::new(),
        };
        assert!(report_install(&GIT_FOR_WINDOWS, &result(0), &mut progress).is_ok());
        assert!(
            report_install(
                &GIT_FOR_WINDOWS,
                &result(WINGET_ALREADY_INSTALLED),
                &mut progress
            )
            .is_ok()
        );
        assert!(report_install(&GIT_FOR_WINDOWS, &result(1), &mut progress).is_err());
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().any(|line| line.contains("already installed")));
    }

    /// The Setup view reads these rows on a workstation, so the probe has to
    /// terminate and describe this one rather than panicking on it.
    #[test]
    fn probing_this_workstation_reports_the_project_it_was_given() {
        let cancellation = Arc::new(AtomicBool::new(false));
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let rows = prerequisites(&root, "omt-client-arm64.tar.gz", &cancellation);
        let source = rows
            .iter()
            .find(|row| row.name == "Project source tree")
            .map(|row| row.satisfied);
        assert_eq!(source, Some(true), "this repository is not a project root");
        assert!(rows.iter().any(|row| row.name == "Container engine"));
        assert!(rows.iter().any(|row| row.name == "POSIX shell"));

        let elsewhere = prerequisites(
            Path::new("/nonexistent-omt-project"),
            "omt-client-arm64.tar.gz",
            &cancellation,
        );
        assert!(
            elsewhere
                .iter()
                .any(|row| row.name == "Project source tree" && row.blocking())
        );
    }

    /// Automatic installation is a Windows affordance. On this project's own
    /// gate hosts it has to refuse rather than shell out to something else.
    #[test]
    fn installation_is_refused_where_there_is_no_winget() {
        if ON_WINDOWS {
            return;
        }
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut progress = |_: &str| {};
        let error = install_packages(&[GIT_FOR_WINDOWS], &cancellation, &mut progress)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(error.contains("make install"), "{error}");
    }
}
