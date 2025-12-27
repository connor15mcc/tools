use crate::command::CommandRunner;
use clap::Parser;
use clap_stdin::FileOrStdin;
use comrak::nodes::{AstNode, NodeValue};
use comrak::{parse_document, Arena, ComrakOptions};
use std::io::{Read, Write};

#[derive(Parser)]
#[command(name = "mdflow", about = "Reflow markdown text")]
pub struct MdFlow {
    #[command(subcommand)]
    command: FlowCommand,
}

#[derive(Parser)]
enum FlowCommand {
    /// Remove hard wrapping from paragraphs
    Unwrap {
        /// File from which to read, defaulting to stdin
        #[clap(default_value = "-")]
        input: FileOrStdin,
    },

    /// Hard wrap paragraphs at specified column width
    Wrap {
        /// Column width for wrapping (default: 80)
        #[arg(short = 'w', long = "width", default_value_t = 80)]
        width: usize,

        /// File from which to read, defaulting to stdin
        #[clap(default_value = "-")]
        input: FileOrStdin,
    },
}

impl CommandRunner for MdFlow {
    fn run(self) -> anyhow::Result<()> {
        match self.command {
            FlowCommand::Unwrap { input } => {
                let mut reader = input.into_reader()?;
                let mut content = String::new();
                reader.read_to_string(&mut content)?;

                let unwrapped = unwrap_markdown(&content)?;

                std::io::stdout().write_all(unwrapped.as_bytes())?;
                Ok(())
            }
            FlowCommand::Wrap { width, input } => {
                let mut reader = input.into_reader()?;
                let mut content = String::new();
                reader.read_to_string(&mut content)?;

                let wrapped = wrap_markdown(&content, width)?;

                std::io::stdout().write_all(wrapped.as_bytes())?;
                Ok(())
            }
        }
    }
}

// ===== UNWRAP FUNCTIONALITY =====

fn unwrap_markdown(content: &str) -> anyhow::Result<String> {
    let arena = Arena::new();
    let options = ComrakOptions::default();

    let root = parse_document(&arena, content, &options);

    // Walk the AST and unwrap paragraphs
    unwrap_paragraphs(&arena, root);

    // Format back to markdown
    let mut output = Vec::new();
    comrak::format_commonmark(root, &options, &mut output)?;

    Ok(String::from_utf8(output)?)
}

fn unwrap_paragraphs<'a>(arena: &'a Arena<AstNode<'a>>, node: &'a AstNode<'a>) {
    // Process current node
    if let NodeValue::Paragraph = node.data.borrow().value {
        unwrap_paragraph_text(arena, node);
    }

    // Recursively process children
    for child in node.children() {
        unwrap_paragraphs(arena, child);
    }
}

fn unwrap_paragraph_text<'a>(arena: &'a Arena<AstNode<'a>>, paragraph: &'a AstNode<'a>) {
    // Collect all text from the paragraph
    let mut full_text = String::new();
    collect_text(paragraph, &mut full_text);

    if full_text.is_empty() {
        return;
    }

    // Unwrap the text (join lines that aren't hard breaks)
    let unwrapped = unwrap_text(&full_text);

    // Skip if no change
    if unwrapped == full_text {
        return;
    }

    // Remove all existing children
    while let Some(child) = paragraph.first_child() {
        child.detach();
    }

    // Create new text node with unwrapped content
    let text_node = arena.alloc(AstNode::new(std::cell::RefCell::new(
        comrak::nodes::Ast::new(NodeValue::Text(unwrapped), paragraph.data.borrow().sourcepos.start),
    )));

    paragraph.append(text_node);
}

fn collect_text<'a>(node: &'a AstNode<'a>, output: &mut String) {
    match &node.data.borrow().value {
        NodeValue::Text(text) => {
            output.push_str(text);
        }
        NodeValue::SoftBreak => {
            output.push('\n');
        }
        NodeValue::LineBreak => {
            // Hard breaks should be preserved - mark them specially
            output.push_str("  \n");
        }
        _ => {
            // Recursively collect from children
            for child in node.children() {
                collect_text(child, output);
            }
        }
    }
}

fn unwrap_text(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut result = String::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_end();

        // Check if this line ends with a hard break (2+ spaces or backslash)
        let is_hard_break = trimmed.ends_with("  ")
            || trimmed.ends_with('\\')
            || line.ends_with("  ");

        result.push_str(trimmed);

        if is_hard_break {
            // Preserve hard break
            result.push_str("  \n");
        } else if i < lines.len() - 1 {
            // Join with next line with a space
            if !trimmed.is_empty() {
                result.push(' ');
            }
        }
    }

    result
}

// ===== WRAP FUNCTIONALITY =====

fn wrap_markdown(content: &str, width: usize) -> anyhow::Result<String> {
    let arena = Arena::new();
    let options = ComrakOptions::default();

    let root = parse_document(&arena, content, &options);

    // Walk the AST and wrap paragraphs
    wrap_paragraphs(&arena, root, width);

    // Format back to markdown
    let mut output = Vec::new();
    comrak::format_commonmark(root, &options, &mut output)?;

    Ok(String::from_utf8(output)?)
}

fn wrap_paragraphs<'a>(arena: &'a Arena<AstNode<'a>>, node: &'a AstNode<'a>, width: usize) {
    // Process current node
    if let NodeValue::Paragraph = node.data.borrow().value {
        wrap_paragraph_text(arena, node, width);
    }

    // Recursively process children
    for child in node.children() {
        wrap_paragraphs(arena, child, width);
    }
}

fn wrap_paragraph_text<'a>(arena: &'a Arena<AstNode<'a>>, paragraph: &'a AstNode<'a>, width: usize) {
    // Collect all text from the paragraph
    let mut full_text = String::new();
    collect_text(paragraph, &mut full_text);

    if full_text.is_empty() {
        return;
    }

    // Wrap the text at specified width
    let wrapped = wrap_text(&full_text, width);

    // Skip if no change
    if wrapped == full_text {
        return;
    }

    // Remove all existing children
    while let Some(child) = paragraph.first_child() {
        child.detach();
    }

    // Split wrapped text by newlines and create proper nodes
    // We need to handle both soft breaks (\n) and hard breaks (  \n)
    let sourcepos = paragraph.data.borrow().sourcepos.start;

    let parts: Vec<&str> = wrapped.split("  \n").collect();

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        // Split by regular newlines (soft breaks)
        let lines: Vec<&str> = part.split('\n').collect();

        for (j, line) in lines.iter().enumerate() {
            if !line.is_empty() {
                let text_node = arena.alloc(AstNode::new(std::cell::RefCell::new(
                    comrak::nodes::Ast::new(NodeValue::Text(line.to_string()), sourcepos),
                )));
                paragraph.append(text_node);
            }

            // Add soft break between lines (except after last line)
            if j < lines.len() - 1 {
                let softbreak_node = arena.alloc(AstNode::new(std::cell::RefCell::new(
                    comrak::nodes::Ast::new(NodeValue::SoftBreak, sourcepos),
                )));
                paragraph.append(softbreak_node);
            }
        }

        // Add hard break between parts (except after last part)
        if i < parts.len() - 1 {
            let hardbreak_node = arena.alloc(AstNode::new(std::cell::RefCell::new(
                comrak::nodes::Ast::new(NodeValue::LineBreak, sourcepos),
            )));
            paragraph.append(hardbreak_node);
        }
    }
}

fn wrap_text(text: &str, width: usize) -> String {
    let mut result = String::new();

    // Split by hard breaks first (preserve them)
    let segments: Vec<&str> = text.split("  \n").collect();

    for (seg_idx, segment) in segments.iter().enumerate() {
        if segment.is_empty() {
            continue;
        }

        // Wrap this segment
        let words: Vec<&str> = segment.split_whitespace().collect();
        let mut current_line = String::new();
        let mut current_length = 0;

        for word in words {
            let word_len = word.len();

            // If adding this word would exceed width
            if current_length + word_len + (if current_length > 0 { 1 } else { 0 }) > width && current_length > 0 {
                // Start new line
                result.push_str(&current_line);
                result.push('\n');
                current_line.clear();
                current_length = 0;
            }

            // Add word to current line
            if current_length > 0 {
                current_line.push(' ');
                current_length += 1;
            }
            current_line.push_str(word);
            current_length += word_len;
        }

        // Add remaining text
        if !current_line.is_empty() {
            result.push_str(&current_line);
        }

        // Preserve hard break if not last segment
        if seg_idx < segments.len() - 1 {
            result.push_str("  \n");
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    // ===== UNWRAP TESTS =====

    #[test]
    fn test_unwrap_basic_hard_wrapped_paragraph() {
        let input = indoc! {"
            This is a paragraph that has been
            hard-wrapped at 40 characters for
            some reason, like in an old email
            client.
        "};

        let expected = indoc! {"
            This is a paragraph that has been hard-wrapped at 40 characters for some reason, like in an old email client.
        "};

        let result = unwrap_markdown(input).unwrap();
        assert_eq!(result.trim(), expected.trim());
    }

    #[test]
    fn test_unwrap_preserve_hard_breaks() {
        // Note: Must use raw string to preserve trailing spaces
        let input = "This paragraph has an intentional  \nhard break in the middle that should  \nbe preserved.\n";

        let result = unwrap_markdown(input).unwrap();
        // Hard breaks should be preserved as either trailing spaces or backslash
        assert!(result.contains("intentional  ") || result.contains("intentional\\"));
        assert!(result.contains("should  ") || result.contains("should\\"));
    }

    #[test]
    fn test_unwrap_code_blocks_unchanged() {
        let input = indoc! {"
            ```rust
            // Code block should
            // remain unchanged
            fn main() {
                println!(\"hello\");
            }
            ```
        "};

        let expected = indoc! {"
            ``` rust
            // Code block should
            // remain unchanged
            fn main() {
                println!(\"hello\");
            }
            ```
        "};

        let result = unwrap_markdown(input).unwrap();
        assert_eq!(result.trim(), expected.trim());
    }

    #[test]
    fn test_unwrap_headings_unchanged() {
        let input = indoc! {"
            # A Heading

            ## Another Heading
        "};

        let expected = indoc! {"
            # A Heading

            ## Another Heading
        "};

        let result = unwrap_markdown(input).unwrap();
        assert_eq!(result.trim(), expected.trim());
    }

    #[test]
    fn test_unwrap_lists_unwrap_items() {
        let input = indoc! {"
            - List item one
              that wraps
            - List item two
              also wrapping
        "};

        let expected = indoc! {"
            - List item one that wraps
            - List item two also wrapping
        "};

        let result = unwrap_markdown(input).unwrap();
        assert_eq!(result.trim(), expected.trim());
    }

    #[test]
    fn test_unwrap_block_quotes() {
        let input = indoc! {"
            > Quote that has
            > been wrapped across
            > multiple lines
        "};

        let expected = indoc! {"
            > Quote that has been wrapped across multiple lines
        "};

        let result = unwrap_markdown(input).unwrap();
        assert_eq!(result.trim(), expected.trim());
    }

    #[test]
    fn test_unwrap_multiple_paragraphs() {
        let input = indoc! {"
            First paragraph
            with wrapping.

            Second paragraph
            also wrapped.
        "};

        let expected = indoc! {"
            First paragraph with wrapping.

            Second paragraph also wrapped.
        "};

        let result = unwrap_markdown(input).unwrap();
        assert_eq!(result.trim(), expected.trim());
    }

    #[test]
    fn test_unwrap_mixed_content() {
        let input = indoc! {"
            This is a paragraph
            that wraps.

            # Heading

            Another paragraph
            spanning lines.

            ```
            code block
            unchanged
            ```
        "};

        let expected = indoc! {"
            This is a paragraph that wraps.

            # Heading

            Another paragraph spanning lines.

                code block
                unchanged
        "};

        let result = unwrap_markdown(input).unwrap();
        assert_eq!(result.trim(), expected.trim());
    }

    #[test]
    fn test_unwrap_empty_input() {
        let input = "";
        let result = unwrap_markdown(input).unwrap();
        assert_eq!(result.trim(), "");
    }

    #[test]
    fn test_unwrap_already_unwrapped() {
        let input = indoc! {"
            This paragraph is already on a single line.
        "};

        let result = unwrap_markdown(input).unwrap();
        assert_eq!(result.trim(), input.trim());
    }

    // ===== WRAP TESTS =====

    #[test]
    fn test_wrap_basic_paragraph() {
        let input = "This is a very long paragraph that should be wrapped at eighty characters to make it more readable in a terminal or text editor.";

        let result = wrap_markdown(input, 80).unwrap();

        // Check that lines don't exceed 80 characters
        for line in result.lines() {
            assert!(line.trim().len() <= 80, "Line too long: {}", line);
        }

        // Check that it was actually wrapped
        assert!(result.contains('\n'));
    }

    #[test]
    fn test_wrap_custom_width() {
        let input = "This is a paragraph that should be wrapped at forty characters for testing purposes.";

        let result = wrap_markdown(input, 40).unwrap();

        // Check that lines don't exceed 40 characters
        for line in result.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                assert!(trimmed.len() <= 40, "Line too long: {} ({})", line, trimmed.len());
            }
        }
    }

    #[test]
    fn test_wrap_preserve_hard_breaks() {
        let input = "This has a hard break here  \nand should be preserved  \nwhen wrapping.";

        let result = wrap_markdown(input, 80).unwrap();

        // Hard breaks should be preserved
        assert!(result.contains("here  ") || result.contains("here\\"));
        assert!(result.contains("preserved  ") || result.contains("preserved\\"));
    }

    #[test]
    fn test_wrap_long_words() {
        let input = "This paragraph has a verylongwordthatexceedsthewidthlimitbutshouldbeonitsownline without breaking.";

        let result = wrap_markdown(input, 40).unwrap();

        // The long word should appear intact somewhere in the output
        assert!(result.contains("verylongwordthatexceedsthewidthlimitbutshouldbeonitsownline"));
    }

    #[test]
    fn test_wrap_code_blocks_unchanged() {
        let input = indoc! {"
            ```rust
            fn main() { println!(\"This is a very long line in a code block that should not be wrapped at all\"); }
            ```
        "};

        let result = wrap_markdown(input, 40).unwrap();

        // Code block content should remain unchanged
        assert!(result.contains("This is a very long line in a code block that should not be wrapped at all"));
    }

    #[test]
    fn test_wrap_headings_unchanged() {
        let input = "# This is a very long heading that should not be wrapped even if it exceeds the column width";

        let result = wrap_markdown(input, 40).unwrap();

        // Heading should remain on one line
        assert!(result.contains("This is a very long heading that should not be wrapped even if it exceeds the column width"));
    }

    #[test]
    fn test_wrap_already_wrapped() {
        let input = indoc! {"
            This is a short
            paragraph that
            fits within
            the width.
        "};

        // First unwrap it
        let unwrapped = unwrap_markdown(input).unwrap();

        // Then wrap it
        let result = wrap_markdown(&unwrapped, 80).unwrap();

        // Should produce a reasonable result
        assert!(!result.is_empty());
    }
}
