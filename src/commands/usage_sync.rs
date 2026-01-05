use std::{fs, process::Command};

use anyhow::{bail, Context};
use clap::Parser;
use log::info;
use regex::Regex;

use crate::command::CommandRunner;

#[derive(Parser)]
#[command(
    name = "usage-sync",
    about = "Sync --help output of a command to README.md between marker comments or infer from placeholders"
)]
pub struct UsageSync {
    /// The command to run --help for
    command: String,

    /// Dry run: print what would be updated without modifying README.md
    #[arg(long)]
    dry_run: bool,

    /// Infer mode: scan for '{command} --help' in code blocks and replace with actual output
    #[arg(long)]
    infer: bool,
}

impl CommandRunner for UsageSync {
    fn run(self) -> anyhow::Result<()> {
        // Run the command --help
        let output = Command::new(&self.command)
            .arg("--help")
            .output()
            .with_context(|| format!("Failed to run '{} --help'", self.command))?;

        if !output.status.success() {
            bail!("Command '{}' --help failed", self.command);
        }

        let help_output = String::from_utf8(output.stdout)
            .with_context(|| format!("Output of '{} --help' is not valid UTF-8", self.command))?;

        // Read README.md
        let readme_path = "README.md";
        let readme_content = if std::path::Path::new(readme_path).exists() {
            fs::read_to_string(readme_path)?
        } else {
            String::new()
        };

        let new_content = update_readme(&readme_content, &self.command, &help_output, self.infer)?;

        if self.dry_run {
            println!("Dry run - would update README.md to:\n{}", new_content);
        } else {
            fs::write(readme_path, new_content)?;
            if self.infer {
                info!(
                    "Inferred and updated README.md with help output from '{}'",
                    self.command
                );
            } else {
                info!("Updated README.md with help output from '{}'", self.command);
            }
        }

        Ok(())
    }
}

fn has_markers(content: &str) -> bool {
    let start_marker = "<!-- HELP START -->";
    let end_marker = "<!-- HELP END -->";
    content.contains(start_marker) && content.contains(end_marker)
}

fn append_section(content: &str, formatted_output: &str) -> String {
    let start_marker = "<!-- HELP START -->";
    let end_marker = "<!-- HELP END -->";
    let extra = if content.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let section = format!(
        "{}## Usage\n\n{}\n{}\n{}\n",
        extra, start_marker, formatted_output, end_marker
    );
    format!("{}{}", content, section)
}

fn replace_markers(content: &str, formatted_output: &str) -> anyhow::Result<String> {
    let start_marker = "<!-- HELP START -->";
    let end_marker = "<!-- HELP END -->";
    let replacement = format!("{}\n{}\n{}", start_marker, formatted_output, end_marker);
    let pattern = format!(
        r"(?s){}(.*?){}",
        regex::escape(start_marker),
        regex::escape(end_marker)
    );
    let re = Regex::new(&pattern)?;
    Ok(re.replace_all(content, replacement.as_str()).to_string())
}

fn infer_replace(content: &str, command: &str, help_output: &str) -> anyhow::Result<String> {
    let placeholder = format!("{} --help", command);
    let pattern = format!(r"```\s*\n\s*{}\s*\n\s*```", regex::escape(&placeholder));
    let re = Regex::new(&pattern)?;
    let replacement = format!("```\n{}\n```", help_output.trim());
    Ok(re.replace_all(content, replacement.as_str()).to_string())
}

fn update_readme(
    content: &str,
    command: &str,
    help_output: &str,
    infer: bool,
) -> anyhow::Result<String> {
    // Format as markdown code block
    let formatted_output = format!("```\n{}\n```", help_output.trim());

    let new_content = if infer {
        let replaced = infer_replace(content, command, help_output)?;
        if replaced == content {
            append_section(content, &formatted_output)
        } else {
            replaced
        }
    } else if has_markers(content) {
        replace_markers(content, &formatted_output)?
    } else {
        append_section(content, &formatted_output)
    };

    Ok(new_content)
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::*;

    #[test]
    fn test_update_readme_append_section() {
        let content = indoc! {"
            # Test

            Some content.
        "};
        let command = "test-cmd";
        let help_output = "Usage: test-cmd [options]";
        let infer = false;

        let result = update_readme(content, command, help_output, infer).unwrap();

        let expected = indoc! {"
            # Test

            Some content.

            ## Usage

            <!-- HELP START -->
            ```
            Usage: test-cmd [options]
            ```
            <!-- HELP END -->
        "};

        assert_eq!(result, expected);
    }

    #[test]
    fn test_update_readme_replace_markers() {
        let content = indoc! {"
            # Test

            <!-- HELP START -->
            old content
            <!-- HELP END -->

            More.
        "};
        let command = "test-cmd";
        let help_output = "Usage: test-cmd [options]";
        let infer = false;

        let result = update_readme(content, command, help_output, infer).unwrap();

        let expected = indoc! {"
            # Test

            <!-- HELP START -->
            ```
            Usage: test-cmd [options]
            ```
            <!-- HELP END -->

            More.
        "};

        assert_eq!(result, expected);
    }

    #[test]
    fn test_update_readme_infer_mode() {
        let content = indoc! {"
            # Test

            ```
            test-cmd --help
            ```

            End.
        "};
        let command = "test-cmd";
        let help_output = "Usage: test-cmd [options]";
        let infer = true;

        let result = update_readme(content, command, help_output, infer).unwrap();

        let expected = indoc! {"
            # Test

            ```
            Usage: test-cmd [options]
            ```

            End.
        "};

        assert_eq!(result, expected);
    }

    #[test]
    fn test_update_readme_infer_mode_no_placeholder() {
        let content = "# Test\n\nSome content.";
        let command = "test-cmd";
        let help_output = "Usage: test-cmd [options]";
        let infer = true;

        let result = update_readme(content, command, help_output, infer).unwrap();

        let expected = indoc! {"
            # Test

            Some content.

            ## Usage

            <!-- HELP START -->
            ```
            Usage: test-cmd [options]
            ```
            <!-- HELP END -->
        "};

        assert_eq!(result, expected);
    }
}
