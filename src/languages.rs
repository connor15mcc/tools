use tree_sitter::Language;

pub trait LanguageSupport: Send + Sync {
    fn name(&self) -> &str;
    fn extensions(&self) -> &[&str];
    fn treesitter_language(&self) -> Language;
    fn is_replaceable_node(&self, node: &tree_sitter::Node) -> bool;
    fn replace_node_content(
        &self,
        source: &str,
        start: usize,
        end: usize,
        new_content: &str,
    ) -> String;
}

#[linkme::distributed_slice]
pub static LANGUAGES: [&'static dyn LanguageSupport] = [..];

pub mod javascript;
pub mod python;
pub mod rust;
