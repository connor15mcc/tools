use std::{
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::command::CommandRunner;

#[derive(Parser)]
#[command(name = "jsonsql", about = "Query a JSON file with DuckDB")]
pub struct Jsonsql {
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Subcommand)]
enum Mode {
    /// Run a single SQL query against JSON read from stdin
    Query {
        /// SQL clause to run
        sql: String,
    },
    /// Open an interactive DuckDB shell with a table loaded from JSON
    Shell {
        /// JSON file to load as a table (defaults to stdin, table named "stdin")
        file: Option<PathBuf>,
    },
}

fn sanitize_table_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}

fn table_name_from_path(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("data");
    sanitize_table_name(stem)
}

impl CommandRunner for Jsonsql {
    fn run(self) -> Result<()> {
        match self.mode {
            Mode::Query { sql } => run_query(&sql),
            Mode::Shell { file } => run_interactive(file),
        }
    }
}

fn run_query(query: &str) -> Result<()> {
    let status = Command::new("duckdb")
        .args(["-noheader", "-list", "-separator", "\t"])
        .arg("-c")
        .arg(format!("FROM read_json_auto('/dev/stdin') {query}"))
        .status()
        .context("failed to run duckdb (is it installed and on PATH?)")?;

    if !status.success() {
        anyhow::bail!("duckdb exited with {status}");
    }

    Ok(())
}

fn run_interactive(file: Option<PathBuf>) -> Result<()> {
    let (file, table, _tempfile) = match file {
        Some(path) => {
            let table = table_name_from_path(&path);
            (path, table, None)
        }
        None => {
            let mut tempfile = tempfile::NamedTempFile::new()
                .context("failed to create temp file for stdin")?;
            io::copy(&mut io::stdin(), &mut tempfile)
                .context("failed to buffer stdin to temp file")?;
            tempfile.flush()?;
            let path = tempfile.path().to_path_buf();
            (path, "stdin".to_string(), Some(tempfile))
        }
    };

    let file_str = file.to_string_lossy();
    let tty = File::open("/dev/tty").context("failed to open /dev/tty for interactive input")?;

    let status = Command::new("duckdb")
        .arg("-cmd")
        .arg(format!(
            "CREATE TABLE \"{table}\" AS FROM read_json_auto('{file_str}')"
        ))
        .arg("-cmd")
        .arg(format!("DESCRIBE \"{table}\""))
        .stdin(tty)
        .status()
        .context("failed to run duckdb (is it installed and on PATH?)")?;

    if !status.success() {
        anyhow::bail!("duckdb exited with {status}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_table_name_simple() {
        assert_eq!(sanitize_table_name("my_data"), "my_data");
    }

    #[test]
    fn test_sanitize_table_name_dashes() {
        assert_eq!(sanitize_table_name("go-version"), "go_version");
    }

    #[test]
    fn test_sanitize_table_name_spaces_and_dots() {
        assert_eq!(sanitize_table_name("my data.v2"), "my_data_v2");
    }

    #[test]
    fn test_sanitize_table_name_leading_digit() {
        // DuckDB accepts quoted identifiers starting with a digit, so no extra handling needed.
        assert_eq!(sanitize_table_name("2024_report"), "2024_report");
    }

    #[test]
    fn test_table_name_from_path_strips_extension() {
        assert_eq!(
            table_name_from_path(Path::new("/tmp/goversion.json")),
            "goversion"
        );
    }

    #[test]
    fn test_table_name_from_path_sanitizes_stem() {
        assert_eq!(
            table_name_from_path(Path::new("/tmp/go-version.report.json")),
            "go_version_report"
        );
    }

    #[test]
    fn test_table_name_from_path_no_extension() {
        assert_eq!(table_name_from_path(Path::new("data")), "data");
    }

    #[test]
    fn test_parses_query_subcommand() {
        let cmd = Jsonsql::parse_from(["jsonsql", "query", "WHERE a=1"]);
        assert!(matches!(cmd.mode, Mode::Query { sql } if sql == "WHERE a=1"));
    }

    #[test]
    fn test_parses_shell_subcommand_with_file() {
        let cmd = Jsonsql::parse_from(["jsonsql", "shell", "data.json"]);
        assert!(matches!(cmd.mode, Mode::Shell { file: Some(f) } if f == PathBuf::from("data.json")));
    }

    #[test]
    fn test_parses_shell_subcommand_without_file() {
        let cmd = Jsonsql::parse_from(["jsonsql", "shell"]);
        assert!(matches!(cmd.mode, Mode::Shell { file: None }));
    }
}
