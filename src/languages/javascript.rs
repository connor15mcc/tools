use crate::languages::LanguageSupport;
use tree_sitter::Language;

pub struct JavaScriptSupport;

#[linkme::distributed_slice(crate::languages::LANGUAGES)]
static JAVASCRIPT: &dyn LanguageSupport = &JavaScriptSupport;

impl LanguageSupport for JavaScriptSupport {
    fn name(&self) -> &str {
        "javascript"
    }

    fn extensions(&self) -> &[&str] {
        &["js", "mjs", "cjs"]
    }

    fn treesitter_language(&self) -> Language {
        tree_sitter_javascript::language()
    }

    fn is_replaceable_node(&self, node: &tree_sitter::Node) -> bool {
        matches!(node.kind(), "string" | "number" | "array")
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

        let replacement = if (node_text.starts_with('"') && node_text.ends_with('"'))
            || (node_text.starts_with('\'') && node_text.ends_with('\''))
        {
            // string
            let trimmed = new_content.trim();
            if (trimmed.starts_with('"') && trimmed.ends_with('"'))
                || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
            {
                trimmed.to_string()
            } else {
                format!("\"{}\"", trimmed.replace('"', "\\\"").replace('\'', "\\'"))
            }
        } else if node_text.chars().all(|c| c.is_ascii_digit() || c == '.') {
            // number
            new_content.trim().to_string()
        } else if node_text.starts_with('[') && node_text.ends_with(']') {
            // array
            if new_content.contains('\n') {
                let items: Vec<String> = new_content
                    .lines()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                format!("[{}]", items.join(", "))
            } else {
                format!("[{}]", new_content.trim())
            }
        } else {
            new_content.to_string()
        };

        format!("{}{}{}", prefix, replacement, suffix)
    }
}
