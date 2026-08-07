#![forbid(unsafe_code)]

use clap::{Args, Parser, Subcommand};
use omt_deployer_core::{
    AuthMethod, Connection, DeployOptions, ManagementAction, Secret, WifiSettings, derive_wpa_psk,
    load_manifest, validate_connection, validate_options, validate_wifi,
};
use serde::{Deserialize, Serialize};
use std::io::{self, Read};
use std::path::PathBuf;

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
    SetupEmulation,
    Deploy(DeployArgs),
    Status,
    Logs,
    Restart,
    Wifi(WifiArgs),
}
#[derive(Args)]
struct DeployArgs {
    #[arg(long, default_value = "/opt/omt-client")]
    remote_directory: String,
    #[arg(long, default_value = "omt-client")]
    image_name: String,
    #[arg(long, default_value = "omt-client-arm64.tar.gz")]
    tarball_name: String,
    #[arg(long)]
    no_build: bool,
}
#[derive(Args)]
struct WifiArgs {
    #[arg(long)]
    ssid: String,
    #[arg(long)]
    no_connect: bool,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretInput {
    password: Option<String>,
    key_passphrase: Option<String>,
    sudo_password: Option<String>,
    wifi_password: Option<String>,
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
fn read_secrets(enabled: bool) -> Result<SecretInput, String> {
    if !enabled {
        return Ok(SecretInput::default());
    }
    let mut input = String::new();
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
        sudo_password: secret(values.sudo_password)?,
    };
    validate_connection(&connection).map_err(|e| e.to_string())?;
    Ok(connection)
}
fn needs_connection(command: &Command) -> bool {
    !matches!(command, Command::Check | Command::SetupEmulation)
}
fn run(cli: Cli) -> Result<(), (i32, String)> {
    let mut secrets = read_secrets(cli.secrets_stdin).map_err(|e| (2, e))?;
    let _connection = if needs_connection(&cli.command) {
        Some(
            connection(
                &cli,
                SecretInput {
                    password: secrets.password.take(),
                    key_passphrase: secrets.key_passphrase.take(),
                    sudo_password: secrets.sudo_password.take(),
                    wifi_password: None,
                },
            )
            .map_err(|e| (2, e))?,
        )
    } else {
        None
    };
    match &cli.command {
        Command::Check => {
            let root = cli
                .project
                .as_ref()
                .ok_or_else(|| (2, "--project is required".into()))?;
            load_manifest(&root.join("deploy/manifest-v3.txt")).map_err(|e| (1, e.to_string()))?;
            emit(
                cli.json,
                "result",
                "Project capsule passed local validation.",
                Some(true),
            );
        }
        Command::SetupEmulation => emit(
            cli.json,
            "result",
            "Run `make setup-arm64-emulation` with administrator approval.",
            Some(true),
        ),
        Command::Deploy(args) => {
            let options = DeployOptions {
                project_root: cli
                    .project
                    .clone()
                    .ok_or_else(|| (2, "--project is required".into()))?,
                remote_directory: args.remote_directory.clone(),
                image_name: args.image_name.clone(),
                tarball_name: args.tarball_name.clone(),
                build_image: !args.no_build,
            };
            validate_options(&options, true).map_err(|e| (2, e.to_string()))?;
            emit(
                cli.json,
                "error",
                "SSH deployment adapter is unavailable in this build.",
                Some(false),
            );
            return Err((1, "operational failure".into()));
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
            };
            validate_wifi(&settings).map_err(|e| (2, e.to_string()))?;
            let _psk = derive_wpa_psk(&settings.ssid, &settings.password)
                .map_err(|e| (1, e.to_string()))?;
            emit(
                cli.json,
                "error",
                "SSH Wi-Fi adapter is unavailable in this build.",
                Some(false),
            );
            return Err((1, "operational failure".into()));
        }
        Command::Status | Command::Logs | Command::Restart => {
            let action = match cli.command {
                Command::Status => ManagementAction::Status,
                Command::Logs => ManagementAction::Logs,
                _ => ManagementAction::Restart,
            };
            let _fixed = action.remote_argv();
            emit(
                cli.json,
                "error",
                "SSH management adapter is unavailable in this build.",
                Some(false),
            );
            return Err((1, "operational failure".into()));
        }
    }
    Ok(())
}
fn main() {
    let cli = Cli::parse();
    let json = cli.json;
    if let Err((code, message)) = run(cli) {
        if message != "operational failure" {
            if json {
                emit(true, "error", &message, Some(false));
            } else {
                eprintln!("{message}");
            }
        }
        std::process::exit(code);
    }
}
