//! rpi-omt-deploy: the scriptable deployer.
//!
//! Exit 2 is a usage failure that leaves the command unrun; exit 1 is a
//! command that ran and failed. Secrets arrive as JSON on stdin
//! (--secrets-stdin) or through terminal prompts (--interactive-secrets),
//! never as arguments. --json makes every output line one JSON object.

#![forbid(unsafe_code)]
// The parsed command line mirrors its flags one to one.
#![allow(clippy::struct_excessive_bools)]

use std::fmt::Write as _;
use std::io::Write as _;
use std::process::ExitCode;

use omt_capsule::CAPSULE;
use omt_common::{Error, json};
use omt_deploy::core::{
    self, Action, AlpineSettings, Auth, Connection, DeployOptions, SdSettings, Secret, WifiSettings,
};
use omt_deploy::sys::{self, Cancel};
use omt_deploy::{ops, sd_card, tools};

const SECRETS_LIMIT: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cmd {
    None,
    Check,
    Prerequisites,
    SetupEmulation,
    PrepareSd,
    AlpineSetup,
    Deploy,
    Status,
    Logs,
    Restart,
    Reboot,
    WebPassword,
    Hostname,
    Wifi,
}

const COMMANDS: [(&str, Cmd, &str); 13] = [
    ("check", Cmd::Check, "Validate the capsule this deployment would upload"),
    (
        "prerequisites",
        Cmd::Prerequisites,
        "Report the local tooling a deployment needs from this workstation",
    ),
    ("setup-emulation", Cmd::SetupEmulation, "Make this workstation able to run ARM64 containers"),
    (
        "prepare-sd",
        Cmd::PrepareSd,
        "Prepare a flashed Alpine boot partition for its first headless boot",
    ),
    (
        "alpine-setup",
        Cmd::AlpineSetup,
        "Install Alpine in persistent sys mode on a factory Raspberry Pi image",
    ),
    ("deploy", Cmd::Deploy, "Upload, verify, install, and start the appliance"),
    ("status", Cmd::Status, "Show the appliance container's status"),
    ("logs", Cmd::Logs, "Show the appliance's recent logs"),
    ("restart", Cmd::Restart, "Restart the appliance service"),
    (
        "reboot",
        Cmd::Reboot,
        "Reboot the Raspberry Pi operating system after acknowledging the request",
    ),
    (
        "web-password",
        Cmd::WebPassword,
        "Change the Web GUI password and revoke every existing Web session",
    ),
    ("hostname", Cmd::Hostname, "Rename an installed appliance, in the host and in the Web GUI"),
    ("wifi", Cmd::Wifi, "Apply Wi-Fi settings on the appliance"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Host,
    Username,
    Key,
    KnownHosts,
    Project,
    Json,
    SecretsStdin,
    InteractiveSecrets,
    Install,
    CheckEmulation,
    BootDirectory,
    Ssid,
    Country,
    Hostname,
    RemoteDirectory,
    RebuildImage,
    Name,
    NoConnect,
    ReplaceExistingProfiles,
}

/// (name, takes a value, the command it belongs to or `Cmd::None` for a
/// global option, the field it sets)
const OPTIONS: [(&str, bool, Cmd, Field); 21] = [
    ("host", true, Cmd::None, Field::Host),
    ("username", true, Cmd::None, Field::Username),
    ("key", true, Cmd::None, Field::Key),
    ("known-hosts", true, Cmd::None, Field::KnownHosts),
    ("project", true, Cmd::None, Field::Project),
    ("json", false, Cmd::None, Field::Json),
    ("secrets-stdin", false, Cmd::None, Field::SecretsStdin),
    ("interactive-secrets", false, Cmd::None, Field::InteractiveSecrets),
    ("install", false, Cmd::Prerequisites, Field::Install),
    ("check-emulation", false, Cmd::Prerequisites, Field::CheckEmulation),
    ("boot-directory", true, Cmd::PrepareSd, Field::BootDirectory),
    ("ssid", true, Cmd::PrepareSd, Field::Ssid),
    ("country", true, Cmd::PrepareSd, Field::Country),
    ("hostname", true, Cmd::AlpineSetup, Field::Hostname),
    ("ssid", true, Cmd::AlpineSetup, Field::Ssid),
    ("remote-directory", true, Cmd::Deploy, Field::RemoteDirectory),
    ("rebuild-image", false, Cmd::Deploy, Field::RebuildImage),
    ("name", true, Cmd::Hostname, Field::Name),
    ("ssid", true, Cmd::Wifi, Field::Ssid),
    ("no-connect", false, Cmd::Wifi, Field::NoConnect),
    ("replace-existing-profiles", false, Cmd::Wifi, Field::ReplaceExistingProfiles),
];

#[derive(Debug, Default)]
struct Cli {
    host: Option<String>,
    username: Option<String>,
    key: Option<String>,
    known_hosts: Option<String>,
    project: Option<String>,
    port: u16,
    json: bool,
    secrets_stdin: bool,
    interactive_secrets: bool,
    cmd: Option<Cmd>,
    install: bool,
    check_emulation: bool,
    hostname: Option<String>,
    ssid: Option<String>,
    boot_directory: Option<String>,
    country: Option<String>,
    remote_directory: Option<String>,
    name: Option<String>,
    rebuild_image: bool,
    no_connect: bool,
    replace_existing_profiles: bool,
}

impl Cli {
    fn cmd(&self) -> Cmd {
        self.cmd.unwrap_or(Cmd::None)
    }

    fn set_flag(&mut self, f: Field) {
        match f {
            Field::Json => self.json = true,
            Field::SecretsStdin => self.secrets_stdin = true,
            Field::InteractiveSecrets => self.interactive_secrets = true,
            Field::Install => self.install = true,
            Field::CheckEmulation => self.check_emulation = true,
            Field::RebuildImage => self.rebuild_image = true,
            Field::NoConnect => self.no_connect = true,
            Field::ReplaceExistingProfiles => self.replace_existing_profiles = true,
            _ => {}
        }
    }

    fn set_value(&mut self, f: Field, v: String) {
        let slot = match f {
            Field::Host => &mut self.host,
            Field::Username => &mut self.username,
            Field::Key => &mut self.key,
            Field::KnownHosts => &mut self.known_hosts,
            Field::Project => &mut self.project,
            Field::BootDirectory => &mut self.boot_directory,
            Field::Ssid => &mut self.ssid,
            Field::Hostname => &mut self.hostname,
            Field::Name => &mut self.name,
            Field::Country => &mut self.country,
            Field::RemoteDirectory => &mut self.remote_directory,
            _ => return,
        };
        *slot = Some(v);
    }
}

// ------------------------------------------------------------------ output

fn emit(json: bool, event: &str, message: &str, success: Option<bool>) {
    let mut stdout = std::io::stdout().lock();
    if !json {
        let _ = writeln!(stdout, "{message}");
        let _ = stdout.flush();
        return;
    }
    let mut line = String::from("{\"event\":");
    json::write_string(&mut line, event);
    line.push_str(",\"message\":");
    json::write_string(&mut line, message);
    line.push_str(",\"success\":");
    line.push_str(match success {
        None => "null",
        Some(true) => "true",
        Some(false) => "false",
    });
    line.push('}');
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

// ------------------------------------------------------------------- usage

fn help_text(cmd: Cmd) -> String {
    let mut out = String::new();
    if cmd == Cmd::None {
        out.push_str(
            "Secure Raspberry Pi OMT appliance deployment client\n\nUsage: rpi-omt-deploy \
             [OPTIONS] <COMMAND>\n\nCommands:\n",
        );
        for (name, _, about) in COMMANDS {
            let _ = writeln!(out, "  {name:<16} {about}");
        }
        let _ = writeln!(out, "  {:<16} Print this message or the help of a command", "help");
    } else {
        for (name, c, about) in COMMANDS {
            if c == cmd {
                let _ = writeln!(out, "{about}\n\nUsage: rpi-omt-deploy {name} [OPTIONS]");
            }
        }
        out.push_str("\nCommand options:\n");
        out.push_str(match cmd {
            Cmd::Prerequisites => {
                "      --install            Install missing prerequisites with winget (Windows \
                 only)\n      --check-emulation    Also prove the engine runs ARM64 containers\n"
            }
            Cmd::PrepareSd => {
                "      --boot-directory <PATH>  Mounted root of the Alpine boot partition\n      \
                 --ssid <SSID>\n      --country <CC>           Wi-Fi regulatory country [default: \
                 US]\n"
            }
            Cmd::AlpineSetup => "      --hostname <NAME>\n      --ssid <SSID>\n",
            Cmd::Deploy => {
                "      --remote-directory <PATH>  [default: /opt/omt-client]\n      \
                 --rebuild-image            Rebuild the ARM64 image from --project first\n"
            }
            Cmd::Hostname => "      --name <NAME>    One DNS label, 1-63 characters\n",
            Cmd::Wifi => {
                "      --ssid <SSID>\n      --no-connect\n      --replace-existing-profiles\n"
            }
            _ => "      (none)\n",
        });
    }
    out.push_str(
        "\nOptions:\n      --host <HOST>\n      --username <USERNAME>\n      --port <PORT>                \
         [default: 22]\n      --key <KEY>\n      --known-hosts <KNOWN_HOSTS>\n      --project \
         <PROJECT>          Take the capsule from this checkout\n      --json\n      \
         --secrets-stdin\n      --interactive-secrets\n  -h, --help                       Print \
         help\n  -V, --version                    Print version\n",
    );
    out
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!(
        "error: {message}\n\nUsage: rpi-omt-deploy [OPTIONS] <COMMAND>\n\nFor more information, \
         try '--help'."
    );
    ExitCode::from(2)
}

// ----------------------------------------------------------------- parsing

/// The parsed command line, or the exit status that ends the program.
fn parse(argv: &[String]) -> Result<Cli, ExitCode> {
    let mut c = Cli { port: 22, ..Cli::default() };
    let mut port_text: Option<String> = None;
    let mut seen = [false; OPTIONS.len()];
    let mut help = false;
    let mut i = 1;
    while i < argv.len() {
        let arg = argv[i].as_str();
        i += 1;
        if arg == "--help" || arg == "-h" {
            help = true;
            continue;
        }
        if arg == "--version" || arg == "-V" {
            println!("rpi-omt-deploy {}", omt_common::VERSION);
            return Err(ExitCode::SUCCESS);
        }
        if let Some(name_part) = arg.strip_prefix("--").filter(|n| !n.is_empty()) {
            let (name, inline) = match name_part.split_once('=') {
                Some((n, v)) => (n, Some(v.to_owned())),
                None => (name_part, None),
            };
            let mut take_value = |what: &str| -> Result<String, ExitCode> {
                if let Some(v) = &inline {
                    return Ok(v.clone());
                }
                if i < argv.len() {
                    i += 1;
                    return Ok(argv[i - 1].clone());
                }
                Err(usage_error(&format!("a value is required for '{what}' but none was supplied")))
            };
            if name == "port" {
                if port_text.is_some() {
                    return Err(usage_error(
                        "the argument '--port <PORT>' cannot be used multiple times",
                    ));
                }
                port_text = Some(take_value("--port <PORT>")?);
                continue;
            }
            let found = OPTIONS.iter().enumerate().find(|(_, (n, _, scope, _))| {
                *n == name && (*scope == Cmd::None || *scope == c.cmd())
            });
            let Some((index, &(_, takes_value, _, field))) = found else {
                return Err(usage_error(&format!("unexpected argument '{arg}' found")));
            };
            if seen[index] {
                return Err(usage_error(&format!(
                    "the argument '{arg}' cannot be used multiple times"
                )));
            }
            seen[index] = true;
            if takes_value {
                let v = take_value(arg)?;
                c.set_value(field, v);
            } else {
                if inline.is_some() {
                    return Err(usage_error(&format!("unexpected value for '{arg}'")));
                }
                c.set_flag(field);
            }
            continue;
        }
        if c.cmd.is_none() && !arg.starts_with('-') {
            if arg == "help" {
                print!("{}", help_text(Cmd::None));
                return Err(ExitCode::SUCCESS);
            }
            let Some(&(_, cmd, _)) = COMMANDS.iter().find(|(n, _, _)| *n == arg) else {
                return Err(usage_error(&format!("unrecognized subcommand '{arg}'")));
            };
            c.cmd = Some(cmd);
            continue;
        }
        return Err(usage_error(&format!("unexpected argument '{arg}' found")));
    }
    if help {
        print!("{}", help_text(c.cmd()));
        return Err(ExitCode::SUCCESS);
    }
    if c.cmd.is_none() {
        eprint!("{}", help_text(Cmd::None));
        return Err(ExitCode::from(2));
    }
    if let Some(text) = port_text {
        match omt_common::buf::parse_u64(text.as_bytes(), 65535).and_then(|p| u16::try_from(p).ok())
        {
            Some(p) => c.port = p,
            None => {
                return Err(usage_error(&format!("invalid value '{text}' for '--port <PORT>'")));
            }
        }
    }
    if c.secrets_stdin && c.interactive_secrets {
        return Err(usage_error(
            "the argument '--secrets-stdin' cannot be used with '--interactive-secrets'",
        ));
    }
    if c.rebuild_image && c.project.is_none() {
        return Err(usage_error(
            "the following required arguments were not provided:\n  --project <PROJECT>",
        ));
    }
    let mut missing = None;
    let cmd = c.cmd();
    if cmd == Cmd::AlpineSetup && c.hostname.is_none() {
        missing = Some("--hostname <HOSTNAME>");
    }
    if cmd == Cmd::PrepareSd && c.boot_directory.is_none() {
        missing = Some("--boot-directory <BOOT_DIRECTORY>");
    }
    if (cmd == Cmd::PrepareSd || cmd == Cmd::Wifi) && c.ssid.is_none() {
        missing = Some("--ssid <SSID>");
    }
    if cmd == Cmd::Hostname && c.name.is_none() {
        missing = Some("--name <NAME>");
    }
    if let Some(m) = missing {
        return Err(usage_error(&format!(
            "the following required arguments were not provided:\n  {m}"
        )));
    }
    c.country.get_or_insert_with(|| "US".to_owned());
    c.remote_directory.get_or_insert_with(|| "/opt/omt-client".to_owned());
    Ok(c)
}

// ----------------------------------------------------------------- secrets

const SECRET_FIELDS: [&str; 8] = [
    "password",
    "key_passphrase",
    "sudo_password",
    "bootstrap_root_password",
    "wifi_password",
    "web_password",
    "root_password",
    "pi_password",
];

/// The eight secrets, in SECRET_FIELDS order.
#[derive(Debug, Default)]
struct Secrets([Secret; 8]);

const PASSWORD: usize = 0;
const KEY_PASSPHRASE: usize = 1;
const SUDO_PASSWORD: usize = 2;
const BOOTSTRAP_ROOT_PASSWORD: usize = 3;
const WIFI_PASSWORD: usize = 4;
const WEB_PASSWORD: usize = 5;
const ROOT_PASSWORD: usize = 6;
const PI_PASSWORD: usize = 7;

/// A failure and the status it exits with.
struct Failure(u8, Error);

type Run<T> = Result<T, Failure>;

fn usage(e: impl Into<String>) -> Failure {
    Failure(2, Error::new(e))
}

fn failed(e: Error) -> Failure {
    Failure(1, e)
}

/// Reads the bounded --secrets-stdin channel. Every buffer that held the
/// document is wiped: it carries every secret at once.
fn read_secrets(enabled: bool, s: &mut Secrets) -> Run<()> {
    if !enabled {
        return Ok(());
    }
    let Some(mut input) = sys::read_stdin(SECRETS_LIMIT).map_err(|e| Failure(2, e))? else {
        return Err(usage("secrets JSON exceeds 16 KiB"));
    };
    let parsed = json::parse(&input);
    omt_common::buf::wipe_vec(&mut input);
    let mut root = match parsed {
        Ok(root) => root,
        Err(e) => return Err(usage(format!("invalid secrets JSON: {e}"))),
    };
    let result = (|| {
        if root.as_object().is_none() {
            return Err(usage("invalid secrets JSON: expected an object"));
        }
        if !root.only_keys(&SECRET_FIELDS) {
            return Err(usage("invalid secrets JSON: unknown field"));
        }
        for (i, field) in SECRET_FIELDS.iter().enumerate() {
            let Some(value) = root.get(field).filter(|v| !v.is_null()) else { continue };
            let Some(text) = value.as_str() else {
                return Err(usage(format!("invalid secrets JSON: {field} must be a string")));
            };
            s.0[i] = Secret::new(text).map_err(usage)?;
        }
        Ok(())
    })();
    root.wipe();
    result
}

fn prompt(text: &str) -> Run<Secret> {
    let mut value = sys::prompt_secret(text).map_err(failed)?;
    let secret = Secret::new(&value).map_err(usage);
    omt_common::buf::wipe_string(&mut value);
    secret
}

/// The secret from stdin, or a prompt, or a usage error naming the field.
fn require_secret(c: &Cli, s: &mut Secrets, index: usize, question: &str) -> Run<()> {
    if s.0[index].is_set() {
        return Ok(());
    }
    if c.interactive_secrets {
        s.0[index] = prompt(question)?;
        return Ok(());
    }
    Err(usage(format!(
        "{} is required through --secrets-stdin or an interactive prompt",
        SECRET_FIELDS[index]
    )))
}

fn build_connection(c: &Cli, s: &mut Secrets) -> Run<Connection> {
    let host = c.host.clone().ok_or_else(|| usage("--host is required"))?;
    let username = c.username.clone().ok_or_else(|| usage("--username is required"))?;
    if c.interactive_secrets && c.key.is_none() && !s.0[PASSWORD].is_set() {
        s.0[PASSWORD] = prompt("SSH password: ")?;
    }
    if c.interactive_secrets && !s.0[SUDO_PASSWORD].is_set() {
        let v = prompt("sudo password (empty if passwordless): ")?;
        s.0[SUDO_PASSWORD] = if v.is_empty() { Secret::absent() } else { v };
    }
    if c.interactive_secrets && !s.0[BOOTSTRAP_ROOT_PASSWORD].is_set() {
        let v = prompt("root password for clean-Alpine bootstrap (empty if not needed): ")?;
        s.0[BOOTSTRAP_ROOT_PASSWORD] = if v.is_empty() { Secret::absent() } else { v };
    }
    let conn = Connection {
        host,
        username,
        port: c.port,
        auth: if c.key.is_some() { Auth::Key } else { Auth::Password },
        password: s.0[PASSWORD].clone(),
        key_path: c.key.clone(),
        key_passphrase: s.0[KEY_PASSPHRASE].clone(),
        known_hosts_path: c.known_hosts.clone(),
        sudo_password: s.0[SUDO_PASSWORD].clone(),
        bootstrap_root_password: s.0[BOOTSTRAP_ROOT_PASSWORD].clone(),
    };
    if let Some(problem) = core::validate_connection(&conn) {
        return Err(usage(problem));
    }
    Ok(conn)
}

fn needs_connection(cmd: Cmd) -> bool {
    !matches!(cmd, Cmd::Check | Cmd::Prerequisites | Cmd::SetupEmulation | Cmd::PrepareSd)
}

fn prerequisite_line(row: &tools::Prerequisite) -> String {
    let mark = if row.satisfied {
        "ok"
    } else if row.required {
        "MISSING"
    } else {
        "optional"
    };
    let mut out = format!("[{mark}] {}: {}", row.name, row.detail);
    if !row.satisfied && !row.remedy.is_empty() {
        let _ = write!(out, " -- {}", row.remedy);
    }
    out
}

// ---------------------------------------------------------------- commands

fn run(c: &Cli) -> Run<()> {
    let mut s = Secrets::default();
    let cancel = Cancel::new(false);
    let cancel = Some(&cancel);
    let json = c.json;
    let progress = move |line: &str| emit(json, "progress", line, None);
    let p: Option<omt_deploy::core::Progress<'_>> = Some(&progress);
    read_secrets(c.secrets_stdin, &mut s)?;
    let conn = if needs_connection(c.cmd()) { Some(build_connection(c, &mut s)?) } else { None };
    let conn = || conn.as_ref().ok_or_else(|| failed(Error::new("no connection")));
    let result = |text: &str| emit(json, "result", text, Some(true));
    let root = c.project.as_deref();
    match c.cmd() {
        Cmd::None => {}
        Cmd::Check => {
            let text = match root {
                Some(project) => {
                    let path =
                        sys::path_join(&sys::path_join(project, "deploy"), "manifest-v3.txt");
                    let names = core::load_manifest(&path).map_err(failed)?;
                    format!(
                        "Project capsule passed local validation: {} members in {project}.",
                        names.len()
                    )
                }
                None => CAPSULE.report().map_err(failed)?,
            };
            result(&text);
        }
        Cmd::Prerequisites => {
            if c.install {
                let rows = tools::probe_prerequisites(root, &CAPSULE, cancel);
                let missing = tools::missing_packages(&rows);
                tools::install_packages(&missing, cancel, p).map_err(failed)?;
            }
            // Probed after any installation, so the report describes the
            // machine as it now is.
            let rows = tools::probe_prerequisites(root, &CAPSULE, cancel);
            for row in &rows {
                emit(json, "progress", &prerequisite_line(row), None);
            }
            let blocking = rows.iter().filter(|r| r.blocking()).count();
            if c.check_emulation {
                tools::ensure_arm64_emulation(cancel, p).map_err(failed)?;
            }
            if blocking > 0 {
                let hint = if sys::ON_WINDOWS && !c.install {
                    "; rerun with --install to install the ones winget can supply"
                } else {
                    ""
                };
                return Err(failed(Error::new(format!(
                    "{blocking} required workstation prerequisite(s) are missing{hint}"
                ))));
            }
            result(if root.is_some() {
                "This workstation can build and deploy the appliance."
            } else {
                "This deployer can deploy the appliance it carries."
            });
        }
        Cmd::SetupEmulation => {
            // On Windows there is no `make setup-arm64-emulation`: that
            // target installs a systemd binfmt unit on a Linux host.
            if sys::ON_WINDOWS {
                tools::ensure_arm64_emulation(cancel, p).map_err(failed)?;
                result("This workstation can run ARM64 containers.");
            } else {
                result("Run `make setup-arm64-emulation` with administrator approval.");
            }
        }
        Cmd::PrepareSd => {
            require_secret(c, &mut s, WIFI_PASSWORD, "Wi-Fi password: ")?;
            let sd = SdSettings {
                boot_directory: c.boot_directory.clone().unwrap_or_default(),
                country: c.country.clone().unwrap_or_default(),
                wifi_ssid: c.ssid.clone().unwrap_or_default(),
                wifi_password: s.0[WIFI_PASSWORD].clone(),
            };
            if let Some(problem) = sd_card::validate_sd_settings(&sd) {
                return Err(usage(problem));
            }
            sd_card::prepare_sd_card(&sd, cancel, p).map_err(failed)?;
            result("The Alpine boot partition is ready for headless first boot.");
        }
        Cmd::AlpineSetup => {
            require_secret(c, &mut s, ROOT_PASSWORD, "Root password: ")?;
            require_secret(c, &mut s, PI_PASSWORD, "pi password: ")?;
            if c.ssid.is_some() {
                require_secret(c, &mut s, WIFI_PASSWORD, "Wi-Fi password: ")?;
            }
            let a = AlpineSettings {
                hostname: c.hostname.clone().unwrap_or_default(),
                wifi: c.ssid.as_ref().map(|ssid| WifiSettings {
                    ssid: ssid.clone(),
                    password: s.0[WIFI_PASSWORD].clone(),
                    connect: false,
                    preserve_existing_profiles: true,
                }),
                root_password: s.0[ROOT_PASSWORD].clone(),
                pi_password: s.0[PI_PASSWORD].clone(),
            };
            if let Some(problem) = core::validate_alpine_setup(&a) {
                return Err(usage(problem));
            }
            ops::alpine_setup(conn()?, &a, &CAPSULE, root, cancel, p).map_err(failed)?;
            result("Alpine sys-mode install completed successfully.");
        }
        Cmd::Deploy => {
            let o = DeployOptions {
                project_root: c.project.clone(),
                remote_directory: c.remote_directory.clone().unwrap_or_default(),
                rebuild_image: c.rebuild_image,
            };
            if let Some(problem) = core::validate_options(&o) {
                return Err(usage(problem));
            }
            ops::deploy(conn()?, &o, &CAPSULE, cancel, p).map_err(failed)?;
            result("Deployment completed successfully.");
        }
        Cmd::Wifi => {
            require_secret(c, &mut s, WIFI_PASSWORD, "Wi-Fi password: ")?;
            let w = WifiSettings {
                ssid: c.ssid.clone().unwrap_or_default(),
                password: s.0[WIFI_PASSWORD].clone(),
                connect: !c.no_connect,
                preserve_existing_profiles: !c.replace_existing_profiles,
            };
            if let Some(problem) = core::validate_wifi(&w) {
                return Err(usage(problem));
            }
            ops::apply_wifi(conn()?, &w, cancel, p).map_err(failed)?;
            result("Wi-Fi settings applied.");
        }
        Cmd::WebPassword => {
            if !s.0[WEB_PASSWORD].is_set() && c.interactive_secrets {
                let first = prompt("New Web GUI password: ")?;
                let confirmation = prompt("Confirm Web GUI password: ")?;
                if first != confirmation {
                    return Err(usage("Web GUI password confirmation does not match"));
                }
                s.0[WEB_PASSWORD] = first;
            } else {
                require_secret(c, &mut s, WEB_PASSWORD, "")?;
            }
            if let Some(problem) = core::validate_web_password(&s.0[WEB_PASSWORD]) {
                return Err(usage(problem));
            }
            ops::change_web_password(conn()?, &s.0[WEB_PASSWORD], cancel, p).map_err(failed)?;
            result("Web GUI password changed.");
        }
        Cmd::Hostname => {
            let name = c.name.clone().unwrap_or_default();
            // A usage error rather than a failed operation on the Pi.
            if !core::valid_appliance_hostname(&name) {
                return Err(usage(
                    "--name must be one DNS label of 1-63 characters: letters, digits, and inner \
                     hyphens",
                ));
            }
            ops::set_hostname(conn()?, &name, &CAPSULE, root, cancel, p).map_err(failed)?;
            result(&format!("Appliance hostname changed to {name}."));
        }
        cmd @ (Cmd::Status | Cmd::Logs | Cmd::Restart | Cmd::Reboot) => {
            let action = match cmd {
                Cmd::Status => Action::Status,
                Cmd::Logs => Action::Logs,
                Cmd::Restart => Action::Restart,
                _ => Action::Reboot,
            };
            ops::manage(conn()?, action, cancel, p).map_err(failed)?;
            result("Remote management action succeeded.");
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args_os().map(|a| a.to_string_lossy().into_owned()).collect();
    let c = match parse(&argv) {
        Ok(c) => c,
        Err(code) => return code,
    };
    #[cfg(windows)]
    omt_deploy::sys::init_console();
    match run(&c) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure(code, e)) => {
            if c.json {
                emit(true, "error", e.message(), Some(false));
            } else {
                eprintln!("{e}");
            }
            ExitCode::from(code)
        }
    }
}
