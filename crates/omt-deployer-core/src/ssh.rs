//! Strict SSH/SFTP adapter backed by russh.
//!
//! Unknown or changed host keys are fatal. Preferred algorithms exclude legacy
//! SHA-1 host-key hashes and CBC ciphers (russh's default cipher list is
//! already CTR/GCM/ChaCha20-only).

use crate::{AuthMethod, Connection, OUTPUT_LIMIT};
use russh::keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKey, load_secret_key};
use russh::{ChannelMsg, Preferred, client, kex};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use std::borrow::Cow;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::time::{self, timeout};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const COMMAND_IDLE_TIMEOUT: Duration = Duration::from_mins(1);
const UPLOAD_TIMEOUT: Duration = Duration::from_mins(30);

#[derive(Debug)]
pub struct RemoteResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl RemoteResult {
    pub fn is_success(&self) -> bool {
        self.exit_code == 0
    }

    pub fn combined(&self) -> String {
        let mut out = self.stdout.clone();
        if !self.stderr.is_empty() {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&self.stderr);
        }
        out
    }
}

struct StrictHostKey {
    host: String,
    port: u16,
}

impl client::Handler for StrictHostKey {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        match keys::check_known_hosts(&self.host, self.port, server_public_key) {
            Ok(true) => Ok(true),
            Ok(false) | Err(keys::Error::KeyChanged { .. } | keys::Error::NoHomeDir) => Ok(false),
            Err(error) => Err(russh::Error::from(error)),
        }
    }
}

const SECURE_HOST_KEY_ALGS: &[russh::keys::Algorithm] = &[
    ssh_key_alg::ed25519(),
    ssh_key_alg::ecdsa_p256(),
    ssh_key_alg::ecdsa_p384(),
    ssh_key_alg::ecdsa_p521(),
    ssh_key_alg::rsa_sha512(),
    ssh_key_alg::rsa_sha256(),
];

const SECURE_KEX_ORDER: &[kex::Name] = &[
    kex::MLKEM768X25519_SHA256,
    kex::CURVE25519,
    kex::CURVE25519_PRE_RFC_8731,
    kex::DH_GEX_SHA256,
    kex::DH_G18_SHA512,
    kex::DH_G17_SHA512,
    kex::DH_G16_SHA512,
    kex::DH_G15_SHA512,
    kex::DH_G14_SHA256,
    kex::EXTENSION_SUPPORT_AS_CLIENT,
    kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT,
];

fn secure_preferred() -> Preferred {
    Preferred {
        kex: Cow::Borrowed(SECURE_KEX_ORDER),
        key: Cow::Borrowed(SECURE_HOST_KEY_ALGS),
        ..Preferred::DEFAULT
    }
}

mod ssh_key_alg {
    use russh::keys::{Algorithm, EcdsaCurve, HashAlg};
    pub const fn ed25519() -> Algorithm {
        Algorithm::Ed25519
    }
    pub const fn ecdsa_p256() -> Algorithm {
        Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP256,
        }
    }
    pub const fn ecdsa_p384() -> Algorithm {
        Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP384,
        }
    }
    pub const fn ecdsa_p521() -> Algorithm {
        Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP521,
        }
    }
    pub const fn rsa_sha512() -> Algorithm {
        Algorithm::Rsa {
            hash: Some(HashAlg::Sha512),
        }
    }
    pub const fn rsa_sha256() -> Algorithm {
        Algorithm::Rsa {
            hash: Some(HashAlg::Sha256),
        }
    }
}

fn known_hosts_path() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "home directory is unavailable for strict host-key verification",
            )
        })?;
    Ok(PathBuf::from(home).join(".ssh").join("known_hosts"))
}

fn ensure_known_hosts_present() -> io::Result<()> {
    let path = known_hosts_path()?;
    if !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "OpenSSH known_hosts was not found at {}. Connect with ssh first and verify the Raspberry Pi host key.",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn map_err(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

pub struct SshSession {
    runtime: tokio::runtime::Runtime,
    handle: client::Handle<StrictHostKey>,
}

impl SshSession {
    pub fn connect(connection: &Connection) -> io::Result<Self> {
        ensure_known_hosts_present()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let handle = runtime.block_on(connect_async(connection))?;
        Ok(Self { runtime, handle })
    }

    pub fn run(
        &mut self,
        command: &str,
        stdin: &str,
        cancellation: &AtomicBool,
    ) -> io::Result<RemoteResult> {
        self.runtime
            .block_on(run_command(&self.handle, command, stdin, cancellation))
    }

    pub fn upload(
        &mut self,
        local: &Path,
        remote: &str,
        cancellation: &AtomicBool,
    ) -> io::Result<()> {
        self.runtime
            .block_on(upload_file(&self.handle, local, remote, cancellation))
    }
}

async fn connect_async(connection: &Connection) -> io::Result<client::Handle<StrictHostKey>> {
    let config = client::Config {
        inactivity_timeout: Some(COMMAND_IDLE_TIMEOUT),
        preferred: secure_preferred(),
        ..client::Config::default()
    };
    let handler = StrictHostKey {
        host: connection.host.clone(),
        port: connection.port,
    };
    let mut handle = timeout(
        CONNECT_TIMEOUT,
        client::connect(
            Arc::new(config),
            (connection.host.as_str(), connection.port),
            handler,
        ),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "SSH connection timed out"))?
    .map_err(|error| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "SSH connection or host-key verification failed: {error}. Unknown or changed host keys are rejected."
            ),
        )
    })?;

    let authenticated = match connection.auth {
        AuthMethod::Password => {
            let password = connection
                .password
                .as_ref()
                .ok_or_else(|| io::Error::other("SSH password is missing"))?;
            handle
                .authenticate_password(connection.username.as_str(), password.expose())
                .await
                .map_err(map_err)?
                .success()
        }
        AuthMethod::Key => {
            let key_path = connection
                .key_path
                .as_ref()
                .ok_or_else(|| io::Error::other("SSH private-key path is missing"))?;
            let passphrase = connection
                .key_passphrase
                .as_ref()
                .map(crate::Secret::expose);
            let key = load_secret_key(key_path, passphrase).map_err(map_err)?;
            let hash = handle
                .best_supported_rsa_hash()
                .await
                .map_err(map_err)?
                .flatten()
                .or(Some(HashAlg::Sha512));
            handle
                .authenticate_publickey(
                    connection.username.as_str(),
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await
                .map_err(map_err)?
                .success()
        }
    };
    if !authenticated {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "SSH authentication failed",
        ));
    }
    Ok(handle)
}

async fn run_command(
    handle: &client::Handle<StrictHostKey>,
    command: &str,
    stdin: &str,
    cancellation: &AtomicBool,
) -> io::Result<RemoteResult> {
    if cancellation.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "operation cancelled",
        ));
    }
    let mut channel = handle.channel_open_session().await.map_err(map_err)?;
    channel.exec(true, command).await.map_err(map_err)?;
    if !stdin.is_empty() {
        channel.data(stdin.as_bytes()).await.map_err(map_err)?;
    }
    channel.eof().await.map_err(map_err)?;

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_code = 1_i32;
    let mut idle = time::interval(Duration::from_millis(100));
    idle.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    let deadline = time::Instant::now() + COMMAND_IDLE_TIMEOUT;
    let mut last_activity = time::Instant::now();

    loop {
        if cancellation.load(Ordering::Relaxed) {
            let _ = channel.close().await;
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "operation cancelled",
            ));
        }
        tokio::select! {
            message = channel.wait() => {
                match message {
                    Some(ChannelMsg::Data { ref data }) => {
                        append_bounded(&mut stdout, data)?;
                        last_activity = time::Instant::now();
                    }
                    Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                        append_bounded(&mut stderr, data)?;
                        last_activity = time::Instant::now();
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        exit_code = i32::try_from(exit_status).unwrap_or(1);
                        last_activity = time::Instant::now();
                    }
                    Some(ChannelMsg::Eof) | None => break,
                    Some(_) => {
                        last_activity = time::Instant::now();
                    }
                }
            }
            _ = idle.tick() => {
                if last_activity.elapsed() > COMMAND_IDLE_TIMEOUT
                    || time::Instant::now() > deadline + COMMAND_IDLE_TIMEOUT
                {
                    let _ = channel.close().await;
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "remote command produced no output for 60 seconds",
                    ));
                }
            }
        }
    }

    Ok(RemoteResult {
        exit_code,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

fn append_bounded(target: &mut Vec<u8>, data: &[u8]) -> io::Result<()> {
    let remaining = OUTPUT_LIMIT.saturating_sub(target.len());
    if data.len() > remaining {
        return Err(io::Error::other("remote command output exceeded 4 MiB"));
    }
    target.extend_from_slice(data);
    Ok(())
}

async fn upload_file(
    handle: &client::Handle<StrictHostKey>,
    local: &Path,
    remote: &str,
    cancellation: &AtomicBool,
) -> io::Result<()> {
    if cancellation.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "operation cancelled",
        ));
    }
    let channel = handle.channel_open_session().await.map_err(map_err)?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(map_err)?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(map_err)?;
    let mut file = File::open(local)?;
    let mut remote_file = timeout(
        UPLOAD_TIMEOUT,
        sftp.open_with_flags(
            remote,
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        ),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "SFTP open timed out"))?
    .map_err(map_err)?;

    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        if cancellation.load(Ordering::Relaxed) {
            let _ = remote_file.shutdown().await;
            let _ = sftp.remove_file(remote).await;
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "operation cancelled",
            ));
        }
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        timeout(UPLOAD_TIMEOUT, remote_file.write_all(&buffer[..count]))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "SFTP upload timed out"))?
            .map_err(map_err)?;
    }
    remote_file.flush().await.map_err(map_err)?;
    remote_file.shutdown().await.map_err(map_err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_excludes_legacy_rsa_sha1() {
        let preferred = secure_preferred();
        assert!(
            preferred.key.iter().all(|algorithm| {
                !matches!(algorithm, russh::keys::Algorithm::Rsa { hash: None })
            })
        );
    }
}
