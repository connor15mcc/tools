use crate::languages::LanguageSupport;
use tree_sitter::Language;

pub struct RustSupport;

#[linkme::distributed_slice(crate::languages::LANGUAGES)]
static RUST: &dyn LanguageSupport = &RustSupport;

impl LanguageSupport for RustSupport {
    fn name(&self) -> &str {
        "rust"
    }

    fn extensions(&self) -> &[&str] {
        &["rs"]
    }

    fn treesitter_language(&self) -> Language {
        tree_sitter_rust::language()
    }

    fn is_replaceable_node(&self, node: &tree_sitter::Node) -> bool {
        matches!(
            node.kind(),
            "string_literal" | "integer_literal" | "enum_variant_list" | "array_expression"
        )
    }

    fn replace_node_content(
        &self,
        source: &str,
        start: usize,
        end: usize,
        new_content: &str,
    ) -> String {
        let prefix = &source[..start];
        let suffix = &source[end..];
        let node_text = &source[start..end];

        let replacement = if node_text.starts_with('"') && node_text.ends_with('"') {
            // string_literal
            if new_content.starts_with('"') && new_content.ends_with('"') {
                new_content.to_string()
            } else {
                format!("\"{}\"", new_content.replace('"', "\\\""))
            }
        } else if node_text.chars().all(|c| c.is_ascii_digit()) {
            // integer_literal
            new_content.trim().to_string()
        } else if node_text.starts_with('{') && node_text.ends_with('}') {
            // enum_variant_list
            let variants: Vec<String> = new_content
                .lines()
                .map(|line| line.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            format!("{{\n{}\n}}", variants.join(",\n"))
        } else if node_text.starts_with('[') && node_text.ends_with(']') {
            // array_expression
            format!("[{}]", new_content.trim())
        } else {
            new_content.to_string()
        };

        format!("{}{}{}", prefix, replacement, suffix)
    }
}
