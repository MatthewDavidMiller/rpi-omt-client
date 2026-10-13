//! What a workstation must provide before it can rebuild the appliance.
//!
//! An operator's deployment needs none of this: the capsule is compiled into
//! the binary, so nothing here is probed unless a project root is named.
//! What remains is the developer path, where the image is built on the
//! machine the deployer runs on -- sharpest on Windows, where a stock
//! install has neither a POSIX shell nor a container engine.
//!
//! Every rule is a function over probed values with the platform passed in,
//! so the Windows answers are tested on a Linux workstation, the only
//! machine the gates run on. The probes themselves are the thin part.

use omt_common::{Error, Result};

use crate::capsule::Capsule;
use crate::core::{IMAGE_MEMBER, Progress, report, sha256_hex};
use crate::sys::{self, Cancel, ON_WINDOWS, PATH_LIST_SEP, ProcessResult};

/// The same pinned probe image scripts/check-arm64-emulation.sh uses.
const EMULATION_PROBE_IMAGE: &str = "docker.io/library/debian:bookworm-slim@sha256:\
     4724b8cc51e33e398f0e2e15e18d5ec2851ff0c2280647e1310bc1642182655d";
/// The same pinned binfmt installer scripts/install-arm64-emulation.sh takes
/// its emulator from. Run with --install, it registers the handler in the
/// kernel it runs against.
const BINFMT_INSTALLER_IMAGE: &str = "docker.io/tonistiigi/binfmt@sha256:\
     400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0";
const BINFMT_ARCHITECTURES: &str = "arm64,arm";

#[derive(Debug, PartialEq, Eq)]
pub struct Package {
    pub id: &'static str,
    pub name: &'static str,
}

pub static GIT_FOR_WINDOWS: Package = Package { id: "Git.Git", name: "Git for Windows" };
pub static DOCKER_DESKTOP: Package = Package { id: "Docker.DockerDesktop", name: "Docker Desktop" };
pub static PYTHON: Package = Package { id: "Python.Python.3.13", name: "Python 3.13" };

/// winget's APPINSTALLER_CLI_ERROR_UPDATE_NOT_APPLICABLE: the package is
/// already installed and current.
pub const WINGET_ALREADY_INSTALLED: i32 = 0x8A15_002B_u32.cast_signed();

const MAX_SUFFIXES: usize = 32;
const MAX_CANDIDATES: usize = 16;

// ------------------------------------------------------------------ lookup

/// The file-name suffixes an executable may carry: the empty suffix first,
/// then PATHEXT's entries on Windows, or .COM .EXE .BAT .CMD when PATHEXT is
/// unset or unusable.
pub fn executable_suffixes(windows: bool, pathext: Option<&str>) -> Vec<String> {
    let mut out = vec![String::new()];
    if !windows {
        return out;
    }
    for entry in pathext.unwrap_or("").split(';') {
        if out.len() >= MAX_SUFFIXES {
            break;
        }
        let t = entry.trim();
        let usable = t.len() > 1 && t.starts_with('.') && !t.contains(['/', '\\', ' ']);
        if usable && !out.iter().any(|s| s.eq_ignore_ascii_case(t)) {
            out.push(t.to_owned());
        }
    }
    if out.len() == 1 {
        out.extend([".COM", ".EXE", ".BAT", ".CMD"].map(str::to_owned));
    }
    out
}

fn has_separator(program: &str) -> bool {
    program.contains('/') || (ON_WINDOWS && program.contains('\\'))
}

/// Locates an executable on PATH, or checks a path that names one.
pub fn find_executable(program: &str) -> Option<String> {
    if has_separator(program) {
        return sys::is_executable_file(program).then(|| program.to_owned());
    }
    let suffixes = executable_suffixes(ON_WINDOWS, sys::getenv("PATHEXT").as_deref());
    let path = sys::getenv("PATH")?;
    for dir in path.split(PATH_LIST_SEP).filter(|d| !d.is_empty()) {
        for suffix in &suffixes {
            let candidate = sys::path_join(dir, &format!("{program}{suffix}"));
            if sys::is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether a bash.exe is Windows' WSL launcher rather than a real shell.
pub fn is_wsl_launcher(path: &str) -> bool {
    let lowered = path.replace('\\', "/").to_ascii_lowercase();
    lowered.contains("/windows/system32/") || lowered.contains("/windows/sysnative/")
}

fn push_bash_root(out: &mut Vec<String>, root: &str) {
    if out.len() < MAX_CANDIDATES {
        out.push(sys::path_join(&sys::path_join(root, "bin"), "bash.exe"));
    }
    if out.len() < MAX_CANDIDATES {
        out.push(sys::path_join(&sys::path_join(&sys::path_join(root, "usr"), "bin"), "bash.exe"));
    }
}

/// Where Git for Windows and MSYS2 put bash, in preference order.
pub fn windows_bash_candidates(lookup: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let mut out = Vec::new();
    for name in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Some(v) = lookup(name) {
            push_bash_root(&mut out, &sys::path_join(&v, "Git"));
        }
    }
    if let Some(v) = lookup("LOCALAPPDATA") {
        push_bash_root(&mut out, &sys::path_join(&sys::path_join(&v, "Programs"), "Git"));
    }
    // SystemDrive is `C:` with no separator, and a bare `C:` joined with a
    // relative path names the drive's current directory rather than its
    // root.
    let drive = lookup("SystemDrive").map_or_else(|| "C:\\".to_owned(), |d| format!("{d}\\"));
    for root in ["msys64", "Git"] {
        push_bash_root(&mut out, &sys::path_join(&drive, root));
    }
    out
}

fn env_lookup(name: &str) -> Option<String> {
    sys::getenv(name).filter(|v| !v.is_empty())
}

pub fn find_bash() -> Option<String> {
    if ON_WINDOWS
        && let Some(found) =
            windows_bash_candidates(&env_lookup).into_iter().find(|c| sys::is_executable_file(c))
    {
        return Some(found);
    }
    find_executable("bash").filter(|b| !ON_WINDOWS || !is_wsl_launcher(b))
}

/// Docker first, then Podman: the order scripts/docker-test-env.sh uses.
pub fn find_container_engine() -> Option<(String, &'static str)> {
    find_executable("docker")
        .map(|e| (e, "docker"))
        .or_else(|| find_executable("podman").map(|e| (e, "podman")))
}

// -------------------------------------------------------------------- plan

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildPlan {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl BuildPlan {
    pub fn summary(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The image build command for make and bash as probed. Windows goes
/// through bash even when GNU Make is installed: the Makefile recipe is a
/// call to scripts/build-arm64.sh, so make without a POSIX shell hands that
/// script to cmd.exe and fails.
pub fn plan_from(make: Option<&str>, bash: Option<&str>, windows: bool) -> Result<BuildPlan> {
    if let Some(bash) = bash
        && (windows || make.is_none())
    {
        return Ok(BuildPlan {
            program: bash.to_owned(),
            args: vec!["scripts/build-arm64.sh".into()],
            env: vec![("ARM64_TARBALL".into(), IMAGE_MEMBER.into())],
        });
    }
    if windows {
        return Err(Error::new(format!(
            "no POSIX shell was found, so the ARM64 appliance image cannot be rebuilt on this \
             Windows machine. Install {}, or deploy without a project root: the archive embedded \
             in this deployer needs no build tooling.",
            GIT_FOR_WINDOWS.name
        )));
    }
    if let Some(make) = make {
        return Ok(BuildPlan {
            program: make.to_owned(),
            args: vec!["build-arm64".into(), format!("ARM64_TARBALL={IMAGE_MEMBER}")],
            env: Vec::new(),
        });
    }
    Err(Error::new(
        "neither GNU Make nor bash is on PATH, so the ARM64 appliance image cannot be rebuilt. \
         Install them with `make install`, or deploy without a project root: the archive \
         embedded in this deployer needs no build tooling.",
    ))
}

pub fn image_build_plan() -> Result<BuildPlan> {
    plan_from(find_executable("make").as_deref(), find_bash().as_deref(), ON_WINDOWS)
}

// ------------------------------------------------------------------ report

#[derive(Debug)]
pub struct Prerequisite {
    pub name: &'static str,
    pub purpose: &'static str,
    pub required: bool,
    pub satisfied: bool,
    pub detail: String,
    pub remedy: String,
    /// `None` when no winget package supplies it.
    pub package: Option<&'static Package>,
}

impl Prerequisite {
    pub fn blocking(&self) -> bool {
        self.required && !self.satisfied
    }
}

#[allow(clippy::fn_params_excessive_bools, clippy::too_many_arguments)]
fn row(
    rows: &mut Vec<Prerequisite>,
    name: &'static str,
    purpose: &'static str,
    required: bool,
    satisfied: bool,
    detail: String,
    remedy: &str,
    package: Option<&'static Package>,
) {
    rows.push(Prerequisite {
        name,
        purpose,
        required,
        satisfied,
        detail,
        remedy: remedy.to_owned(),
        // A winget package is only an answer on Windows.
        package: if satisfied || !ON_WINDOWS { None } else { package },
    });
}

fn manual_remedy(windows: &'static str, unix: &'static str) -> &'static str {
    if ON_WINDOWS { windows } else { unix }
}

fn shell_row(rows: &mut Vec<Prerequisite>) {
    const NAME: &str = "POSIX shell";
    const PURPOSE: &str = "runs the pinned ARM64 image build";
    match find_bash() {
        Some(bash) => row(rows, NAME, PURPOSE, ON_WINDOWS, true, bash, "", None),
        None => row(
            rows,
            NAME,
            PURPOSE,
            ON_WINDOWS,
            false,
            "no bash was found".into(),
            manual_remedy(
                "Install Git for Windows. Its bash and coreutils are what the image build runs in.",
                "Install bash through this system's package manager.",
            ),
            Some(&GIT_FOR_WINDOWS),
        ),
    }
}

fn engine_row(rows: &mut Vec<Prerequisite>, cancel: Option<&Cancel>) {
    const NAME: &str = "Container engine";
    const PURPOSE: &str = "builds and exports the ARM64 appliance image";
    let Some((engine, kind)) = find_container_engine() else {
        row(
            rows,
            NAME,
            PURPOSE,
            true,
            false,
            "neither docker nor podman was found".into(),
            manual_remedy(
                "Install Docker Desktop and start it with the Linux engine selected.",
                "Install Docker or Podman through this system's package manager.",
            ),
            Some(&DOCKER_DESKTOP),
        );
        return;
    };
    // Installed is not running: Docker Desktop is the common case of a
    // docker on PATH whose engine is stopped.
    let cwd = sys::current_dir();
    let ran = sys::run_process(&engine, &["info"], Some(&cwd), &[], cancel);
    if ran.is_ok_and(|r| r.exit_code == 0) {
        row(rows, NAME, PURPOSE, true, true, format!("{kind} at {engine} is running"), "", None);
    } else {
        row(
            rows,
            NAME,
            PURPOSE,
            true,
            false,
            format!("{kind} at {engine} is installed but not responding"),
            manual_remedy(
                "Start Docker Desktop and wait for it to report that the engine is running.",
                "Start the container engine's service, or add this account to its group.",
            ),
            None,
        );
    }
}

/// Python is reported as found wherever it resolves. Nothing breaks either
/// way: scripts/detect-version.sh falls back to the Git tag without it.
fn python_row(rows: &mut Vec<Prerequisite>) {
    const PURPOSE: &str = "stamps the exact release version into the image";
    match find_executable("python3").or_else(|| find_executable("python")) {
        Some(p) => row(rows, "Python 3", PURPOSE, false, true, p, "", None),
        None => row(
            rows,
            "Python 3",
            PURPOSE,
            false,
            false,
            "no python3 was found".into(),
            "Optional. Without it the build falls back to the Git tag for its version.",
            Some(&PYTHON),
        ),
    }
}

fn project_rows(rows: &mut Vec<Prerequisite>, root: &str) {
    const ARCHIVE: &str = "Appliance image archive";
    const ARCHIVE_PURPOSE: &str = "the ARM64 image the Pi loads";
    const SOURCE: &str = "Project source tree";
    const SOURCE_PURPOSE: &str = "supplies the manifest-v3 capsule that is uploaded";
    let manifest = sys::path_join(&sys::path_join(root, "deploy"), "manifest-v3.txt");
    if sys::is_file(&manifest) {
        row(rows, SOURCE, SOURCE_PURPOSE, true, true, root.to_owned(), "", None);
    } else {
        row(
            rows,
            SOURCE,
            SOURCE_PURPOSE,
            true,
            false,
            format!("{manifest} is not there"),
            "Point --project at the checkout of this repository, or drop it and deploy the \
             capsule embedded in this deployer.",
            None,
        );
    }
    match sys::file_size(sys::path_join(root, IMAGE_MEMBER)) {
        Some(bytes) => row(
            rows,
            ARCHIVE,
            ARCHIVE_PURPOSE,
            false,
            true,
            format!("{IMAGE_MEMBER}, {} MiB", bytes / (1024 * 1024)),
            "",
            None,
        ),
        None => row(
            rows,
            ARCHIVE,
            ARCHIVE_PURPOSE,
            false,
            false,
            format!("{IMAGE_MEMBER} has not been built in this tree yet"),
            "Run `make build-arm64`, or deploy without --project to upload the archive embedded \
             in this deployer.",
            None,
        ),
    }
}

/// The only row an operator's deployment has, satisfied by construction:
/// the build will not produce a deployer without the archive. It is reported
/// because its size and digest are the only description of the appliance a
/// single-file deployer can offer.
fn embedded_rows(rows: &mut Vec<Prerequisite>, capsule: &Capsule) {
    let image = capsule.image();
    let detail = match image {
        Some(m) => format!(
            "{} files, {IMAGE_MEMBER} {} MiB, sha256 {}",
            capsule.members.len(),
            m.bytes.len() / (1024 * 1024),
            &sha256_hex(m.bytes)[..12]
        ),
        None => format!("{IMAGE_MEMBER} is not embedded"),
    };
    row(
        rows,
        "Embedded appliance capsule",
        "everything the Raspberry Pi receives",
        true,
        image.is_some(),
        detail,
        "",
        None,
    );
}

pub fn probe_prerequisites(
    project_root: Option<&str>,
    capsule: &Capsule,
    cancel: Option<&Cancel>,
) -> Vec<Prerequisite> {
    let mut rows = Vec::new();
    let Some(root) = project_root else {
        embedded_rows(&mut rows, capsule);
        return rows;
    };
    engine_row(&mut rows, cancel);
    shell_row(&mut rows);
    if !ON_WINDOWS {
        const NAME: &str = "GNU Make";
        const PURPOSE: &str = "the documented entry point for the image build";
        match find_executable("make") {
            Some(m) => row(&mut rows, NAME, PURPOSE, false, true, m, "", None),
            None => row(
                &mut rows,
                NAME,
                PURPOSE,
                false,
                false,
                "make is not on PATH".into(),
                "Optional. Without it the build calls scripts/build-arm64.sh directly.",
                None,
            ),
        }
    }
    python_row(&mut rows);
    project_rows(&mut rows, root);
    rows
}

/// The distinct packages that would satisfy the unsatisfied rows.
pub fn missing_packages(rows: &[Prerequisite]) -> Vec<&'static Package> {
    let mut out: Vec<&'static Package> = Vec::new();
    for pkg in rows.iter().filter(|r| !r.satisfied).filter_map(|r| r.package) {
        if !out.iter().any(|p| std::ptr::eq(*p, pkg)) {
            out.push(pkg);
        }
    }
    out
}

pub fn report_install(
    package: &Package,
    result: &ProcessResult,
    p: Option<Progress<'_>>,
) -> Result<()> {
    if result.exit_code == 0 {
        report(p, &format!("Installed {}.", package.name));
        return Ok(());
    }
    if result.exit_code == WINGET_ALREADY_INSTALLED {
        report(p, &format!("{} is already installed.", package.name));
        return Ok(());
    }
    Err(Error::new(format!(
        "installing {} failed (winget exit {}):\n{}",
        package.name,
        result.exit_code,
        result.output.trim()
    )))
}

pub fn install_packages(
    packages: &[&Package],
    cancel: Option<&Cancel>,
    p: Option<Progress<'_>>,
) -> Result<()> {
    if !ON_WINDOWS {
        return Err(Error::new(
            "automatic prerequisite installation is Windows-only; on this system run `make install`",
        ));
    }
    if packages.is_empty() {
        report(p, "Every prerequisite with a winget package is already installed.");
        return Ok(());
    }
    let winget = find_executable("winget").ok_or_else(|| {
        Error::new(
            "winget was not found. Install \"App Installer\" from the Microsoft Store, then \
             re-check prerequisites.",
        )
    })?;
    let cwd = sys::current_dir();
    report(p, "Windows will ask to approve each installer. Approve the UAC prompt to continue.");
    for pkg in packages {
        report(p, &format!("Installing {} ({})...", pkg.name, pkg.id));
        let args = [
            "install",
            "--exact",
            "--id",
            pkg.id,
            "--source",
            "winget",
            "--accept-package-agreements",
            "--accept-source-agreements",
        ];
        let result = sys::run_process(&winget, &args, Some(&cwd), &[], cancel)?;
        report_install(pkg, &result, p)?;
    }
    report(
        p,
        "Prerequisites installed. Docker Desktop must be started once, and a sign-out or restart \
         may be needed before new tools appear on PATH.",
    );
    Ok(())
}

// --------------------------------------------------------------- emulation

/// Whether an ARM64 probe ran, and ran on an ARM64 kernel. The machine name
/// is the last non-empty line: an engine that pulls the image first prints
/// progress ahead of the answer.
pub fn reports_aarch64(result: &ProcessResult) -> bool {
    result.exit_code == 0
        && result.output.split('\n').map(str::trim).rfind(|l| !l.is_empty()) == Some("aarch64")
}

pub fn emulation_failure(kind: &str, windows: bool, repaired: bool, output: &str) -> String {
    let remedy = if !windows {
        "Register it once with `make setup-arm64-emulation`, then re-check."
    } else if repaired {
        "The emulator was registered from the pinned installer image and linux/arm64 still will \
         not run. Docker Desktop must be running its Linux engine, not Windows containers: check \
         Settings > General, then restart Docker Desktop and re-check."
    } else {
        "Start Docker Desktop's Linux engine, then re-check."
    };
    let mut out = format!("{kind} cannot execute linux/arm64 containers here.\n{remedy}");
    let t = output.trim();
    if !t.is_empty() {
        out.push('\n');
        out.push_str(t);
    }
    out
}

fn engine_run(engine: &str, args: &[&str], cancel: Option<&Cancel>) -> Result<ProcessResult> {
    sys::run_process(engine, args, Some(&sys::current_dir()), &[], cancel)
}

/// `uname -m` is passed as the container's command rather than through
/// `--entrypoint /bin/sh`, matching scripts/check-arm64-emulation.sh: Git
/// Bash rewrites a leading-slash argument into a Windows path before
/// docker.exe ever sees it.
fn probe_arm64(engine: &str, cancel: Option<&Cancel>) -> Result<ProcessResult> {
    engine_run(
        engine,
        &["run", "--rm", "--platform", "linux/arm64", EMULATION_PROBE_IMAGE, "uname", "-m"],
        cancel,
    )
}

pub fn ensure_arm64_emulation(cancel: Option<&Cancel>, p: Option<Progress<'_>>) -> Result<()> {
    let (engine, kind) = find_container_engine().ok_or_else(|| {
        Error::new("no container engine is installed, so ARM64 emulation cannot be checked")
    })?;
    if kind == "docker" && engine_run(&engine, &["buildx", "version"], cancel)?.exit_code != 0 {
        return Err(Error::new(
            "Docker buildx is not available, so the ARM64 image cannot be exported",
        ));
    }
    report(p, "Running a pinned ARM64 container to confirm emulation is registered...");
    let result = probe_arm64(&engine, cancel)?;
    if reports_aarch64(&result) {
        report(p, "ARM64 emulation is registered on this workstation.");
        return Ok(());
    }
    if !ON_WINDOWS {
        return Err(Error::new(emulation_failure(kind, false, false, &result.output)));
    }
    report(
        p,
        "ARM64 emulation is not registered. Installing it into the container engine with the \
         pinned binfmt image...",
    );
    let install = engine_run(
        &engine,
        &["run", "--rm", "--privileged", BINFMT_INSTALLER_IMAGE, "--install", BINFMT_ARCHITECTURES],
        cancel,
    )?;
    if install.exit_code != 0 {
        return Err(Error::new(format!(
            "registering ARM64 emulation with {kind} failed (exit {}).\nDocker Desktop must be \
             running its Linux engine for this to work.\n{}",
            install.exit_code,
            install.output.trim()
        )));
    }
    report(p, "Re-running the pinned ARM64 container...");
    let result = probe_arm64(&engine, cancel)?;
    if !reports_aarch64(&result) {
        return Err(Error::new(emulation_failure(kind, true, true, &result.output)));
    }
    report(
        p,
        "ARM64 emulation is registered. The container engine forgets this when it restarts, so a \
         deployment that builds the image registers it again.",
    );
    Ok(())
}
