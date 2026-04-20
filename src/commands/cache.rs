use std::{
    env,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use clap::Parser;
use humantime::parse_duration as humantime_parse;
use log::info;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::command::CommandRunner;

fn parse_ttl(s: &str) -> Result<Duration, String> {
    humantime_parse(s).map_err(|e| format!("Invalid TTL '{}': {}", s, e))
}

fn cache_dir() -> Result<PathBuf> {
    if let Some(home) = env::var_os("HOME") {
        let cache_home = env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(home).join(".cache"));
        return Ok(cache_home.join("tools"));
    }
    anyhow::bail!("Could not determine cache directory: HOME not set")
}

fn ensure_cache_dir() -> Result<PathBuf> {
    let dir = cache_dir()?;
    if !dir.exists() {
        fs::create_dir_all(&dir).context("Failed to create cache directory")?;
    }
    Ok(dir)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    command: String,
    pwd: String,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    cached_at: u64,
    ttl_secs: u64,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now > self.cached_at + self.ttl_secs
    }
}

fn compute_cache_key(command: &str, pwd: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(command.as_bytes());
    hasher.update(pwd.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

fn get_shell() -> String {
    env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

#[derive(Parser)]
#[command(
    name = "cache",
    about = "Cache command output to disk with TTL",
    long_about = "Execute a command and cache its output to disk. Future invocations\nwith the same command and working directory will return cached results until\nthe TTL expires."
)]
pub struct Cache {
    /// Cache TTL (e.g., "5m", "1h", "30s")
    #[arg(long, value_parser = parse_ttl, default_value = "15m")]
    ttl: Duration,

    /// Reset cache before executing
    #[arg(long)]
    reset: bool,

    /// Command to execute and cache
    #[arg(required = true)]
    command: String,
}

impl CommandRunner for Cache {
    fn run(self) -> Result<()> {
        let cache_dir = ensure_cache_dir()?;
        let pwd = env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let command = &self.command;

        let cache_key = compute_cache_key(command, &pwd);
        let cache_path = cache_dir.join(&cache_key);

        if self.reset && cache_path.exists() {
            fs::remove_file(&cache_path)?;
        }

        if let Some(entry) = load_cache_entry(&cache_path)? {
            if !entry.is_expired() {
                info!(
                    "cache hit (command: {}, pwd: {}, cached {}s ago)",
                    command,
                    pwd,
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                        - entry.cached_at
                );
                io::stdout().write_all(&entry.stdout)?;
                io::stderr().write_all(&entry.stderr)?;
                return Ok(());
            }
        } else {
            info!("cache miss (not found / expired)");
        }
        info!("executing: {}", command);

        let shell = get_shell();
        let output = Command::new(&shell)
            .arg("-c")
            .arg(command)
            .current_dir(&pwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| format!("Failed to execute command: {}", command))?;

        let entry = CacheEntry {
            command: command.to_string(),
            pwd: pwd.clone(),
            stdout: output.stdout,
            stderr: output.stderr,
            cached_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            ttl_secs: self.ttl.as_secs(),
        };

        save_cache_entry(&cache_path, &entry)?;

        io::stdout().write_all(&entry.stdout)?;
        io::stderr().write_all(&entry.stderr)?;

        Ok(())
    }
}

fn load_cache_entry(path: &Path) -> Result<Option<CacheEntry>> {
    if !path.exists() {
        return Ok(None);
    }

    let mut file = File::open(path).context("Failed to open cache file")?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)
        .context("Failed to read cache file")?;

    let entry: CacheEntry =
        serde_json::from_slice(&contents).context("Failed to parse cache file")?;

    Ok(Some(entry))
}

fn save_cache_entry(path: &Path, entry: &CacheEntry) -> Result<()> {
    let contents = serde_json::to_vec(entry).context("Failed to serialize cache entry")?;
    let mut file = File::create(path).context("Failed to create cache file")?;
    file.write_all(&contents)
        .context("Failed to write cache file")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ttl_valid() {
        assert_eq!(parse_ttl("1s").unwrap(), Duration::from_secs(1));
        assert_eq!(parse_ttl("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_ttl("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_ttl("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_ttl("1h30m").unwrap(), Duration::from_secs(5400));
    }

    #[test]
    fn test_parse_ttl_invalid() {
        assert!(parse_ttl("invalid").is_err());
        assert!(parse_ttl("").is_err());
    }

    #[test]
    fn test_cache_key_deterministic() {
        let key1 = compute_cache_key("echo hello", "/home/user");
        let key2 = compute_cache_key("echo hello", "/home/user");
        let key3 = compute_cache_key("echo world", "/home/user");

        assert_eq!(key1, key2, "same command and pwd should produce same key");
        assert_ne!(key1, key3, "different command should produce different key");
    }

    #[test]
    fn test_cache_key_includes_pwd() {
        let key1 = compute_cache_key("echo hello", "/home/user");
        let key2 = compute_cache_key("echo hello", "/home/other");

        assert_ne!(key1, key2, "different pwd should produce different key");
    }

    #[test]
    fn test_cache_key_different_pwd_same_command() {
        let key1 = compute_cache_key("curl http://example.com", "/tmp");
        let key2 = compute_cache_key("curl http://example.com", "/home/user");

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_cache_entry_is_expired() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let entry_fresh = CacheEntry {
            command: "echo hello".to_string(),
            pwd: "/home".to_string(),
            stdout: b"hello\n".to_vec(),
            stderr: b"".to_vec(),
            cached_at: now,
            ttl_secs: 60,
        };
        assert!(!entry_fresh.is_expired());

        let entry_expired = CacheEntry {
            command: "echo hello".to_string(),
            pwd: "/home".to_string(),
            stdout: b"hello\n".to_vec(),
            stderr: b"".to_vec(),
            cached_at: now - 120,
            ttl_secs: 60,
        };
        assert!(entry_expired.is_expired());
    }

    #[test]
    fn test_cache_entry_not_expired_at_exact_ttl() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let entry = CacheEntry {
            command: "echo hello".to_string(),
            pwd: "/home".to_string(),
            stdout: b"hello\n".to_vec(),
            stderr: b"".to_vec(),
            cached_at: now,
            ttl_secs: 0,
        };
        assert!(!entry.is_expired(), "at exactly TTL should not be expired");
    }

    #[test]
    fn test_get_shell_from_env() {
        let shell = get_shell();
        assert!(!shell.is_empty());
    }

    #[test]
    fn test_cache_serialize_deserialize() {
        let entry = CacheEntry {
            command: "echo test".to_string(),
            pwd: "/home/user".to_string(),
            stdout: b"test output\n".to_vec(),
            stderr: b"error output\n".to_vec(),
            cached_at: 1234567890,
            ttl_secs: 300,
        };

        let json = serde_json::to_vec(&entry).unwrap();
        let loaded: CacheEntry = serde_json::from_slice(&json).unwrap();

        assert_eq!(loaded.command, entry.command);
        assert_eq!(loaded.pwd, entry.pwd);
        assert_eq!(loaded.stdout, entry.stdout);
        assert_eq!(loaded.stderr, entry.stderr);
        assert_eq!(loaded.cached_at, entry.cached_at);
        assert_eq!(loaded.ttl_secs, entry.ttl_secs);
    }

    #[test]
    fn test_cache_entry_with_binary_output() {
        let entry = CacheEntry {
            command: "cat binary.dat".to_string(),
            pwd: "/tmp".to_string(),
            stdout: vec![0x00, 0x01, 0x02, 0xFF, 0xFE],
            stderr: vec![],
            cached_at: 1234567890,
            ttl_secs: 60,
        };

        let json = serde_json::to_vec(&entry).unwrap();
        let loaded: CacheEntry = serde_json::from_slice(&json).unwrap();

        assert_eq!(loaded.stdout, vec![0x00, 0x01, 0x02, 0xFF, 0xFE]);
    }

    #[test]
    fn test_cache_key_hex_encoding() {
        let key = compute_cache_key("echo hello", "/home/user");

        assert_eq!(key.len(), 64, "SHA256 produces 32 bytes = 64 hex chars");

        for c in key.chars() {
            assert!(c.is_ascii_hexdigit(), "key should be valid hex: {}", c);
        }
    }
}
