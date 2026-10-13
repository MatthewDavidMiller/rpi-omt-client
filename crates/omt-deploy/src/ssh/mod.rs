//! A strict first-party SSH client: commands and uploads over one
//! authenticated connection. Every primitive comes from OpenSSL's
//! libcrypto; this code only speaks the protocol around them (RFC
//! 4250-4254, 4419, 5656, 8308, 8332, 8709, 8731, the OpenSSH PROTOCOL
//! extensions, and the strict key exchange that closes the Terrapin
//! prefix-truncation attack).
//!
//! Unknown or changed host keys are fatal, and known_hosts must exist.
//! Algorithms exclude SHA-1 signatures and MACs, CBC, and compression;
//! strict key exchange is mandatory. Uploads use SFTP when the server
//! offers it and fall back to `cat` on an exec channel for factory headless
//! images.

pub mod auth;
pub mod bcrypt;
#[rustfmt::skip] // generated; tools/gen/gen_blowfish_tables.py --check holds it
mod blowfish_tables;
pub mod channel;
pub mod cipher;
pub mod kex;
pub mod keys;
pub mod known_hosts;
pub mod sftp;
pub mod transport;
pub mod wire;

use omt_common::{Error, Result};

use crate::core::Connection;
use crate::sys::{self, Cancel};
use channel::Source;
use transport::Transport;

/// The largest packet either side may send: OpenSSH's own ceiling.
pub const MAX_PACKET: usize = 256 * 1024;

/// Message numbers.
pub mod msg {
    pub const DISCONNECT: u8 = 1;
    pub const IGNORE: u8 = 2;
    pub const UNIMPLEMENTED: u8 = 3;
    pub const DEBUG: u8 = 4;
    pub const SERVICE_REQUEST: u8 = 5;
    pub const SERVICE_ACCEPT: u8 = 6;
    pub const EXT_INFO: u8 = 7;
    pub const KEXINIT: u8 = 20;
    pub const NEWKEYS: u8 = 21;
    /// KEXDH_INIT, KEX_ECDH_INIT, KEX_HYBRID_INIT
    pub const KEX_30: u8 = 30;
    /// KEXDH_REPLY, KEX_ECDH_REPLY, KEX_DH_GEX_GROUP
    pub const KEX_31: u8 = 31;
    pub const KEX_DH_GEX_INIT: u8 = 32;
    pub const KEX_DH_GEX_REPLY: u8 = 33;
    pub const KEX_DH_GEX_REQUEST: u8 = 34;
    pub const USERAUTH_REQUEST: u8 = 50;
    pub const USERAUTH_FAILURE: u8 = 51;
    pub const USERAUTH_SUCCESS: u8 = 52;
    pub const USERAUTH_BANNER: u8 = 53;
    /// PK_OK, PASSWD_CHANGEREQ, INFO_REQUEST
    pub const USERAUTH_60: u8 = 60;
    pub const USERAUTH_INFO_RESPONSE: u8 = 61;
    pub const GLOBAL_REQUEST: u8 = 80;
    pub const REQUEST_SUCCESS: u8 = 81;
    pub const REQUEST_FAILURE: u8 = 82;
    pub const CHANNEL_OPEN: u8 = 90;
    pub const CHANNEL_OPEN_CONFIRMATION: u8 = 91;
    pub const CHANNEL_OPEN_FAILURE: u8 = 92;
    pub const CHANNEL_WINDOW_ADJUST: u8 = 93;
    pub const CHANNEL_DATA: u8 = 94;
    pub const CHANNEL_EXTENDED_DATA: u8 = 95;
    pub const CHANNEL_EOF: u8 = 96;
    pub const CHANNEL_CLOSE: u8 = 97;
    pub const CHANNEL_REQUEST: u8 = 98;
    pub const CHANNEL_SUCCESS: u8 = 99;
    pub const CHANNEL_FAILURE: u8 = 100;
}

#[derive(Clone, Debug, Default)]
pub struct SshResult {
    pub exit_code: i32,
    /// Lossily decoded UTF-8.
    pub out: String,
    pub err: String,
}

impl SshResult {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }

    /// stdout, a newline if needed, then stderr.
    pub fn combined(&self) -> String {
        let mut out = self.out.clone();
        if !self.err.is_empty() {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&self.err);
        }
        out
    }
}

/// known_hosts as used for a connection: the explicit path, or
/// ~/.ssh/known_hosts from HOME or USERPROFILE.
pub fn known_hosts_path(c: &Connection) -> Result<String> {
    if let Some(p) = &c.known_hosts_path {
        return Ok(p.clone());
    }
    let home = sys::getenv("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| sys::getenv("USERPROFILE").filter(|h| !h.is_empty()))
        .ok_or_else(|| {
            Error::new("home directory is unavailable for strict host-key verification")
        })?;
    Ok(sys::path_join(&sys::path_join(&home, ".ssh"), "known_hosts"))
}

/// One authenticated connection.
#[derive(Debug)]
pub struct Session<'c> {
    pub(crate) t: Transport<'c>,
    pub(crate) next_channel: u32,
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        self.t.disconnect(11, "disconnected by user");
        self.t.close();
    }
}

impl<'c> Session<'c> {
    pub fn connect(c: &Connection, cancel: Option<&'c Cancel>) -> Result<Self> {
        // known_hosts has to exist before anything is sent: a missing file is
        // the operator's to fix, and its message should not read as a
        // network one.
        let path = known_hosts_path(c)?;
        if !sys::is_file(&path) {
            return Err(Error::new(known_hosts::missing_message(&path)));
        }
        let mut s = Self { t: Transport::new(cancel), next_channel: 0 };
        if let Err(e) = s.t.open(&c.host, c.port, &path) {
            let text = e.message();
            if text == "SSH connection timed out"
                || text == "operation cancelled"
                || text.starts_with("OpenSSH known_hosts was not found")
            {
                return Err(e);
            }
            return Err(Error::new(format!(
                "SSH connection or host-key verification failed: {text}. Unknown or changed host \
                 keys are rejected."
            )));
        }
        auth::authenticate(&mut s.t, c)?;
        // Commands are cancelled by their own flag from here on.
        s.t.cancel = None;
        Ok(s)
    }

    pub fn run(
        &mut self,
        command: &str,
        input: &[u8],
        cancel: Option<&Cancel>,
    ) -> Result<SshResult> {
        channel::run(self, command, input, cancel)
    }

    pub fn run_pty_after_marker(
        &mut self,
        command: &str,
        marker: &str,
        input: &[u8],
        cancel: Option<&Cancel>,
    ) -> Result<SshResult> {
        channel::run_pty_after_marker(self, command, marker, input, cancel)
    }

    fn upload(
        &mut self,
        src: &mut Source<'_>,
        remote: &str,
        cancel: Option<&Cancel>,
    ) -> Result<()> {
        let Err(sftp_error) = sftp::upload(self, src, remote, cancel) else { return Ok(()) };
        let fallback = src.rewind().and_then(|()| channel::shell_upload(self, src, remote, cancel));
        fallback.map_err(|shell_error| {
            Error::new(format!(
                "SFTP upload failed ({sftp_error}); shell fallback failed ({shell_error})"
            ))
        })
    }

    pub fn upload_file(
        &mut self,
        local: &str,
        remote: &str,
        cancel: Option<&Cancel>,
    ) -> Result<()> {
        let mut src = Source::File(sys::open_read(local)?);
        self.upload(&mut src, remote, cancel)
    }

    pub fn upload_bytes(
        &mut self,
        data: &[u8],
        remote: &str,
        cancel: Option<&Cancel>,
    ) -> Result<()> {
        let mut src = Source::Memory { data, pos: 0 };
        self.upload(&mut src, remote, cancel)
    }
}
