use std::io::BufRead;

use anyhow::Context;
use clap::Parser;
use regex::Regex;

use crate::command::CommandRunner;

#[derive(Parser)]
#[command(name = "align", about = "Align text on a regex pattern")]
pub struct Align {
    /// Regex pattern to align on
    pattern: String,
}

impl CommandRunner for Align {
    fn run(self) -> anyhow::Result<()> {
        let re = Regex::new(&self.pattern).context("Invalid regex pattern")?;

        let stdin = std::io::stdin();
        let mut lines = Vec::new();

        for line in stdin.lock().lines() {
            lines.push(line?);
        }

        let max_end = lines
            .iter()
            .filter_map(|line| re.find(line).map(|m| m.end()))
            .max()
            .unwrap_or(0);

        for line in lines {
            if let Some(m) = re.find(&line) {
                let match_start = m.start();
                let match_end = m.end();
                let padding = " ".repeat(max_end - match_end);
                let before_match = &line[..match_start];
                let after_match = &line[match_end..];
                println!("{}{}{}{}", before_match, padding, m.as_str(), after_match);
            } else {
                println!("{}", line);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::*;

    fn run_align(pattern: &str, input: &str) -> String {
        let re = Regex::new(pattern).unwrap();
        let lines: Vec<&str> = input.lines().collect();

        let max_end = lines
            .iter()
            .filter_map(|line| re.find(line).map(|m: regex::Match| m.end()))
            .max()
            .unwrap_or(0);

        let mut output = String::new();
        for line in lines {
            if let Some(m) = re.find(line) {
                let match_start = m.start();
                let match_end = m.end();
                let padding = " ".repeat(max_end - match_end);
                let before_match = &line[..match_start];
                let after_match = &line[match_end..];
                output.push_str(&format!(
                    "{}{}{}{}\n",
                    before_match,
                    padding,
                    m.as_str(),
                    after_match
                ));
            } else {
                output.push_str(&format!("{}\n", line));
            }
        }

        output
    }

    #[test]
    fn test_align_on_equals() {
        let input = indoc! {"
            foo = bar
            baz = qux
        "};
        let output = run_align("=", input);
        assert_eq!(
            output,
            indoc! {"
            foo = bar
            baz = qux
        "}
        );
    }

    #[test]
    fn test_align_on_equals_different_lengths() {
        let input = indoc! {"
            a = b
            longer = c
        "};
        let output = run_align("=", input);
        assert_eq!(
            output,
            indoc! {"
            a      = b
            longer = c
        "}
        );
    }

    #[test]
    fn test_align_on_colon() {
        let input = indoc! {"
            key: value
            longkey: val
        "};
        let output = run_align(":", input);
        assert_eq!(
            output,
            indoc! {"
            key    : value
            longkey: val
        "}
        );
    }

    #[test]
    fn test_align_with_no_matches() {
        let input = indoc! {"
            foo bar
            baz qux
        "};
        let output = run_align("=", input);
        assert_eq!(
            output,
            indoc! {"
            foo bar
            baz qux
        "}
        );
    }

    #[test]
    fn test_align_with_regex_metacharacters() {
        let input = indoc! {"
            a\\+b
            foo\\+bar
        "};
        let output = run_align(r"\\+", input);
        assert_eq!(
            output,
            indoc! {"
            a  \\+b
            foo\\+bar
        "}
        );
    }
}
