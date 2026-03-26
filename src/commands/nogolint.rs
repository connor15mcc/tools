use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Read},
};

use anyhow::{Context, Result};
use clap::Parser;
use clap_stdin::FileOrStdin;
use regex::Regex;

use crate::command::CommandRunner;

#[derive(Parser)]
#[command(
    name = "nogolint",
    about = "Add nolint directives to suppress golangci-lint findings"
)]
pub struct NoGolint {
    /// Show what would change without modifying files
    #[arg(short = 'n', long)]
    dry_run: bool,

    /// Add a custom reason to nolint directives (e.g., "TODO: fix later")
    #[arg(short, long)]
    reason: Option<String>,

    /// Input from golangci-lint (file or stdin)
    #[clap(default_value = "-")]
    input: FileOrStdin,
}

/// A parsed lint finding from golangci-lint output
#[derive(Debug)]
struct LintFinding {
    file: String,
    line: usize,
    linter: String,
}

/// Parse golangci-lint default output format
/// Format: path/to/file.go:42:15: error message (lintername)
fn parse_lint_output(reader: impl Read) -> Result<Vec<LintFinding>> {
    let re = Regex::new(r"^(.+?):(\d+):\d+:.*\((\w+)\)$")?;
    let buf_reader = BufReader::new(reader);
    let mut findings = Vec::new();

    for line in buf_reader.lines() {
        let line = line?;
        if let Some(caps) = re.captures(&line) {
            findings.push(LintFinding {
                file: caps[1].to_string(),
                line: caps[2].parse()?,
                linter: caps[3].to_string(),
            });
        }
        // Lines that don't match are silently ignored (could be header/footer noise)
    }

    Ok(findings)
}

/// Group findings by file, then by line number, collecting linters
/// Returns: BTreeMap<file, BTreeMap<line, BTreeSet<linters>>>
fn group_findings(
    findings: Vec<LintFinding>,
) -> BTreeMap<String, BTreeMap<usize, BTreeSet<String>>> {
    let mut grouped: BTreeMap<String, BTreeMap<usize, BTreeSet<String>>> = BTreeMap::new();

    for finding in findings {
        grouped
            .entry(finding.file)
            .or_default()
            .entry(finding.line)
            .or_default()
            .insert(finding.linter);
    }

    grouped
}

/// Build the nolint directive string
fn build_directive(linters: &BTreeSet<String>, reason: Option<&str>) -> String {
    let linter_list = linters.iter().cloned().collect::<Vec<_>>().join(",");
    match reason {
        Some(r) => format!(" //nolint:{} // {}", linter_list, r),
        None => format!(" //nolint:{}", linter_list),
    }
}

/// Apply nolint directives to a file's contents
/// Returns the modified content
fn apply_directives(
    content: &str,
    line_directives: &BTreeMap<usize, BTreeSet<String>>,
    reason: Option<&str>,
) -> String {
    let mut result: String = content
        .lines()
        .enumerate()
        .map(|(idx, line)| {
            let line_num = idx + 1; // 1-indexed
            if let Some(linters) = line_directives.get(&line_num) {
                let directive = build_directive(linters, reason);
                format!("{}{}", line.trim_end(), directive)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Ensure file ends with newline
    result.push('\n');
    result
}

impl CommandRunner for NoGolint {
    fn run(self) -> Result<()> {
        let reader = self.input.into_reader()?;
        let findings = parse_lint_output(reader)?;

        if findings.is_empty() {
            eprintln!("No lint findings parsed from input");
            return Ok(());
        }

        let grouped = group_findings(findings);
        let reason = self.reason.as_deref();

        let mut total_files = 0;
        let mut total_lines = 0;

        for (file_path, line_directives) in &grouped {
            let content = fs::read_to_string(file_path)
                .with_context(|| format!("Failed to read {}", file_path))?;

            let modified = apply_directives(&content, line_directives, reason);

            if self.dry_run {
                // Print diff-like output
                println!("--- {}", file_path);
                for (line_num, linters) in line_directives {
                    let directive = build_directive(linters, reason);
                    println!("  L{}: +{}", line_num, directive.trim());
                }
            } else {
                fs::write(file_path, &modified)
                    .with_context(|| format!("Failed to write {}", file_path))?;
            }

            total_files += 1;
            total_lines += line_directives.len();
        }

        if self.dry_run {
            eprintln!(
                "\nDry run: would modify {} lines in {} files",
                total_lines, total_files
            );
        } else {
            eprintln!("Modified {} lines in {} files", total_lines, total_files);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use std::io::Cursor;

    #[test]
    fn test_parse_lint_output() {
        let input = indoc! {r#"
            main.go:10:5: error message here (staticcheck)
            pkg/foo.go:42:1: another error (errcheck)
            noise line without match
            main.go:10:20: different error same line (gosimple)
        "#};

        let findings = parse_lint_output(Cursor::new(input)).unwrap();

        assert_eq!(findings.len(), 3);
        assert_eq!(findings[0].file, "main.go");
        assert_eq!(findings[0].line, 10);
        assert_eq!(findings[0].linter, "staticcheck");
        assert_eq!(findings[1].file, "pkg/foo.go");
        assert_eq!(findings[1].line, 42);
        assert_eq!(findings[1].linter, "errcheck");
        assert_eq!(findings[2].file, "main.go");
        assert_eq!(findings[2].line, 10);
        assert_eq!(findings[2].linter, "gosimple");
    }

    #[test]
    fn test_group_findings() {
        let findings = vec![
            LintFinding {
                file: "a.go".into(),
                line: 10,
                linter: "x".into(),
            },
            LintFinding {
                file: "a.go".into(),
                line: 10,
                linter: "y".into(),
            },
            LintFinding {
                file: "a.go".into(),
                line: 20,
                linter: "z".into(),
            },
            LintFinding {
                file: "b.go".into(),
                line: 5,
                linter: "x".into(),
            },
        ];

        let grouped = group_findings(findings);

        assert_eq!(grouped.len(), 2); // 2 files
        assert_eq!(grouped["a.go"].len(), 2); // 2 lines in a.go
        assert_eq!(grouped["a.go"][&10].len(), 2); // 2 linters on line 10
        assert!(grouped["a.go"][&10].contains("x"));
        assert!(grouped["a.go"][&10].contains("y"));
    }

    #[test]
    fn test_apply_directives() {
        let content = indoc! {r#"
            package main

            func foo() {
                x := 1 // existing comment
            }
        "#};

        let mut line_directives = BTreeMap::new();
        let mut linters = BTreeSet::new();
        linters.insert("staticcheck".to_string());
        linters.insert("errcheck".to_string());
        line_directives.insert(4, linters);

        let result = apply_directives(content.trim_end(), &line_directives, None);

        assert!(result.contains("x := 1 // existing comment //nolint:errcheck,staticcheck"));
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn test_apply_directives_with_reason() {
        let content = "    x := 1";
        let mut line_directives = BTreeMap::new();
        let mut linters = BTreeSet::new();
        linters.insert("foo".to_string());
        line_directives.insert(1, linters);

        let result = apply_directives(content, &line_directives, Some("TODO: fix"));

        assert_eq!(result, "    x := 1 //nolint:foo // TODO: fix\n");
    }

    #[test]
    fn test_build_directive() {
        let mut linters = BTreeSet::new();
        linters.insert("b".to_string());
        linters.insert("a".to_string());

        assert_eq!(build_directive(&linters, None), " //nolint:a,b");
        assert_eq!(
            build_directive(&linters, Some("reason")),
            " //nolint:a,b // reason"
        );
    }

    #[test]
    fn test_parse_lint_output_empty() {
        let input = "";
        let findings = parse_lint_output(Cursor::new(input)).unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn test_parse_lint_output_no_matches() {
        let input = indoc! {r#"
            some random text
            another line without lint format
        "#};
        let findings = parse_lint_output(Cursor::new(input)).unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn test_apply_directives_preserves_unmodified_lines() {
        let content = indoc! {r#"
            line 1
            line 2
            line 3
        "#};

        let mut line_directives = BTreeMap::new();
        let mut linters = BTreeSet::new();
        linters.insert("foo".to_string());
        line_directives.insert(2, linters);

        let result = apply_directives(content.trim_end(), &line_directives, None);

        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines[0], "line 1");
        assert_eq!(lines[1], "line 2 //nolint:foo");
        assert_eq!(lines[2], "line 3");
    }
}
