#![forbid(unsafe_code)]

use clap::{Args, Parser, Subcommand};
use omt_deployer_core::{
    AlpineSetupSettings, AuthMethod, Connection, DeployOptions, IMAGE_MEMBER, ManagementAction,
    ON_WINDOWS, Prerequisite, Secret, WifiSettings, alpine_setup, apply_wifi, change_web_password,
    deploy, embedded_image, embedded_members, ensure_arm64_emulation, install_packages,
    load_manifest, manage, missing_packages, prerequisites, set_hostname, sha256_bytes,
    valid_appliance_hostname, validate_alpine_setup, validate_connection, validate_options,
    validate_web_password, validate_wifi,
};
use serde::{Deserialize, Serialize};
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use zeroize::Zeroizing;

const VERSION: &str = match option_env!("RPI_OMT_CLIENT_VERSION") {
    Some(value) => value,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser)]
#[command(version = VERSION, about = "Secure Raspberry Pi OMT appliance deployment client")]
struct Cli {
    #[arg(long, global = true)]
    host: Option<String>,
    #[arg(long, global = true)]
    username: Option<String>,
    #[arg(long, default_value_t = 22, global = true)]
    port: u16,
    #[arg(long, global = true)]
    key: Option<PathBuf>,
    #[arg(long, global = true)]
    known_hosts: Option<PathBuf>,
    /// Take the deployment capsule from this checkout instead of the copy
    /// compiled into this program. The developer override: an operator's
    /// deployment needs no source tree at all.
    #[arg(long, global = true)]
    project: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true, conflicts_with = "interactive_secrets")]
    secrets_stdin: bool,
    #[arg(long, global = true)]
    interactive_secrets: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Check,
    /// Report the local tooling a deployment needs from this workstation.
    Prerequisites(PrerequisiteArgs),
    /// Make this workstation able to run ARM64 containers. On Windows it
    /// registers the emulator in the container engine and verifies it; on
    /// Linux it names the target that installs it persistently.
    SetupEmulation,
    /// Install Alpine in persistent sys mode on a factory Raspberry Pi image.
    AlpineSetup(AlpineSetupArgs),
    Deploy(DeployArgs),
    Status,
    Logs,
    Restart,
    /// Reboot the Raspberry Pi operating system after acknowledging the request.
    Reboot,
    /// Change the Web GUI password and revoke every existing Web session.
    WebPassword,
    /// Rename an installed appliance, in the host and in the Web GUI.
    Hostname(HostnameArgs),
    Wifi(WifiArgs),
}
#[derive(Args)]
struct PrerequisiteArgs {
    /// Install the missing prerequisites that have a winget package. Windows
    /// only, and each installer raises its own approval prompt.
    #[arg(long)]
    install: bool,
    /// Also prove the container engine can run ARM64 containers, registering
    /// the emulator first on Windows, where the engine needs that done for it.
    #[arg(long)]
    check_emulation: bool,
}
#[derive(Args)]
struct AlpineSetupArgs {
    #[arg(long)]
    hostname: String,
    #[arg(long)]
    ssid: Option<String>,
}
#[derive(Args)]
struct DeployArgs {
    #[arg(long, default_value = "/opt/omt-client")]
    remote_directory: String,
    /// Rebuild the ARM64 appliance image from --project before uploading it.
    /// Needs the container engine and ARM64 emulation `prerequisites` reports.
    #[arg(long, requires = "project")]
    rebuild_image: bool,
}
#[derive(Args)]
struct HostnameArgs {
    /// The appliance's new name: one DNS label, 1-63 characters.
    #[arg(long)]
    name: String,
}
#[derive(Args)]
struct WifiArgs {
    #[arg(long)]
    ssid: String,
    #[arg(long)]
    no_connect: bool,
    /// Remove every other saved Wi-Fi profile, verifying the new one first
    /// unless --no-connect defers association until the next boot.
    #[arg(long)]
    replace_existing_profiles: bool,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretInput {
    password: Option<String>,
    key_passphrase: Option<String>,
    sudo_password: Option<String>,
    bootstrap_root_password: Option<String>,
    wifi_password: Option<String>,
    web_password: Option<String>,
    root_password: Option<String>,
    pi_password: Option<String>,
}
#[derive(Serialize)]
struct OutputLine<'a> {
    event: &'a str,
    message: &'a str,
    success: Option<bool>,
}

fn emit(json: bool, event: &str, message: &str, success: Option<bool>) {
    if json {
        println!(
            "{}",
            serde_json::to_string(&OutputLine {
                event,
                message,
                success
            })
            .unwrap_or_else(|_| "{\"event\":\"error\"}".into())
        );
    } else {
        println!("{message}");
    }
}
/// Reads the bounded `--secrets-stdin` channel.
///
/// The buffer is `Zeroizing` because it holds every secret at once in plain
/// text. The per-field `String`s serde produces are moved into `Secret`, which
/// zeroizes the same allocation, but this document is the one copy that would
/// otherwise be freed intact.
fn read_secrets(enabled: bool) -> Result<SecretInput, String> {
    if !enabled {
        return Ok(SecretInput::default());
    }
    let mut input = Zeroizing::new(String::new());
    io::stdin()
        .take(16 * 1024 + 1)
        .read_to_string(&mut input)
        .map_err(|e| e.to_string())?;
    if input.len() > 16 * 1024 {
        return Err("secrets JSON exceeds 16 KiB".into());
    }
    serde_json::from_str(&input).map_err(|e| format!("invalid secrets JSON: {e}"))
}
fn secret(value: Option<String>) -> Result<Option<Secret>, String> {
    value
        .map(Secret::new)
        .transpose()
        .map_err(|e| e.to_string())
}
fn connection(cli: &Cli, mut values: SecretInput) -> Result<Connection, String> {
    let host = cli.host.clone().ok_or("--host is required")?;
    let username = cli.username.clone().ok_or("--username is required")?;
    if cli.interactive_secrets && cli.key.is_none() && values.password.is_none() {
        values.password =
            Some(rpassword::prompt_password("SSH password: ").map_err(|e| e.to_string())?);
    }
    if cli.interactive_secrets && values.sudo_password.is_none() {
        let prompted = rpassword::prompt_password("sudo password (empty if passwordless): ")
            .map_err(|e| e.to_string())?;
        if !prompted.is_empty() {
            values.sudo_password = Some(prompted);
        }
    }
    if cli.interactive_secrets && values.bootstrap_root_password.is_none() {
        let prompted = rpassword::prompt_password(
            "root password for clean-Alpine bootstrap (empty if not needed): ",
        )
        .map_err(|e| e.to_string())?;
        if !prompted.is_empty() {
            values.bootstrap_root_password = Some(prompted);
        }
    }
    let connection = Connection {
        host,
        username,
        port: cli.port,
        auth: if cli.key.is_some() {
            AuthMethod::Key
        } else {
            AuthMethod::Password
        },
        password: secret(values.password)?,
        key_path: cli.key.clone(),
        key_passphrase: secret(values.key_passphrase)?,
        known_hosts_path: cli.known_hosts.clone(),
        sudo_password: secret(values.sudo_password)?,
        bootstrap_root_password: secret(values.bootstrap_root_password)?,
    };
    validate_connection(&connection).map_err(|e| e.to_string())?;
    Ok(connection)
}
fn needs_connection(command: &Command) -> bool {
    !matches!(
        command,
        Command::Check | Command::Prerequisites(_) | Command::SetupEmulation
    )
}

/// What this program would upload with no `--project` given.
///
/// The digest is the point: a single-file deployer is otherwise opaque about
/// which appliance build it carries, and this is the same value the Pi's own
/// `sha256sum` is held to during the deployment.
fn embedded_capsule_report() -> Result<String, (i32, String)> {
    let image = embedded_image()
        .ok_or_else(|| (1, format!("this deployer was built without {IMAGE_MEMBER}")))?;
    Ok(format!(
        "Embedded capsule: {} members, {IMAGE_MEMBER} {} MiB, sha256 {}.",
        embedded_members().len(),
        image.bytes.len() / (1024 * 1024),
        sha256_bytes(image.bytes)
    ))
}

/// One prerequisite as a line an operator or a wrapping script can read.
fn prerequisite_line(row: &Prerequisite) -> String {
    let mark = if row.satisfied {
        "ok"
    } else if row.required {
        "MISSING"
    } else {
        "optional"
    };
    let mut line = format!("[{mark}] {}: {}", row.name, row.detail);
    if !row.satisfied && !row.remedy.is_empty() {
        line.push_str(" -- ");
        line.push_str(&row.remedy);
    }
    line
}
fn progress_emitter(json: bool) -> impl FnMut(&str) {
    move |message: &str| {
        for line in message.lines() {
            if !line.is_empty() {
                emit(json, "progress", line, None);
            }
        }
    }
}
fn run(cli: Cli) -> Result<(), (i32, String)> {
    let mut secrets = read_secrets(cli.secrets_stdin).map_err(|e| (2, e))?;
    let connection = if needs_connection(&cli.command) {
        Some(
            connection(
                &cli,
                SecretInput {
                    password: secrets.password.take(),
                    key_passphrase: secrets.key_passphrase.take(),
                    sudo_password: secrets.sudo_password.take(),
                    bootstrap_root_password: secrets.bootstrap_root_password.take(),
                    wifi_password: None,
                    web_password: None,
                    root_password: None,
                    pi_password: None,
                },
            )
            .map_err(|e| (2, e))?,
        )
    } else {
        None
    };
    let cancellation = Arc::new(AtomicBool::new(false));
    match &cli.command {
        // Without --project this validates the capsule compiled into this
        // program, which is what an operator's deployment will upload.
        Command::Check => {
            let message = match cli.project.as_ref() {
                Some(root) => {
                    let members = load_manifest(&root.join("deploy/manifest-v3.txt"))
                        .map_err(|e| (1, e.to_string()))?;
                    format!(
                        "Project capsule passed local validation: {} members in {}.",
                        members.len(),
                        root.display()
                    )
                }
                None => embedded_capsule_report()?,
            };
            emit(cli.json, "result", &message, Some(true));
        }
        Command::Prerequisites(args) => {
            let root = cli.project.clone();
            let mut progress = progress_emitter(cli.json);
            if args.install {
                let rows = prerequisites(root.as_deref(), &cancellation);
                install_packages(&missing_packages(&rows), &cancellation, &mut progress)
                    .map_err(|e| (1, e.to_string()))?;
            }
            // Probed after any installation, so the report describes the
            // machine as it now is rather than as it was.
            let rows = prerequisites(root.as_deref(), &cancellation);
            for row in &rows {
                emit(cli.json, "progress", &prerequisite_line(row), None);
            }
            if args.check_emulation {
                ensure_arm64_emulation(&cancellation, &mut progress)
                    .map_err(|e| (1, e.to_string()))?;
            }
            let blocking = rows.iter().filter(|row| row.blocking()).count();
            if blocking > 0 {
                return Err((
                    1,
                    format!(
                        "{blocking} required workstation prerequisite(s) are missing{}",
                        if ON_WINDOWS && !args.install {
                            "; rerun with --install to install the ones winget can supply"
                        } else {
                            ""
                        }
                    ),
                ));
            }
            emit(
                cli.json,
                "result",
                if root.is_some() {
                    "This workstation can build and deploy the appliance."
                } else {
                    "This deployer can deploy the appliance it carries."
                },
                Some(true),
            );
        }
        // On Windows there is no `make setup-arm64-emulation` to run: that
        // target installs a systemd binfmt unit on a Linux host. The engine's
        // own VM is where the handler belongs there, and this puts it there.
        Command::SetupEmulation => {
            if ON_WINDOWS {
                let mut progress = progress_emitter(cli.json);
                ensure_arm64_emulation(&cancellation, &mut progress)
                    .map_err(|e| (1, e.to_string()))?;
                emit(
                    cli.json,
                    "result",
                    "This workstation can run ARM64 containers.",
                    Some(true),
                );
            } else {
                emit(
                    cli.json,
                    "result",
                    "Run `make setup-arm64-emulation` with administrator approval.",
                    Some(true),
                );
            }
        }
        Command::AlpineSetup(args) => {
            let root_value = if let Some(value) = secrets.root_password.take() {
                value
            } else if cli.interactive_secrets {
                rpassword::prompt_password("Root password: ").map_err(|e| (1, e.to_string()))?
            } else {
                return Err((
                    2,
                    "root_password is required through --secrets-stdin or an interactive prompt"
                        .into(),
                ));
            };
            let pi_value = if let Some(value) = secrets.pi_password.take() {
                value
            } else if cli.interactive_secrets {
                rpassword::prompt_password("pi password: ").map_err(|e| (1, e.to_string()))?
            } else {
                return Err((
                    2,
                    "pi_password is required through --secrets-stdin or an interactive prompt"
                        .into(),
                ));
            };
            let wifi = if let Some(ssid) = args.ssid.clone() {
                let password = if let Some(value) = secrets.wifi_password.take() {
                    Secret::new(value).map_err(|e| (2, e.to_string()))?
                } else if cli.interactive_secrets {
                    Secret::new(
                        rpassword::prompt_password("Wi-Fi password: ")
                            .map_err(|e| (1, e.to_string()))?,
                    )
                    .map_err(|e| (2, e.to_string()))?
                } else {
                    return Err((
                        2,
                        "wifi_password is required through --secrets-stdin or an interactive prompt"
                            .into(),
                    ));
                };
                Some(WifiSettings {
                    ssid,
                    password,
                    connect: false,
                    preserve_existing_profiles: true,
                })
            } else {
                None
            };
            let settings = AlpineSetupSettings {
                hostname: args.hostname.clone(),
                wifi,
                root_password: Secret::new(root_value).map_err(|e| (2, e.to_string()))?,
                pi_password: Secret::new(pi_value).map_err(|e| (2, e.to_string()))?,
            };
            validate_alpine_setup(&settings).map_err(|e| (2, e.to_string()))?;
            let connection = connection.ok_or_else(|| (2, "connection required".into()))?;
            let mut progress = progress_emitter(cli.json);
            alpine_setup(
                &connection,
                &settings,
                cli.project.as_deref(),
                &cancellation,
                &mut progress,
            )
            .map_err(|e| (1, e.to_string()))?;
            emit(
                cli.json,
                "result",
                "Alpine sys-mode install completed successfully.",
                Some(true),
            );
        }
        Command::Deploy(args) => {
            let options = DeployOptions {
                project_root: cli.project.clone(),
                remote_directory: args.remote_directory.clone(),
                rebuild_image: args.rebuild_image,
            };
            validate_options(&options).map_err(|e| (2, e.to_string()))?;
            let connection = connection.ok_or_else(|| (2, "connection required".into()))?;
            let mut progress = progress_emitter(cli.json);
            deploy(&connection, &options, &cancellation, &mut progress)
                .map_err(|e| (1, e.to_string()))?;
            emit(
                cli.json,
                "result",
                "Deployment completed successfully.",
                Some(true),
            );
        }
        Command::Wifi(args) => {
            let password = if let Some(value) = secrets.wifi_password.take() {
                Secret::new(value).map_err(|e| (2, e.to_string()))?
            } else if cli.interactive_secrets {
                Secret::new(
                    rpassword::prompt_password("Wi-Fi password: ")
                        .map_err(|e| (1, e.to_string()))?,
                )
                .map_err(|e| (2, e.to_string()))?
            } else {
                return Err((
                    2,
                    "wifi_password is required through --secrets-stdin or an interactive prompt"
                        .into(),
                ));
            };
            let settings = WifiSettings {
                ssid: args.ssid.clone(),
                password,
                connect: !args.no_connect,
                preserve_existing_profiles: !args.replace_existing_profiles,
            };
            validate_wifi(&settings).map_err(|e| (2, e.to_string()))?;
            let connection = connection.ok_or_else(|| (2, "connection required".into()))?;
            let mut progress = progress_emitter(cli.json);
            apply_wifi(&connection, &settings, &cancellation, &mut progress)
                .map_err(|e| (1, e.to_string()))?;
            emit(cli.json, "result", "Wi-Fi settings applied.", Some(true));
        }
        Command::WebPassword => {
            let value = if let Some(value) = secrets.web_password.take() {
                value
            } else if cli.interactive_secrets {
                let first = rpassword::prompt_password("New Web GUI password: ")
                    .map_err(|e| (1, e.to_string()))?;
                let confirmation = rpassword::prompt_password("Confirm Web GUI password: ")
                    .map_err(|e| (1, e.to_string()))?;
                if first != confirmation {
                    return Err((2, "Web GUI password confirmation does not match".into()));
                }
                first
            } else {
                return Err((
                    2,
                    "web_password is required through --secrets-stdin or an interactive prompt"
                        .into(),
                ));
            };
            let password = Secret::new(value).map_err(|e| (2, e.to_string()))?;
            validate_web_password(&password).map_err(|e| (2, e.to_string()))?;
            let connection = connection.ok_or_else(|| (2, "connection required".into()))?;
            let mut progress = progress_emitter(cli.json);
            change_web_password(&connection, &password, &cancellation, &mut progress)
                .map_err(|e| (1, e.to_string()))?;
            emit(cli.json, "result", "Web GUI password changed.", Some(true));
        }
        Command::Hostname(args) => {
            // Rejected here as a usage error rather than reaching the Pi as a
            // failed operation: exit 2 is what every other bad argument gives.
            if !valid_appliance_hostname(&args.name) {
                return Err((
                    2,
                    "--name must be one DNS label of 1-63 characters: letters, digits, \
                     and inner hyphens"
                        .into(),
                ));
            }
            let connection = connection.ok_or_else(|| (2, "connection required".into()))?;
            let mut progress = progress_emitter(cli.json);
            set_hostname(
                &connection,
                &args.name,
                cli.project.as_deref(),
                &cancellation,
                &mut progress,
            )
            .map_err(|e| (1, e.to_string()))?;
            emit(
                cli.json,
                "result",
                &format!("Appliance hostname changed to {}.", args.name),
                Some(true),
            );
        }
        Command::Status | Command::Logs | Command::Restart | Command::Reboot => {
            let action = match cli.command {
                Command::Status => ManagementAction::Status,
                Command::Logs => ManagementAction::Logs,
                Command::Restart => ManagementAction::Restart,
                _ => ManagementAction::Reboot,
            };
            let connection = connection.ok_or_else(|| (2, "connection required".into()))?;
            let mut progress = progress_emitter(cli.json);
            manage(&connection, action, &cancellation, &mut progress)
                .map_err(|e| (1, e.to_string()))?;
            emit(
                cli.json,
                "result",
                "Remote management action succeeded.",
                Some(true),
            );
        }
    }
    Ok(())
}
fn main() {
    let cli = Cli::parse();
    let json = cli.json;
    if let Err((code, message)) = run(cli) {
        if json {
            emit(true, "error", &message, Some(false));
        } else {
            eprintln!("{message}");
        }
        std::process::exit(code);
    }
}
