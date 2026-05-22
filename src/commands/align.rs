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
        let lines: Vec<String> = std::io::stdin().lock().lines().collect::<Result<_, _>>()?;
        let output = Self::align(re, &lines);
        println!("{}", output.join("\n"));
        Ok(())
    }
}

impl Align {
    fn align(re: Regex, lines: &[String]) -> Vec<String> {
        let mut max_end: Vec<usize> = Vec::new();
        for line in lines {
            for (i, m) in re.find_iter(line).enumerate() {
                if max_end.len() <= i {
                    max_end.push(m.end());
                } else if max_end[i] < m.end() {
                    max_end[i] = m.end();
                }
            }
        }

        let mut output = Vec::new();
        for line in lines {
            let matches: Vec<_> = re.find_iter(line).collect();
            if matches.is_empty() {
                output.push(line.to_string());
                continue;
            }
            let mut new_line = String::new();
            let mut prev_end = 0;
            let mut cumulative_padding = 0;
            for (i, m) in matches.into_iter().enumerate() {
                new_line.push_str(&line[prev_end..m.start()]);
                // Padding needed so that padded match ends at column max_end[i]
                let needed = max_end[i].saturating_sub(m.end() + cumulative_padding);
                let padding = " ".repeat(needed);
                new_line.push_str(&format!("{}{}", padding, m.as_str()));
                cumulative_padding += needed;
                prev_end = m.end();
            }
            new_line.push_str(&line[prev_end..]);
            output.push(new_line);
        }
        output
    }

    #[cfg(test)]
    fn align_str(pattern: &str, input: &str) -> String {
        let re = Regex::new(pattern).unwrap();
        let lines: Vec<String> = input.lines().map(str::to_string).collect();

        let mut out = Self::align(re, &lines).join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::*;

    #[test]
    fn test_align_on_equals() {
        let input = indoc! {"
            foo = bar
            baz = qux
        "};
        let output = Align::align_str("=", input);
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
        let output = Align::align_str("=", input);
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
        let output = Align::align_str(":", input);
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
        let output = Align::align_str("=", input);
        assert_eq!(
            output,
            indoc! {"
            foo bar
            baz qux
        "}
        );
    }

    #[test]
    fn test_align_with_repeats() {
        let input = indoc! {"
            foo = bar = baz = qux
            testing = values = are = hard
        "};
        let output = Align::align_str("=", input);
        assert_eq!(
            output,
            indoc! {"
            foo     = bar    = baz = qux
            testing = values = are = hard
        "}
        );
    }

    #[test]
    fn test_align_with_regex_metacharacters() {
        let input = indoc! {"
            a\\+b
            foo\\+bar
        "};
        let output = Align::align_str(r"\\+", input);
        assert_eq!(
            output,
            indoc! {"
            a  \\+b
            foo\\+bar
        "}
        );
    }
}
