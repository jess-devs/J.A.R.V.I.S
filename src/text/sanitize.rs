//! Limpia marcado de texto (markdown) que un LLM puede colar en la
//! respuesta. Esto es una conversación hablada, no texto escrito: aunque el
//! system prompt le pide al modelo no usar markdown, un modelo chico/local
//! no lo respeta siempre, así que este es un filtro defensivo que se aplica
//! justo antes de sintetizar cada frase, para que nunca se reproduzca
//! literalmente "asterisco asterisco" o símbolos sueltos.

use std::sync::LazyLock;

use regex::{Captures, Regex};

static CODE_FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"```[\s\S]*?```").unwrap());
static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\([^)]*\)").unwrap());
static BOLD_OR_UNDERLINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\*\*([^*]+)\*\*|__([^_]+)__").unwrap());
static INLINE_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`]+)`").unwrap());
static LEFTOVER_MARKUP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[*_`#]+").unwrap());

pub fn strip_markdown_for_speech(text: &str) -> String {
    let text = CODE_FENCE.replace_all(text, " ");
    let text = LINK.replace_all(&text, "$1");
    let text = BOLD_OR_UNDERLINE.replace_all(&text, |caps: &Captures| {
        caps.get(1)
            .or_else(|| caps.get(2))
            .map(|m| m.as_str())
            .unwrap_or("")
            .to_string()
    });
    let text = INLINE_CODE.replace_all(&text, "$1");

    // Encabezados ("# Título") y viñetas ("- item", "1. item") al inicio de
    // la frase: cada frase ya viene delimitada por el sentence splitter
    // (que corta justo en los saltos de línea), así que alcanza con mirar
    // el comienzo del string.
    let without_prefix = strip_list_or_heading_prefix(text.trim_start());

    let cleaned = LEFTOVER_MARKUP.replace_all(without_prefix, "");
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_list_or_heading_prefix(text: &str) -> &str {
    let after_hashes = text.trim_start_matches('#');
    if after_hashes.len() != text.len() {
        return after_hashes.trim_start();
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = text.strip_prefix(marker) {
            return rest;
        }
    }
    if let Some(pos) = text.find(". ") {
        if !text[..pos].is_empty() && text[..pos].chars().all(|c| c.is_ascii_digit()) {
            return &text[pos + 2..];
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quita_negrita() {
        assert_eq!(
            strip_markdown_for_speech("Esto es **muy** importante."),
            "Esto es muy importante."
        );
    }

    #[test]
    fn quita_encabezado() {
        assert_eq!(strip_markdown_for_speech("## Título"), "Título");
    }

    #[test]
    fn quita_vineta() {
        assert_eq!(strip_markdown_for_speech("- primer punto"), "primer punto");
    }

    #[test]
    fn quita_lista_numerada() {
        assert_eq!(strip_markdown_for_speech("1. primer punto"), "primer punto");
    }

    #[test]
    fn quita_codigo_inline_y_bloques() {
        assert_eq!(
            strip_markdown_for_speech("Usá `cargo run` para arrancarlo."),
            "Usá cargo run para arrancarlo."
        );
        assert_eq!(
            strip_markdown_for_speech("```rust\nfn main() {}\n``` listo"),
            "listo"
        );
    }

    #[test]
    fn quita_link_y_deja_texto() {
        assert_eq!(
            strip_markdown_for_speech("mirá [la doc](https://example.com)"),
            "mirá la doc"
        );
    }

    #[test]
    fn deja_texto_normal_intacto() {
        assert_eq!(
            strip_markdown_for_speech("Hola, ¿cómo estás hoy?"),
            "Hola, ¿cómo estás hoy?"
        );
    }
}
