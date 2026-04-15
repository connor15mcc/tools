use std::{
    env,
    io::{self, BufRead, Write},
    path::Path,
    process::Command,
};

use anyhow::Result;
use clap::Parser;
use indoc::indoc;

use crate::command::CommandRunner;

#[derive(Parser)]
#[command(
    name = "yank",
    about = "Copy command and its output to clipboard",
    long_about = indoc! {"
        Passes stdin through to stdout while capturing both
        the parent shell command and output to clipboard.

        Requirements:
        - Atuin must be installed and configured (https://atuin.sh)
    "},
    after_help = indoc! {"
        Examples:
          cat foo.txt | yank
          git status | yank
          ls -la | grep test | yank
    "}
)]
pub struct Yank {
    /// Command prompt marker
    #[arg(long, default_value = "$")]
    prompt: String,

    /// Include current working directory in the prompt
    #[arg(long)]
    cwd: bool,
}

impl CommandRunner for Yank {
    fn run(self) -> Result<()> {
        let output_lines = read_and_echo_stdin()?;
        log::debug!("Captured {} lines of output", output_lines.len());

        let mut command = get_parent_command()?;
        log::debug!("Detected command: {}", command);

        command = strip_yank_suffix(&command);
        log::debug!("Stripped command: {}", command);

        let clipboard_text =
            format_clipboard_content(&command, &output_lines, &self.prompt, self.cwd);
        log::debug!("Formatted {} bytes for clipboard", clipboard_text.len());

        copy_to_clipboard(&clipboard_text)?;
        log::debug!("Successfully copied to clipboard");

        Ok(())
    }
}

fn read_and_echo_stdin() -> Result<Vec<String>> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut lines = Vec::new();

    for line in stdin.lock().lines() {
        let line = line?;
        writeln!(stdout, "{}", line)?;
        lines.push(line);
    }

    Ok(lines)
}

fn get_parent_command() -> Result<String> {
    get_command_from_atuin().map_err(|e| {
        anyhow::anyhow!(
            indoc! {"
            Unable to detect parent command from atuin.

            Details: {}

            Requirements:
              - Install atuin: https://atuin.sh
              - Configure atuin for your shell (see atuin docs)
              - Make sure atuin is tracking your shell history

            You can verify atuin is working by running:
              atuin history last --cmd-only
        "},
            e
        )
    })
}

fn get_command_from_atuin() -> Result<String> {
    log::debug!("Fetching last command from atuin");

    let output = Command::new("atuin")
        .arg("history")
        .arg("last")
        .arg("--cmd-only")
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!(
                    "atuin command not found. Please install atuin from https://atuin.sh"
                )
            } else {
                anyhow::anyhow!("Failed to execute atuin: {}", e)
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("atuin command failed: {}", stderr.trim());
    }

    let command = String::from_utf8(output.stdout)
        .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in atuin output: {}", e))?;

    let command = command.trim().to_string();

    if command.is_empty() {
        anyhow::bail!("atuin returned empty command history");
    }

    log::debug!("Retrieved command from atuin: {}", command);
    Ok(command)
}

fn relativize_path(path: &Path) -> String {
    if let Ok(home) = env::var("HOME") {
        let home_path = Path::new(&home);
        if let Ok(rel) = path.strip_prefix(home_path) {
            return format!("~/{}", rel.display());
        }
    }
    path.display().to_string()
}

fn format_clipboard_content(
    command: &str,
    output_lines: &[String],
    prompt: &str,
    cwd: bool,
) -> String {
    let marker = if cwd {
        if let Ok(current_dir) = env::current_dir() {
            format!("{} {} ", relativize_path(&current_dir), prompt)
        } else {
            format!("{} ", prompt)
        }
    } else {
        format!("{} ", prompt)
    };

    let mut result = format!("{}{}\n", marker, command);
    for line in output_lines {
        result.push_str(line);
        result.push('\n');
    }
    result
}

fn strip_yank_suffix(command: &str) -> String {
    if let Some(last_pipe_pos) = command.rfind('|') {
        return command[..last_pipe_pos].trim_end().to_string();
    }
    command.to_string()
}

fn copy_to_clipboard(text: &str) -> Result<()> {
    use arboard::Clipboard;

    let mut clipboard =
        Clipboard::new().map_err(|e| anyhow::anyhow!("Failed to access clipboard: {}", e))?;

    clipboard
        .set_text(text)
        .map_err(|e| anyhow::anyhow!("Failed to copy to clipboard: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_clipboard_content_single_line() {
        let cmd = "echo hello";
        let output = vec!["hello".to_string()];
        let result = format_clipboard_content(cmd, &output, "$", false);
        assert_eq!(result, "$ echo hello\nhello\n");
    }

    #[test]
    fn test_format_clipboard_content_multiple_lines() {
        let cmd = "cat file.txt";
        let output = vec![
            "line 1".to_string(),
            "line 2".to_string(),
            "line 3".to_string(),
        ];
        let result = format_clipboard_content(cmd, &output, "$", false);
        assert_eq!(result, "$ cat file.txt\nline 1\nline 2\nline 3\n");
    }

    #[test]
    fn test_format_clipboard_content_empty_output() {
        let cmd = "true";
        let output: Vec<String> = vec![];
        let result = format_clipboard_content(cmd, &output, "$", false);
        assert_eq!(result, "$ true\n");
    }

    #[test]
    fn test_format_clipboard_content_with_pipe() {
        let cmd = "cat foo.txt | grep bar";
        let output = vec!["matched line".to_string()];
        let result = format_clipboard_content(cmd, &output, "$", false);
        assert_eq!(result, "$ cat foo.txt | grep bar\nmatched line\n");
    }

    #[test]
    fn test_strip_yank_suffix_basic() {
        assert_eq!(strip_yank_suffix("cat foo.txt | yank"), "cat foo.txt");
    }

    #[test]
    fn test_strip_yank_suffix_with_flags() {
        assert_eq!(strip_yank_suffix("cat foo.txt | yank -v"), "cat foo.txt");
    }

    #[test]
    fn test_strip_yank_suffix_multi_pipe() {
        assert_eq!(
            strip_yank_suffix("cat foo.txt | grep bar | yank"),
            "cat foo.txt | grep bar"
        );
    }

    #[test]
    fn test_strip_yank_suffix_quoted_pipe() {
        assert_eq!(
            strip_yank_suffix("echo 'hello | world' | yank"),
            "echo 'hello | world'"
        );
    }

    #[test]
    fn test_strip_yank_suffix_no_yank() {
        // Now strips everything after last pipe, even if it's not "yank"
        assert_eq!(strip_yank_suffix("cat foo.txt | grep bar"), "cat foo.txt");
    }

    #[test]
    fn test_strip_yank_suffix_no_pipe() {
        assert_eq!(strip_yank_suffix("cat foo.txt"), "cat foo.txt");
    }

    #[test]
    fn test_strip_yank_suffix_yank_in_middle() {
        // "yank" in middle is fine, only strips after the last pipe
        assert_eq!(strip_yank_suffix("cat yank.txt | grep bar"), "cat yank.txt");
    }

    #[test]
    fn test_strip_yank_suffix_cargo_run() {
        // Works with cargo run invocation too
        assert_eq!(
            strip_yank_suffix("echo 'foo' | cargo run -- yank"),
            "echo 'foo'"
        );
    }

    #[test]
    fn test_strip_yank_suffix_whitespace_variations() {
        assert_eq!(strip_yank_suffix("cat foo.txt|yank"), "cat foo.txt");
        assert_eq!(strip_yank_suffix("cat foo.txt |yank"), "cat foo.txt");
        assert_eq!(strip_yank_suffix("cat foo.txt | yank"), "cat foo.txt");
        assert_eq!(strip_yank_suffix("cat foo.txt  |  yank"), "cat foo.txt");
    }
}
