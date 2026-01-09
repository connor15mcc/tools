use clap::Parser;
use regex::Regex;
use std::fs;
use std::process::Command;

use crate::{
    command::CommandRunner,
    languages::{LanguageSupport, LANGUAGES},
};

#[derive(Parser)]
#[command(
    name = "cmdctl",
    about = "Execute commands in comments and replace adjacent code"
)]
pub struct CmdCtl {
    file: String,
}

impl CommandRunner for CmdCtl {
    fn run(self) -> anyhow::Result<()> {
        process_file(&self.file)?;
        Ok(())
    }
}

fn process_file(file_path: &str) -> anyhow::Result<()> {
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("No file extension"))?;

    let lang = LANGUAGES
        .iter()
        .find(|l| l.extensions().contains(&ext))
        .ok_or_else(|| anyhow::anyhow!("Unsupported language"))?;

    let source = fs::read_to_string(file_path)?;
    let mut parser = tree_sitter::Parser::new();
    let language = lang.treesitter_language();
    parser.set_language(&language)?;
    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| anyhow::anyhow!("Parse failed"))?;

    let mut replacements = Vec::new();

    traverse(tree.root_node(), &source, *lang, &mut replacements);

    // Apply replacements from end to start to preserve positions
    replacements.sort_by_key(|&(start, ..)| std::cmp::Reverse(start));
    let mut new_source = source;
    for (start, end, output) in replacements {
        new_source = lang.replace_node_content(&new_source, start, end, &output);
    }

    fs::write(file_path, new_source)?;
    Ok(())
}

fn is_comment_node(node: &tree_sitter::Node) -> bool {
    node.kind() == "comment" || node.kind() == "line_comment"
}

fn extract_command(comment_text: &str) -> Option<String> {
    let re = Regex::new(r"^\s*(?://|#)\s*\$\s*(.+)$").unwrap();
    re.captures(comment_text)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
}

fn find_replaceable_node(
    node: tree_sitter::Node,
    lang: &dyn LanguageSupport,
) -> Option<(usize, usize)> {
    let mut candidates = Vec::new();
    fn collect(
        node: tree_sitter::Node,
        lang: &dyn LanguageSupport,
        candidates: &mut Vec<(usize, usize)>,
    ) {
        if lang.is_replaceable_node(&node) {
            candidates.push((node.start_byte(), node.end_byte()));
        }
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                collect(cursor.node(), lang, candidates);
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }
    collect(node, lang, &mut candidates);
    // Return the one with the highest start position (last in source)
    candidates.into_iter().max_by_key(|&(start, _)| start)
}

fn traverse(
    node: tree_sitter::Node,
    source: &str,
    lang: &dyn LanguageSupport,
    replacements: &mut Vec<(usize, usize, String)>,
) {
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if is_comment_node(&child) {
                if let Some(command) =
                    extract_command(&source[child.start_byte()..child.end_byte()])
                {
                    if let Some(next_node) = child.next_sibling() {
                        if next_node.kind() != "whitespace" && next_node.kind() != "newline" {
                            if let Some((start, end)) = find_replaceable_node(next_node, lang) {
                                if let Ok(output) = execute_command(&command) {
                                    replacements.push((start, end, output));
                                }
                            }
                        }
                    }
                }
            } else {
                traverse(child, source, lang, replacements);
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

fn execute_command(command: &str) -> anyhow::Result<String> {
    let output = Command::new("sh").arg("-c").arg(command).output()?;
    if output.status.success() {
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    } else {
        Err(anyhow::anyhow!(
            "Command failed: {}",
            String::from_utf8(output.stderr)?
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use indoc::indoc;
    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn test_python_seconds() {
        let before = indoc! {r#"
            # $ python3 -c "print(60 * 60 * 24)"
            SECONDS_PER_DAY = 0
        "#};
        let after = indoc! {r#"
            # $ python3 -c "print(60 * 60 * 24)"
            SECONDS_PER_DAY = 86400
        "#};
        let temp_file = NamedTempFile::with_suffix(".py").unwrap();
        fs::write(&temp_file, before).unwrap();
        process_file(temp_file.path().to_str().unwrap()).unwrap();
        let result = fs::read_to_string(&temp_file).unwrap();
        assert_eq!(result.trim(), after.trim());
    }

    #[test]
    fn test_javascript_seconds() {
        let before = indoc! {r#"
            // $ python3 -c "print(60 * 60 * 24)"
            const SECONDS_PER_DAY = 0;
        "#};
        let after = indoc! {r#"
            // $ python3 -c "print(60 * 60 * 24)"
            const SECONDS_PER_DAY = 86400;
        "#};
        let temp_file = NamedTempFile::with_suffix(".js").unwrap();
        fs::write(&temp_file, before).unwrap();
        process_file(temp_file.path().to_str().unwrap()).unwrap();
        let result = fs::read_to_string(&temp_file).unwrap();
        assert_eq!(result.trim(), after.trim());
    }

    #[test]
    fn test_rust_seconds() {
        let before = indoc! {r#"
            // $ python3 -c "print(60 * 60 * 24)"
            const SECONDS_PER_DAY: usize = 0;
        "#};
        let after = indoc! {r#"
            // $ python3 -c "print(60 * 60 * 24)"
            const SECONDS_PER_DAY: usize = 86400;
        "#};
        let temp_file = NamedTempFile::with_suffix(".rs").unwrap();
        fs::write(&temp_file, before).unwrap();
        process_file(temp_file.path().to_str().unwrap()).unwrap();
        let result = fs::read_to_string(&temp_file).unwrap();
        assert_eq!(result.trim(), after.trim());
    }

    #[test]
    fn test_python_seq_list() {
        let before = indoc! {r#"
            # $ seq 1 5 | tr '\n' ',' | sed 's/,$//'
            NUMBERS = []
        "#};
        let after = indoc! {r#"
            # $ seq 1 5 | tr '\n' ',' | sed 's/,$//'
            NUMBERS = [1,2,3,4,5]
        "#};
        let temp_file = NamedTempFile::with_suffix(".py").unwrap();
        fs::write(&temp_file, before).unwrap();
        process_file(temp_file.path().to_str().unwrap()).unwrap();
        let result = fs::read_to_string(&temp_file).unwrap();
        assert_eq!(result.trim(), after.trim());
    }

    #[test]
    fn test_python_string_const() {
        let before = indoc! {r#"
            # $ echo "Hello, World!"
            MESSAGE = ""
        "#};
        let after = indoc! {r#"
            # $ echo "Hello, World!"
            MESSAGE = "Hello, World!"
        "#};
        let temp_file = NamedTempFile::with_suffix(".py").unwrap();
        fs::write(&temp_file, before).unwrap();
        process_file(temp_file.path().to_str().unwrap()).unwrap();
        let result = fs::read_to_string(&temp_file).unwrap();
        assert_eq!(result.trim(), after.trim());
    }

    #[test]
    fn test_rust_seq_list() {
        let before = indoc! {r#"
            // $ seq 1 5 | tr '\n' ',' | sed 's/,$//'
            const NUMBERS: [usize; 5] = [];
        "#};
        let after = indoc! {r#"
            // $ seq 1 5 | tr '\n' ',' | sed 's/,$//'
            const NUMBERS: [usize; 5] = [1,2,3,4,5];
        "#};
        let temp_file = NamedTempFile::with_suffix(".rs").unwrap();
        fs::write(&temp_file, before).unwrap();
        process_file(temp_file.path().to_str().unwrap()).unwrap();
        let result = fs::read_to_string(&temp_file).unwrap();
        assert_eq!(result.trim(), after.trim());
    }

    #[test]
    fn test_rust_string_const() {
        let before = indoc! {r#"
            // $ echo "Hello, World!"
            const MESSAGE: &str = "";
        "#};
        let after = indoc! {r#"
            // $ echo "Hello, World!"
            const MESSAGE: &str = "Hello, World!";
        "#};
        let temp_file = NamedTempFile::with_suffix(".rs").unwrap();
        fs::write(&temp_file, before).unwrap();
        process_file(temp_file.path().to_str().unwrap()).unwrap();
        let result = fs::read_to_string(&temp_file).unwrap();
        assert_eq!(result.trim(), after.trim());
    }

    #[test]
    fn test_js_seq_list() {
        let before = indoc! {r#"
            // $ seq 1 5 | tr '\n' ',' | sed 's/,$//'
            const NUMBERS = [];
        "#};
        let after = indoc! {r#"
            // $ seq 1 5 | tr '\n' ',' | sed 's/,$//'
            const NUMBERS = [1,2,3,4,5];
        "#};
        let temp_file = NamedTempFile::with_suffix(".js").unwrap();
        fs::write(&temp_file, before).unwrap();
        process_file(temp_file.path().to_str().unwrap()).unwrap();
        let result = fs::read_to_string(&temp_file).unwrap();
        assert_eq!(result.trim(), after.trim());
    }

    #[test]
    fn test_js_string_const() {
        let before = indoc! {r#"
            // $ echo "Hello, World!"
            const MESSAGE = "";
        "#};
        let after = indoc! {r#"
            // $ echo "Hello, World!"
            const MESSAGE = "Hello, World!";
        "#};
        let temp_file = NamedTempFile::with_suffix(".js").unwrap();
        fs::write(&temp_file, before).unwrap();
        process_file(temp_file.path().to_str().unwrap()).unwrap();
        let result = fs::read_to_string(&temp_file).unwrap();
        assert_eq!(result.trim(), after.trim());
    }
}
