use getrandom::fill;
use std::fmt::Write as _;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub fn read_bounded(path: &Path, maximum: usize) -> Result<Option<Vec<u8>>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("path is not a safe regular file".to_owned());
    }
    if metadata.len() > u64::try_from(maximum).unwrap_or(u64::MAX) {
        return Err("file exceeds its size limit".to_owned());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| error.to_string())?;
    let opened = file.metadata().map_err(|error| error.to_string())?;
    if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() || !opened.is_file() {
        return Err("file changed while opening".to_owned());
    }
    let mut data = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(u64::try_from(maximum.saturating_add(1)).unwrap_or(u64::MAX))
        .read_to_end(&mut data)
        .map_err(|error| error.to_string())?;
    if data.len() > maximum {
        return Err("file exceeds its size limit".to_owned());
    }
    Ok(Some(data))
}

pub fn read_text(path: &Path, maximum: usize) -> Result<Option<String>, String> {
    read_bounded(path, maximum)?
        .map(|data| String::from_utf8(data).map_err(|_| "file is not valid UTF-8".to_owned()))
        .transpose()
}

fn temporary_path(path: &Path) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("invalid target path")?;
    let mut nonce = [0_u8; 12];
    fill(&mut nonce).map_err(|error| error.to_string())?;
    let suffix = hex(&nonce);
    Ok(path.with_file_name(format!(".{name}.{suffix}.tmp")))
}

pub fn atomic_replace(path: &Path, value: &[u8], maximum: usize) -> Result<(), String> {
    if value.len() > maximum {
        return Err("replacement exceeds its size limit".to_owned());
    }
    let parent = path.parent().ok_or("target has no parent directory")?;
    let parent_meta = fs::symlink_metadata(parent).map_err(|error| error.to_string())?;
    if parent_meta.file_type().is_symlink() || !parent_meta.is_dir() {
        return Err("target directory is unsafe".to_owned());
    }
    if let Ok(metadata) = fs::symlink_metadata(path)
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err("target path is unsafe".to_owned());
    }
    let staged = temporary_path(path)?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&staged)
            .map_err(|error| error.to_string())?;
        file.write_all(value).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&staged, path).map_err(|error| error.to_string())?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ignored = fs::remove_file(staged);
    }
    result
}

pub fn remove_file_durable(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err("target path is unsafe".to_owned());
        }
        Ok(_) => fs::remove_file(path).map_err(|error| error.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    }
    let parent = path.parent().ok_or("target has no parent")?;
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|error| error.to_string())
}

pub fn write_fixed_inode(path: &Path, value: &[u8], maximum: usize) -> Result<(), String> {
    if value.len() > maximum {
        return Err("request exceeds its size limit".to_owned());
    }
    let before = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if before.file_type().is_symlink() || !before.is_file() {
        return Err("request file is unsafe".to_owned());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| error.to_string())?;
    let opened = file.metadata().map_err(|error| error.to_string())?;
    if opened.dev() != before.dev() || opened.ino() != before.ino() || !opened.is_file() {
        return Err("request file changed while opening".to_owned());
    }
    file.set_len(0).map_err(|error| error.to_string())?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    file.write_all(value).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    let after = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if after.file_type().is_symlink() || after.dev() != before.dev() || after.ino() != before.ino()
    {
        return Err("request file changed during write".to_owned());
    }
    Ok(())
}

pub fn random_hex(bytes: usize) -> Result<String, String> {
    let mut data = vec![0_u8; bytes];
    fill(&mut data).map_err(|error| error.to_string())?;
    Ok(hex(&data))
}

fn hex(data: &[u8]) -> String {
    data.iter()
        .fold(String::with_capacity(data.len() * 2), |mut output, byte| {
            let _ignored = write!(output, "{byte:02x}");
            output
        })
}
