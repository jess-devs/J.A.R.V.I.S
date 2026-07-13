//! Trocea el texto que va llegando en fragmentos (tokens del LLM) en frases
//! completas, listas para mandar a sintetizar. Corta en `. ! ? \n` seguido
//! de espacio/fin de buffer, evitando cortar decimales ("3.14") y algunas
//! abreviaturas comunes en español. Si el buffer crece demasiado sin
//! encontrar una frontera, corta en el último espacio antes del límite
//! (fallback anti texto corrido).

const ABBREVIATIONS: &[&str] = &[
    "sr.", "sra.", "srta.", "dr.", "dra.", "ud.", "uds.", "etc.", "p.ej.", "ej.", "vs.", "no.",
];

pub struct SentenceChunker {
    buffer: String,
    max_len: usize,
    min_len: usize,
}

impl SentenceChunker {
    pub fn new(max_len: usize, min_len: usize) -> Self {
        Self {
            buffer: String::new(),
            max_len,
            min_len,
        }
    }

    /// Agrega un fragmento de texto y devuelve 0 o más frases completas que
    /// quedaron listas para sintetizar.
    pub fn push(&mut self, token: &str) -> Vec<String> {
        self.buffer.push_str(token);
        let mut phrases = Vec::new();
        let mut search_from = 0usize;

        while let Some(boundary) = self.find_boundary_from(search_from) {
            let candidate_len = self.buffer[..boundary].trim().len();
            if candidate_len < self.min_len {
                search_from = boundary;
                continue;
            }
            let phrase: String = self.buffer.drain(..boundary).collect();
            let phrase = phrase.trim().to_string();
            if !phrase.is_empty() {
                phrases.push(phrase);
            }
            search_from = 0;
        }

        while self.buffer.len() > self.max_len {
            let mut boundary = self.max_len;
            while !self.buffer.is_char_boundary(boundary) {
                boundary -= 1;
            }
            let cut = self.buffer[..boundary]
                .rfind(' ')
                .map(|p| p + 1)
                .unwrap_or(boundary);
            if cut == 0 {
                break;
            }
            let phrase: String = self.buffer.drain(..cut).collect();
            let phrase = phrase.trim().to_string();
            if !phrase.is_empty() {
                phrases.push(phrase);
            }
        }

        phrases
    }

    /// Vacía cualquier texto pendiente al terminar el stream del LLM.
    pub fn finish(&mut self) -> Option<String> {
        let phrase = self.buffer.trim().to_string();
        self.buffer.clear();
        if phrase.is_empty() {
            None
        } else {
            Some(phrase)
        }
    }

    fn find_boundary_from(&self, from: usize) -> Option<usize> {
        let slice = self.buffer.get(from..)?;
        for (offset, ch) in slice.char_indices() {
            if !matches!(ch, '.' | '!' | '?' | '\n') {
                continue;
            }
            let i = from + offset;
            let boundary_end = i + ch.len_utf8();
            let next = self.buffer[boundary_end..].chars().next();

            let followed_by_space_or_end = match next {
                None => false,
                Some(c) => c.is_whitespace(),
            };
            if !followed_by_space_or_end {
                continue;
            }

            if ch == '.' {
                let prev_is_digit = self.buffer[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_digit());
                if prev_is_digit {
                    let next_after_space = self.buffer[boundary_end..].trim_start().chars().next();
                    if next_after_space.is_some_and(|c| c.is_ascii_digit()) {
                        continue;
                    }
                }

                if ends_with_abbreviation(&self.buffer[..boundary_end]) {
                    continue;
                }
            }

            return Some(boundary_end);
        }
        None
    }
}

/// Compara la última *palabra* (no un sufijo cualquiera) contra la lista de
/// abreviaturas. Comparar por sufijo haría que, por ejemplo, "no." (de la
/// lista) calzara con cualquier palabra terminada en "-no.", como "temprano.".
fn ends_with_abbreviation(text: &str) -> bool {
    let last_word = text
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or(text)
        .to_lowercase();
    ABBREVIATIONS.contains(&last_word.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corta_en_punto_seguido_de_espacio() {
        let mut chunker = SentenceChunker::new(220, 5);
        let phrases = chunker.push("Hola mundo. ¿Cómo estás?");
        assert_eq!(phrases, vec!["Hola mundo.".to_string()]);
        assert_eq!(chunker.finish(), Some("¿Cómo estás?".to_string()));
    }

    #[test]
    fn no_corta_decimales() {
        let mut chunker = SentenceChunker::new(220, 3);
        let phrases = chunker.push("El valor es 3.14 aproximadamente. Listo.");
        assert_eq!(
            phrases,
            vec!["El valor es 3.14 aproximadamente.".to_string()]
        );
    }

    #[test]
    fn no_corta_abreviaturas() {
        let mut chunker = SentenceChunker::new(220, 3);
        let phrases = chunker.push("El Dr. García llegó temprano. Saludó a todos.");
        assert_eq!(phrases, vec!["El Dr. García llegó temprano.".to_string()]);
    }

    #[test]
    fn fallback_por_longitud_maxima() {
        let mut chunker = SentenceChunker::new(20, 3);
        let phrases = chunker.push("una oracion muy larga sin puntuacion alguna que sigue y sigue");
        assert!(!phrases.is_empty());
        assert!(phrases.iter().all(|p| p.len() <= 21));
    }

    #[test]
    fn fallback_no_panickea_en_frontera_multibyte() {
        let mut chunker = SentenceChunker::new(20, 3);
        let texto = "una oracion larga｜sin puntuacion que sigue y sigue";
        let phrases = chunker.push(texto);
        assert!(!phrases.is_empty());
        assert!(phrases.iter().all(|p| p.is_char_boundary(0)));
    }

    #[test]
    fn junta_frases_muy_cortas() {
        // "Sí." y "Sí. No." son más cortas que min_len: se van juntando en
        // el buffer hasta encontrar una frontera suficientemente larga. Como
        // la última oración no tiene un espacio después del punto final
        // (todavía no llegó más texto del LLM), push() no la emite todavía
        // — recién sale completa al hacer finish().
        let mut chunker = SentenceChunker::new(220, 10);
        let phrases = chunker.push("Sí. No. Bueno, entendido.");
        assert!(phrases.is_empty());
        assert_eq!(
            chunker.finish(),
            Some("Sí. No. Bueno, entendido.".to_string())
        );
    }
}
