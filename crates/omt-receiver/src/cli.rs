// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// The receiver's CLI is a trust boundary: control-omt.sh and the Rust Web
// diagnostics service both build argument vectors for it, so every rejection
// has to be a named usage failure that leaves the command unrun.

use std::collections::BTreeMap;

/// More options than any subcommand accepts, which bounds the parse.
const MAX_OPTIONS: usize = 8;

#[derive(Default)]
pub struct Options {
    values: BTreeMap<String, Option<String>>,
}

impl Options {
    /// Parses `--key value` pairs and the single `--json` flag.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut result = Self::default();
        let mut index = 0;
        while index < args.len() {
            let key = &args[index];
            if !key.starts_with("--") {
                return Err(format!("Unexpected argument: {key}"));
            }
            let flag = key == "--json";
            let value = if flag {
                None
            } else {
                index += 1;
                Some(
                    args.get(index)
                        .ok_or_else(|| format!("Missing value for {key}"))?
                        .clone(),
                )
            };
            if result.values.insert(key.clone(), value).is_some() {
                return Err(format!("Duplicate option: {key}"));
            }
            if result.values.len() > MAX_OPTIONS {
                return Err("Too many options.".into());
            }
            index += 1;
        }
        Ok(result)
    }

    /// Rejects any option the subcommand does not define.
    pub fn allowed(&self, names: &[&str]) -> Result<(), String> {
        self.values
            .keys()
            .find(|key| !names.contains(&key.as_str()))
            .map_or(Ok(()), |key| {
                Err(format!("Option {key} is not valid for this command."))
            })
    }

    pub fn required(&self, name: &str) -> Result<&str, String> {
        self.values
            .get(name)
            .and_then(Option::as_deref)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{name} is required."))
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).and_then(Option::as_deref)
    }

    /// Requires a valueless flag such as `--json`.
    pub fn flag(&self, name: &str) -> Result<(), String> {
        if self.values.get(name).is_some_and(Option::is_none) {
            Ok(())
        } else {
            Err(format!("{name} is required."))
        }
    }

    pub fn number(&self, name: &str, default: u64, min: u64, max: u64) -> Result<u64, String> {
        let Some(value) = self.values.get(name) else {
            return Ok(default);
        };
        let text = value
            .as_deref()
            .ok_or_else(|| format!("{name} requires a value."))?;
        let parsed = text
            .parse::<u64>()
            .map_err(|_| format!("{name} must be between {min} and {max}."))?;
        if !(min..=max).contains(&parsed) {
            return Err(format!("{name} must be between {min} and {max}."));
        }
        Ok(parsed)
    }
}

pub fn usage() -> i32 {
    eprintln!(
        "Usage: omt-receiver --version | discover --wait-ms N --json | probe --target TARGET --timeout-ms N --json | play --target TARGET --connector auto|HDMI-A-1|HDMI-A-2 --status-file PATH --video-ceiling WIDTHxHEIGHT@FPS[,...]"
    );
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn rejects_malformed_invocations() {
        assert!(Options::parse(&args(&["extra"])).is_err());
        assert!(Options::parse(&args(&["--json", "--json"])).is_err());
        assert!(Options::parse(&args(&["--wait-ms"])).is_err());
    }

    #[test]
    fn enforces_ranges_and_membership() {
        let options = Options::parse(&args(&["--wait-ms", "10", "--json"]))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(options.number("--wait-ms", 1500, 0, 60_000), Ok(10));
        assert!(options.number("--wait-ms", 1500, 0, 5).is_err());
        assert!(options.allowed(&["--wait-ms", "--json"]).is_ok());
        assert!(options.allowed(&["--json"]).is_err());
        assert!(options.flag("--json").is_ok());
        assert!(options.flag("--wait-ms").is_err());
    }
}
