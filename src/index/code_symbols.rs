use std::collections::HashSet;
use std::path::Path;

use tree_sitter::{Language, Node, Parser};

use crate::normalize_query;

use super::types::IndexedChunk;
use super::util::stable_hash;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CodeLanguage {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
}

impl CodeLanguage {
    fn detect(path: &Path) -> Option<Self> {
        match path.extension().and_then(|ext| ext.to_str())? {
            "rs" => Some(Self::Rust),
            "py" => Some(Self::Python),
            "js" | "mjs" | "cjs" | "jsx" => Some(Self::JavaScript),
            "ts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            _ => None,
        }
    }

    fn parser_language(self) -> Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
        }
    }
}

pub(crate) fn supports_code_symbol_indexing(path: &Path) -> bool {
    CodeLanguage::detect(path).is_some()
}

pub(crate) fn build_symbol_chunks(path: &Path, body: &str) -> Vec<IndexedChunk> {
    let Some(language) = CodeLanguage::detect(path) else {
        return Vec::new();
    };

    let mut parser = Parser::new();
    if parser.set_language(&language.parser_language()).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(body, None) else {
        return Vec::new();
    };

    let mut chunks = Vec::new();
    walk_node(language, tree.root_node(), body, path, &mut chunks);
    chunks
}

fn walk_node(
    language: CodeLanguage,
    node: Node<'_>,
    body: &str,
    path: &Path,
    chunks: &mut Vec<IndexedChunk>,
) {
    if let Some(chunk) = symbol_chunk_for_node(language, node, body, path, chunks.len()) {
        chunks.push(chunk);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_node(language, child, body, path, chunks);
    }
}

fn symbol_chunk_for_node(
    language: CodeLanguage,
    node: Node<'_>,
    body: &str,
    path: &Path,
    chunk_index: usize,
) -> Option<IndexedChunk> {
    let symbol_kind = symbol_kind(language, node)?;
    let name_node = symbol_name_node(language, node)?;
    let symbol_name = node_text(name_node, body)?.trim().to_string();
    if symbol_name.is_empty() {
        return None;
    }

    let signature_text = first_line(node_text(node, body)?);
    let doc_text = extract_doc_text(language, node, body);
    let rel_path = path.to_string_lossy().replace('\\', "/");
    let container = container_for_node(language, node, body);
    let identifier_terms = extract_identifier_terms(language, node, body);
    let identifier_line = if identifier_terms.is_empty() {
        String::new()
    } else {
        format!("\nidentifiers: {}", identifier_terms.join(" "))
    };

    let search_text = normalize_query(&format!(
        "name: {symbol_name}\nsignature: {signature_text}\ndoc: {doc_text}\ncontainer: {}\npath: {rel_path}{}",
        container.clone().unwrap_or_default(),
        identifier_line,
    ));
    if search_text.is_empty() {
        return None;
    }

    let shared_text = normalize_query(&identifier_terms.join(" "));
    if shared_text.is_empty() {
        return None;
    }

    let raw_text = format!("{symbol_kind} {symbol_name} in {rel_path}");

    Some(IndexedChunk {
        chunk_index,
        line_start: node.start_position().row + 1,
        line_end: node.end_position().row + 1,
        raw_text,
        normalized_text: search_text,
        shared_normalized_text: shared_text.clone(),
        shared_normalized_text_hash: stable_hash(shared_text.as_bytes()),
        chunk_kind: "symbol".to_string(),
        language: Some(language.as_str().to_string()),
        symbol_kind: Some(symbol_kind.to_string()),
        container,
    })
}

fn symbol_kind(language: CodeLanguage, node: Node<'_>) -> Option<&'static str> {
    match language {
        CodeLanguage::Rust => match node.kind() {
            "function_item" => Some("function"),
            "struct_item" => Some("struct"),
            "enum_item" => Some("enum"),
            "trait_item" => Some("trait"),
            "type_item" => Some("type"),
            "mod_item" => Some("module"),
            "const_item" | "static_item" => Some("constant"),
            _ => None,
        },
        CodeLanguage::Python => match node.kind() {
            "function_definition" => Some("function"),
            "class_definition" => Some("class"),
            "assignment" if is_python_module_assignment(node) => Some("constant"),
            _ => None,
        },
        CodeLanguage::JavaScript | CodeLanguage::TypeScript | CodeLanguage::Tsx => {
            match node.kind() {
                "function_declaration" => Some("function"),
                "function_signature" => Some("function"),
                "method_definition" | "method_signature" | "abstract_method_signature" => {
                    Some("method")
                }
                "class_declaration" | "class" | "abstract_class_declaration" => Some("class"),
                "interface_declaration" => Some("interface"),
                "module" => Some("module"),
                "variable_declarator" if is_js_function_binding(node) => Some("function"),
                "variable_declarator" if is_js_top_level_binding(node) => Some("constant"),
                _ => None,
            }
        }
    }
}

fn symbol_name_node(language: CodeLanguage, node: Node<'_>) -> Option<Node<'_>> {
    match language {
        CodeLanguage::Python if node.kind() == "assignment" => node.child_by_field_name("left"),
        CodeLanguage::Rust
        | CodeLanguage::Python
        | CodeLanguage::JavaScript
        | CodeLanguage::TypeScript
        | CodeLanguage::Tsx => node.child_by_field_name("name"),
    }
}

fn container_for_node(language: CodeLanguage, node: Node<'_>, body: &str) -> Option<String> {
    let mut parts = Vec::new();
    let mut cursor = node.parent();
    while let Some(parent) = cursor {
        if let Some(name) = container_name_for_node(language, parent, body) {
            parts.push(name);
        }
        cursor = parent.parent();
    }
    if parts.is_empty() {
        return None;
    }
    parts.reverse();
    Some(parts.join("::"))
}

fn container_name_for_node(language: CodeLanguage, node: Node<'_>, body: &str) -> Option<String> {
    match language {
        CodeLanguage::Rust => match node.kind() {
            "mod_item" | "trait_item" | "struct_item" | "enum_item" | "type_item" => node
                .child_by_field_name("name")
                .and_then(|name| node_text(name, body))
                .map(|name| name.trim().to_string()),
            "impl_item" => node
                .child_by_field_name("type")
                .and_then(|name| node_text(name, body))
                .map(|name| name.trim().to_string()),
            _ => None,
        },
        CodeLanguage::Python => match node.kind() {
            "class_definition" | "function_definition" => node
                .child_by_field_name("name")
                .and_then(|name| node_text(name, body))
                .map(|name| name.trim().to_string()),
            _ => None,
        },
        CodeLanguage::JavaScript | CodeLanguage::TypeScript | CodeLanguage::Tsx => {
            match node.kind() {
                "class_declaration"
                | "class"
                | "abstract_class_declaration"
                | "interface_declaration"
                | "function_declaration"
                | "function_signature"
                | "module" => node
                    .child_by_field_name("name")
                    .and_then(|name| node_text(name, body))
                    .map(|name| name.trim().to_string()),
                _ => None,
            }
        }
    }
}

fn extract_preceding_doc_text(body: &str, start_row: usize, language: CodeLanguage) -> String {
    let lines = body.lines().collect::<Vec<_>>();
    if start_row == 0 || start_row > lines.len() {
        return String::new();
    }

    let mut out = Vec::new();
    let mut row = start_row;
    while row > 0 {
        row -= 1;
        let line = lines[row].trim();
        if line.is_empty() {
            if out.is_empty() {
                continue;
            }
            break;
        }
        let Some(cleaned) = clean_comment_prefix(language, line) else {
            break;
        };
        out.push(cleaned.to_string());
    }
    out.reverse();
    out.join("\n").trim().to_string()
}

fn extract_doc_text(language: CodeLanguage, node: Node<'_>, body: &str) -> String {
    let leading = extract_preceding_doc_text(body, node.start_position().row, language);
    if !leading.is_empty() {
        return leading;
    }

    if language == CodeLanguage::Python {
        return extract_python_docstring(node, body).unwrap_or_default();
    }

    String::new()
}

fn clean_comment_prefix(language: CodeLanguage, line: &str) -> Option<&str> {
    match language {
        CodeLanguage::Python => line.strip_prefix('#').map(str::trim),
        CodeLanguage::Rust
        | CodeLanguage::JavaScript
        | CodeLanguage::TypeScript
        | CodeLanguage::Tsx => line
            .strip_prefix("///")
            .or_else(|| line.strip_prefix("//!"))
            .or_else(|| line.strip_prefix("//"))
            .or_else(|| line.strip_prefix("/**"))
            .or_else(|| line.strip_prefix("/*"))
            .or_else(|| line.strip_prefix('*'))
            .map(str::trim),
    }
}

fn extract_python_docstring(node: Node<'_>, body: &str) -> Option<String> {
    let body_node = node.child_by_field_name("body")?;
    let mut cursor = body_node.walk();
    let first = body_node
        .children(&mut cursor)
        .find(|child| child.is_named())?;
    if first.kind() != "expression_statement" {
        return None;
    }

    let mut expr_cursor = first.walk();
    let inner = first
        .children(&mut expr_cursor)
        .find(|child| child.is_named())?;
    if inner.kind() != "string" {
        return None;
    }

    let text = node_text(inner, body)?.trim();
    Some(text.trim_matches('"').trim_matches('\'').trim().to_string())
}

fn is_python_module_assignment(node: Node<'_>) -> bool {
    let Some(left) = node.child_by_field_name("left") else {
        return false;
    };
    if left.kind() != "identifier" {
        return false;
    }

    let Some(parent) = node.parent() else {
        return false;
    };
    parent.kind() == "expression_statement"
        && parent
            .parent()
            .is_some_and(|grand| grand.kind() == "module")
}

fn is_js_function_binding(node: Node<'_>) -> bool {
    if !is_js_top_level_binding(node) {
        return false;
    }
    let Some(name) = node.child_by_field_name("name") else {
        return false;
    };
    if name.kind() != "identifier" {
        return false;
    }
    node.child_by_field_name("value").is_some_and(|value| {
        matches!(
            value.kind(),
            "arrow_function" | "function_expression" | "generator_function"
        )
    })
}

fn is_js_top_level_binding(node: Node<'_>) -> bool {
    let Some(name) = node.child_by_field_name("name") else {
        return false;
    };
    if name.kind() != "identifier" {
        return false;
    }
    let Some(parent) = node.parent() else {
        return false;
    };
    let is_decl = matches!(
        parent.kind(),
        "lexical_declaration" | "variable_declaration"
    );
    if !is_decl {
        return false;
    }
    let Some(grand) = parent.parent() else {
        return false;
    };
    matches!(grand.kind(), "program" | "export_statement")
}

fn extract_identifier_terms(language: CodeLanguage, root: Node<'_>, body: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut seen = HashSet::new();
    collect_identifier_terms(language, root, root, body, &mut seen, &mut terms);
    terms
}

fn collect_identifier_terms(
    language: CodeLanguage,
    root: Node<'_>,
    node: Node<'_>,
    body: &str,
    seen: &mut HashSet<String>,
    terms: &mut Vec<String>,
) {
    if node.id() != root.id() && symbol_kind(language, node).is_some() {
        return;
    }

    if is_identifier_like(node.kind()) {
        if let Some(text) = node_text(node, body) {
            for term in expand_identifier_terms(text) {
                if seen.insert(term.clone()) {
                    terms.push(term);
                    if terms.len() >= 64 {
                        return;
                    }
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if terms.len() >= 64 {
            return;
        }
        collect_identifier_terms(language, root, child, body, seen, terms);
    }
}

fn is_identifier_like(kind: &str) -> bool {
    kind == "identifier"
        || kind == "type_identifier"
        || kind == "field_identifier"
        || kind == "property_identifier"
        || kind == "shorthand_property_identifier"
}

fn expand_identifier_terms(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let mut terms = Vec::new();
    let whole = trimmed.to_lowercase();
    terms.push(whole);

    let mut current = String::new();
    let mut prev_is_lower_or_digit = false;

    for ch in trimmed.chars() {
        if !ch.is_alphanumeric() {
            push_identifier_part(&mut terms, &mut current);
            prev_is_lower_or_digit = false;
            continue;
        }

        let is_upper = ch.is_uppercase();
        if is_upper && prev_is_lower_or_digit && !current.is_empty() {
            push_identifier_part(&mut terms, &mut current);
        }
        prev_is_lower_or_digit = ch.is_lowercase() || ch.is_ascii_digit();
        current.push(ch.to_ascii_lowercase());
    }
    push_identifier_part(&mut terms, &mut current);

    let mut deduped = Vec::new();
    let mut seen = HashSet::new();
    for term in terms {
        if !term.is_empty() && seen.insert(term.clone()) {
            deduped.push(term);
        }
    }
    deduped
}

fn push_identifier_part(terms: &mut Vec<String>, current: &mut String) {
    if current.is_empty() {
        return;
    }
    terms.push(std::mem::take(current));
}

fn node_text<'a>(node: Node<'_>, body: &'a str) -> Option<&'a str> {
    node.utf8_text(body.as_bytes()).ok()
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::build_symbol_chunks;
    use std::path::Path;

    #[test]
    fn rust_symbol_chunks_extract_function_and_struct() {
        let body = r#"
/// parse docs
pub fn parse_query(input: &str) -> String {
    let retry_backoff_ms = input.len();
    retry_backoff_ms.to_string()
}

pub struct Parser;
pub const DEFAULT_LIMIT: usize = 20;
"#;
        let chunks = build_symbol_chunks(Path::new("src/lib.rs"), body);
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.symbol_kind.as_deref() == Some("function"))
        );
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.symbol_kind.as_deref() == Some("struct"))
        );
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.symbol_kind.as_deref() == Some("constant"))
        );
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.normalized_text.contains("path: src/lib.rs"))
        );
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.shared_normalized_text.contains("retry backoff ms"))
        );
        assert!(
            chunks
                .iter()
                .all(|chunk| !chunk.shared_normalized_text.contains("path:"))
        );
        assert!(
            chunks
                .iter()
                .all(|chunk| !chunk.shared_normalized_text.contains("doc:"))
        );
    }

    #[test]
    fn python_symbol_chunks_extract_class_and_function() {
        let body = r#"
# alpha docs
DEFAULT_LIMIT = 20

class Parser:
    pass

def parse_query(input):
    return input
"#;
        let chunks = build_symbol_chunks(Path::new("parser.py"), body);
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.symbol_kind.as_deref() == Some("class"))
        );
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.symbol_kind.as_deref() == Some("function"))
        );
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.symbol_kind.as_deref() == Some("constant"))
        );
    }

    #[test]
    fn python_docstring_is_used_when_leading_comment_is_missing() {
        let body = r#"
def parse_query(input):
    """query parser docs"""
    return input
"#;
        let chunks = build_symbol_chunks(Path::new("parser.py"), body);
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.normalized_text.contains("query parser docs"))
        );
    }

    #[test]
    fn javascript_symbol_chunks_extract_class_function_and_bindings() {
        let body = r#"
// docs
class Parser {}

function parseQuery(input) { return input; }
const makeParser = (input) => input;
const DEFAULT_LIMIT = 20;
"#;
        let chunks = build_symbol_chunks(Path::new("parser.js"), body);
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.symbol_kind.as_deref() == Some("class"))
        );
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.symbol_kind.as_deref() == Some("function"))
        );
        assert!(
            chunks
                .iter()
                .filter(|chunk| chunk.symbol_kind.as_deref() == Some("function"))
                .count()
                >= 2
        );
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.symbol_kind.as_deref() == Some("constant"))
        );
    }

    #[test]
    fn typescript_symbol_chunks_extract_bindings() {
        let body = r#"
interface ParserOptions {}
const makeParser = (input: string) => input;
const DEFAULT_LIMIT = 20;
"#;
        let chunks = build_symbol_chunks(Path::new("parser.ts"), body);
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.symbol_kind.as_deref() == Some("interface"))
        );
        assert!(
            chunks
                .iter()
                .filter(|chunk| chunk.symbol_kind.as_deref() == Some("function"))
                .count()
                >= 1
        );
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.symbol_kind.as_deref() == Some("constant"))
        );
    }
}
