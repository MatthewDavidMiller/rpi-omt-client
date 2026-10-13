//! User authentication (RFC 4252, 4256, and RFC 8332's RSA signature
//! selection). Factory Alpine images accept root with an empty password, and
//! OpenSSH and Dropbear advertise the methods that allow it in different
//! orders, so a password connection tries `none` (for an empty password),
//! then `password`, then `keyboard-interactive`.

use omt_common::buf::{Cursor, wipe_vec};
use omt_common::{Error, Result};

use super::keys::PrivateKey;
use super::msg;
use super::transport::Transport;
use super::wire::{self, Put, namelist_has};
use crate::core::{Auth, Connection, Secret};

const KBD_ROUNDS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Success,
    Failure,
}

fn request_head(b: &mut Vec<u8>, user: &str, method: &str) {
    b.put_u8(msg::USERAUTH_REQUEST);
    b.put_string(user.as_bytes());
    b.put_string(b"ssh-connection");
    b.put_string(method.as_bytes());
}

/// The next reply that is not a banner.
fn next_reply(t: &mut Transport<'_>) -> Result<()> {
    loop {
        t.recv_wait()?;
        if t.msg_type() != msg::USERAUTH_BANNER {
            return Ok(());
        }
    }
}

fn verdict(t: &Transport<'_>) -> Result<Verdict> {
    match t.msg_type() {
        msg::USERAUTH_SUCCESS => Ok(Verdict::Success),
        msg::USERAUTH_FAILURE => Ok(Verdict::Failure),
        other => Err(Error::new(format!("unexpected SSH message {other} during authentication"))),
    }
}

/// Sends a request (wiped afterwards: it may carry a secret) and reads the
/// verdict.
fn send_and_wait(t: &mut Transport<'_>, mut m: Vec<u8>) -> Result<Verdict> {
    let sent = t.send(&m);
    wipe_vec(&mut m);
    sent?;
    next_reply(t)?;
    verdict(t)
}

fn try_none(t: &mut Transport<'_>, user: &str) -> Result<Verdict> {
    let mut m = Vec::new();
    request_head(&mut m, user, "none");
    send_and_wait(t, m)
}

fn try_password(t: &mut Transport<'_>, user: &str, password: &Secret) -> Result<Verdict> {
    let mut m = Vec::new();
    request_head(&mut m, user, "password");
    m.put_bool(false);
    m.put_string(password.text().as_bytes());
    let sent = t.send(&m);
    wipe_vec(&mut m);
    sent?;
    next_reply(t)?;
    // PASSWD_CHANGEREQ: an expired password is a failure here.
    if t.msg_type() == msg::USERAUTH_60 {
        return Ok(Verdict::Failure);
    }
    verdict(t)
}

fn try_keyboard(t: &mut Transport<'_>, user: &str, password: &Secret) -> Result<Verdict> {
    let mut m = Vec::new();
    request_head(&mut m, user, "keyboard-interactive");
    m.put_string(b"");
    m.put_string(b"");
    t.send(&m)?;
    let malformed = || Error::new("malformed keyboard-interactive request");
    for _ in 0..KBD_ROUNDS {
        next_reply(t)?;
        if t.msg_type() != msg::USERAUTH_60 {
            return verdict(t);
        }
        // INFO_REQUEST: answer every prompt with the password.
        let mut r = Cursor::new(t.body());
        for _ in 0..3 {
            wire::string(&mut r).ok_or_else(malformed)?;
        }
        let prompts = wire::u32(&mut r).filter(|&p| p <= 64).ok_or_else(malformed)?;
        let mut reply = vec![msg::USERAUTH_INFO_RESPONSE];
        reply.put_u32(prompts);
        for _ in 0..prompts {
            if wire::string(&mut r).and_then(|_| wire::boolean(&mut r)).is_none() {
                wipe_vec(&mut reply);
                return Err(malformed());
            }
            reply.put_string(password.text().as_bytes());
        }
        let sent = t.send(&reply);
        wipe_vec(&mut reply);
        sent?;
    }
    Ok(Verdict::Failure)
}

/// rsa-sha2-512 unless the server's EXT_INFO lists only rsa-sha2-256.
fn signature_alg<'k>(t: &Transport<'_>, key: &'k PrivateKey) -> &'k str {
    if key.type_name != "ssh-rsa" {
        return &key.type_name;
    }
    let algs = &t.server_sig_algs;
    if !algs.is_empty() && !namelist_has(algs, "rsa-sha2-512") && namelist_has(algs, "rsa-sha2-256")
    {
        return "rsa-sha2-256";
    }
    "rsa-sha2-512"
}

fn try_publickey(t: &mut Transport<'_>, user: &str, key: &PrivateKey) -> Result<Verdict> {
    let alg = signature_alg(t, key);
    let mut signed = Vec::new();
    signed.put_string(&t.session_id);
    request_head(&mut signed, user, "publickey");
    signed.put_bool(true);
    signed.put_string(alg.as_bytes());
    signed.put_string(&key.blob);
    let sig = key.sign(alg, &signed)?;
    // The request is the signed data without its session-id prefix.
    let mut m = signed[4 + t.session_id.len()..].to_vec();
    m.put_string(&sig);
    send_and_wait(t, m)
}

pub fn authenticate(t: &mut Transport<'_>, c: &Connection) -> Result<()> {
    let mut m = vec![msg::SERVICE_REQUEST];
    m.put_string(b"ssh-userauth");
    t.send(&m)?;
    t.recv_wait()?;
    if t.msg_type() != msg::SERVICE_ACCEPT {
        return Err(Error::new("the SSH server refused user authentication"));
    }
    let mut status = Verdict::Failure;
    match c.auth {
        Auth::Password => {
            if !c.password.is_set() {
                return Err(Error::new("SSH password is missing"));
            }
            if c.password.is_empty() {
                status = try_none(t, &c.username)?;
            }
            if status == Verdict::Failure {
                status = try_password(t, &c.username, &c.password)?;
            }
            if status == Verdict::Failure {
                status = try_keyboard(t, &c.username, &c.password)?;
            }
        }
        Auth::Key => {
            let path = c
                .key_path
                .as_deref()
                .ok_or_else(|| Error::new("SSH private-key path is missing"))?;
            let key = PrivateKey::load(path, c.key_passphrase.text().as_bytes())?;
            status = try_publickey(t, &c.username, &key)?;
        }
    }
    match status {
        Verdict::Success => Ok(()),
        Verdict::Failure => Err(Error::new("SSH authentication failed")),
    }
}
