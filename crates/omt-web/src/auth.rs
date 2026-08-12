use crate::{
    io::{atomic_replace, random_hex, read_text},
    json,
    settings::Settings,
};
use hmac::{Hmac, Mac};
use pbkdf2::pbkdf2_hmac;
use scrypt::{Params as ScryptParams, scrypt};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fmt::Write as _;
use std::{
    collections::BTreeMap,
    env, fs,
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;

const SECRET_LIMIT: usize = 256;
const PASSWORD_LIMIT: usize = 16 * 1024;
const REGISTRY_LIMIT: usize = 64 * 1024;
const MAXIMUM_SESSIONS: usize = 64;
const PBKDF2_ITERATIONS: u32 = 600_000;
pub const MINIMUM_PASSWORD_BYTES: usize = 12;
pub const MAXIMUM_PASSWORD_BYTES: usize = 128;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
enum Password {
    Plain(String),
    Pbkdf2 {
        iterations: u32,
        salt: Vec<u8>,
        digest: Vec<u8>,
    },
    Scrypt {
        n: u32,
        r: u32,
        p: u32,
        salt: Vec<u8>,
        digest: Vec<u8>,
    },
}

fn hex_decode(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid hexadecimal value".to_owned());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).map_err(|error| error.to_string())?;
            u8::from_str_radix(text, 16).map_err(|error| error.to_string())
        })
        .collect()
}

fn hex_encode(value: &[u8]) -> String {
    value.iter().fold(
        String::with_capacity(value.len() * 2),
        |mut output, byte| {
            let _ignored = write!(output, "{byte:02x}");
            output
        },
    )
}

fn parse_password(value: String) -> Result<Password, String> {
    if let Some(rest) = value.strip_prefix("pbkdf2:sha256:") {
        let (parameters, body) = rest.split_once('$').ok_or("invalid PBKDF2 password hash")?;
        let (salt, digest) = body.split_once('$').ok_or("invalid PBKDF2 password hash")?;
        let iterations = parameters
            .parse::<u32>()
            .ok()
            .filter(|number| *number > 0)
            .ok_or("invalid PBKDF2 iterations")?;
        return Ok(Password::Pbkdf2 {
            iterations,
            salt: salt.as_bytes().to_vec(),
            digest: hex_decode(digest)?,
        });
    }
    if let Some(rest) = value.strip_prefix("scrypt:") {
        let (parameters, body) = rest.split_once('$').ok_or("invalid scrypt password hash")?;
        let mut values = parameters.split(':').map(str::parse::<u32>);
        let n = values
            .next()
            .transpose()
            .map_err(|_| "invalid scrypt N")?
            .ok_or("missing scrypt N")?;
        let r = values
            .next()
            .transpose()
            .map_err(|_| "invalid scrypt r")?
            .ok_or("missing scrypt r")?;
        let p = values
            .next()
            .transpose()
            .map_err(|_| "invalid scrypt p")?
            .ok_or("missing scrypt p")?;
        if values.next().is_some() || !n.is_power_of_two() || n < 2 {
            return Err("invalid scrypt parameters".to_owned());
        }
        let (salt, digest) = body.split_once('$').ok_or("invalid scrypt password hash")?;
        let log_n = u8::try_from(n.ilog2()).map_err(|_| "invalid scrypt N")?;
        ScryptParams::new(log_n, r, p, digest.len() / 2)
            .map_err(|error| format!("unsupported scrypt parameters: {error}"))?;
        return Ok(Password::Scrypt {
            n,
            r,
            p,
            salt: salt.as_bytes().to_vec(),
            digest: hex_decode(digest)?,
        });
    }
    if value.starts_with("argon2:") || value.starts_with("pbkdf2:") {
        return Err("The Web GUI password hash uses an unsupported format".to_owned());
    }
    Ok(Password::Plain(value))
}

impl Password {
    fn verify(&self, supplied: &str) -> bool {
        let expected = match self {
            Self::Plain(value) => {
                return constant_time_equal(value.as_bytes(), supplied.as_bytes());
            }
            Self::Pbkdf2 {
                iterations,
                salt,
                digest,
            } => {
                let mut actual = vec![0_u8; digest.len()];
                pbkdf2_hmac::<Sha256>(supplied.as_bytes(), salt, *iterations, &mut actual);
                (actual, digest)
            }
            Self::Scrypt {
                n,
                r,
                p,
                salt,
                digest,
            } => {
                let Ok(log_n) = u8::try_from(n.ilog2()) else {
                    return false;
                };
                let Ok(parameters) = ScryptParams::new(log_n, *r, *p, digest.len()) else {
                    return false;
                };
                let mut actual = vec![0_u8; digest.len()];
                if scrypt(supplied.as_bytes(), salt, &parameters, &mut actual).is_err() {
                    return false;
                }
                (actual, digest)
            }
        };
        constant_time_equal(&expected.0, expected.1)
    }
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

#[derive(Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
struct Registry {
    version: u8,
    sessions: BTreeMap<String, SessionRecord>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionRecord {
    expires_at: u64,
    password_digest: String,
}

pub struct Authentication {
    settings: Settings,
    secret: Vec<u8>,
    password: Password,
    password_digest: String,
    registry: Mutex<()>,
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_secs())
}

impl Authentication {
    pub fn load(settings: &Settings) -> Result<Self, String> {
        let secret_path = settings.config_dir.join("web_secret");
        let secret = read_text(&secret_path, SECRET_LIMIT)?
            .filter(|value| !value.trim().is_empty())
            .ok_or("The Web secret is missing or unsafe")?;
        let password_text = match env::var("OMT_WEB_PASSWORD") {
            Ok(value) if !value.is_empty() => value,
            _ => read_text(&settings.password_file, PASSWORD_LIMIT)?
                .filter(|value| !value.trim().is_empty())
                .ok_or("The Web GUI password file is missing or unsafe")?
                .trim()
                .to_owned(),
        };
        let secret_bytes = secret.trim().as_bytes().to_vec();
        let password_digest = keyed_hex(&secret_bytes, password_text.as_bytes())?;
        Ok(Self {
            settings: settings.clone(),
            secret: secret_bytes,
            password: parse_password(password_text)?,
            password_digest,
            registry: Mutex::new(()),
        })
    }

    fn registry_path(&self) -> std::path::PathBuf {
        self.settings.config_dir.join("web_sessions.json")
    }

    fn read_registry(&self) -> Registry {
        let Ok(Some(text)) = read_text(&self.registry_path(), REGISTRY_LIMIT) else {
            return Registry {
                version: 2,
                sessions: BTreeMap::new(),
            };
        };
        let Ok(registry) = json::from_str::<Registry>(&text) else {
            return Registry {
                version: 2,
                sessions: BTreeMap::new(),
            };
        };
        if registry.version == 2 {
            registry
        } else {
            Registry {
                version: 2,
                sessions: BTreeMap::new(),
            }
        }
    }

    fn write_registry(&self, registry: &Registry) -> Result<(), String> {
        let mut data = serde_json::to_vec(registry).map_err(|error| error.to_string())?;
        data.push(b'\n');
        atomic_replace(&self.registry_path(), &data, REGISTRY_LIMIT)
    }

    fn session_digest(&self, session_id: &str) -> Result<String, String> {
        keyed_hex(&self.secret, session_id.as_bytes())
    }

    pub fn authenticate(
        &self,
        password: &str,
        previous: Option<&str>,
    ) -> Result<Option<String>, String> {
        if !self.password.verify(password) {
            return Ok(None);
        }
        let session_id = random_hex(32)?;
        let now = now_epoch();
        let _guard = self
            .registry
            .lock()
            .map_err(|_| "session registry lock failed")?;
        let mut registry = self.read_registry();
        registry.sessions.retain(|_, record| {
            record.expires_at > now && record.password_digest == self.password_digest
        });
        if let Some(previous_id) = previous {
            registry.sessions.remove(&self.session_digest(previous_id)?);
        }
        registry.sessions.insert(
            self.session_digest(&session_id)?,
            SessionRecord {
                expires_at: now.saturating_add(self.settings.session_lifetime.as_secs()),
                password_digest: self.password_digest.clone(),
            },
        );
        while registry.sessions.len() > MAXIMUM_SESSIONS {
            let Some(oldest) = registry
                .sessions
                .iter()
                .min_by_key(|(key, record)| (record.expires_at, *key))
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            registry.sessions.remove(&oldest);
        }
        self.write_registry(&registry)?;
        Ok(Some(session_id))
    }

    pub fn is_current(&self, session_id: &str) -> bool {
        let Ok(_guard) = self.registry.lock() else {
            return false;
        };
        let Ok(digest) = self.session_digest(session_id) else {
            return false;
        };
        self.read_registry()
            .sessions
            .get(&digest)
            .is_some_and(|record| {
                record.expires_at > now_epoch() && record.password_digest == self.password_digest
            })
    }

    pub fn revoke(&self, session_id: &str) -> Result<(), String> {
        let _guard = self
            .registry
            .lock()
            .map_err(|_| "session registry lock failed")?;
        let mut registry = self.read_registry();
        if registry
            .sessions
            .remove(&self.session_digest(session_id)?)
            .is_some()
        {
            self.write_registry(&registry)?;
        }
        Ok(())
    }

    pub fn csrf_token(&self, scope: &str, nonce: &str) -> Result<String, String> {
        keyed_hex(&self.secret, format!("csrf\0{scope}\0{nonce}").as_bytes())
    }

    pub fn verify_csrf(&self, scope: &str, nonce: &str, token: &str) -> bool {
        self.csrf_token(scope, nonce)
            .is_ok_and(|expected| constant_time_equal(expected.as_bytes(), token.as_bytes()))
    }
}

fn keyed_hex(key: &[u8], value: &[u8]) -> Result<String, String> {
    let mut mac = HmacSha256::new_from_slice(key).map_err(|error| error.to_string())?;
    mac.update(value);
    Ok(hex_encode(&mac.finalize().into_bytes()))
}

pub fn validate_new_password(password: &str) -> Result<(), String> {
    if !(MINIMUM_PASSWORD_BYTES..=MAXIMUM_PASSWORD_BYTES).contains(&password.len()) {
        return Err(format!(
            "The Web GUI password must contain {MINIMUM_PASSWORD_BYTES}-{MAXIMUM_PASSWORD_BYTES} UTF-8 bytes"
        ));
    }
    if password.chars().any(char::is_control) {
        return Err("The Web GUI password must not contain control characters".to_owned());
    }
    Ok(())
}

fn encoded_password(password: &str) -> Result<String, String> {
    let salt = random_hex(16)?;
    let mut digest = [0_u8; 32];
    pbkdf2_hmac::<Sha256>(
        password.as_bytes(),
        salt.as_bytes(),
        PBKDF2_ITERATIONS,
        &mut digest,
    );
    Ok(format!(
        "pbkdf2:sha256:{PBKDF2_ITERATIONS}${salt}${}\n",
        hex_encode(&digest)
    ))
}

pub fn replace_password(settings: &Settings, password: &str) -> Result<(), String> {
    if env::var("OMT_WEB_PASSWORD").is_ok_and(|value| !value.is_empty()) {
        return Err(
            "OMT_WEB_PASSWORD overrides the password file; remove the emergency override first"
                .to_owned(),
        );
    }
    validate_new_password(password)?;
    let encoded = encoded_password(password)?;
    atomic_replace(&settings.password_file, encoded.as_bytes(), PASSWORD_LIMIT)
}

pub fn initialize(settings: &Settings) -> Result<Option<String>, String> {
    fs::create_dir_all(&settings.config_dir).map_err(|error| error.to_string())?;
    let secret_path = settings.config_dir.join("web_secret");
    if read_text(&secret_path, SECRET_LIMIT)?.is_none_or(|value| value.trim().is_empty()) {
        let legacy_path = settings.config_dir.join("flask_secret");
        let secret = read_text(&legacy_path, SECRET_LIMIT)?
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(random_hex(32)?);
        atomic_replace(
            &secret_path,
            format!("{}\n", secret.trim()).as_bytes(),
            SECRET_LIMIT,
        )?;
    }
    if read_text(&settings.password_file, PASSWORD_LIMIT)?
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(None);
    }
    let password = random_hex(16)?;
    let encoded = encoded_password(&password)?;
    atomic_replace(&settings.password_file, encoded.as_bytes(), PASSWORD_LIMIT)?;
    Ok(Some(password))
}

pub fn remove_legacy_secret(settings: &Settings) {
    let legacy = settings.config_dir.join("flask_secret");
    if Path::new(&legacy).is_file() {
        let _ignored = fs::remove_file(legacy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_werkzeug_hashes() {
        let hash = parse_password(
            "pbkdf2:sha256:1$salt$120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
                .to_owned(),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert!(hash.verify("password"));
        assert!(!hash.verify("no"));
        let scrypt_hash = parse_password("scrypt:32768:8:1$07kZLpT9$d12f4706055d4d0812a754b965a9150e8c843b0ce3672d8db291df10e0bf144bc268bb049c9c3f209ea4614d5309a759eba4e123a4bd12e08daa002f95ccfe97".to_owned()).unwrap_or_else(|error| panic!("{error}"));
        assert!(scrypt_hash.verify("password"));
        assert!(!scrypt_hash.verify("not-password"));
    }

    #[test]
    fn new_password_policy_and_encoding_are_bounded() {
        assert!(validate_new_password("correct horse battery staple").is_ok());
        assert!(validate_new_password("too-short").is_err());
        assert!(validate_new_password(&"x".repeat(MAXIMUM_PASSWORD_BYTES + 1)).is_err());
        assert!(validate_new_password("twelve bytes\n").is_err());

        let encoded = encoded_password("correct horse battery staple")
            .unwrap_or_else(|error| panic!("{error}"));
        let parsed =
            parse_password(encoded.trim().to_owned()).unwrap_or_else(|error| panic!("{error}"));
        assert!(parsed.verify("correct horse battery staple"));
        assert!(!parsed.verify("wrong password"));
    }
}
