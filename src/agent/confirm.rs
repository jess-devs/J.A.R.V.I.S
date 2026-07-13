//! Interpretación de las respuestas de confirmación por voz: sí/no por voz
//! palabras clave normalizadas (sin LLM — menor latencia)

use crate::config::AgentConfig;

#[derive(Debug, PartialEq, Eq)]
pub enum ConfirmDecision {
    Yes,
    No,
    Unrelated,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CodeDecision {
    Correct,
    Wrong,
    Cancelled,
    Unrelated,
}

fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.to_lowercase().chars() {
        match c {
            'á' => out.push('a'),
            'é' => out.push('e'),
            'í' => out.push('i'),
            'ó' => out.push('o'),
            'ú' | 'ü' => out.push('u'),
            c if c.is_alphanumeric() => out.push(c),
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Busca `phrase` (posiblemente varias palabras) como bloque contiguo en
/// cualquier posición de `words`, no solo si ocupa la respuesta entera.
/// Devuelve cuántas palabras ocupa el match, para que el llamador pueda
/// medir cuánto texto "de sobra" quedó fuera de la frase reconocida.
fn phrase_match_len(words: &[&str], phrase: &str) -> Option<usize> {
    let phrase = normalize(phrase);
    if phrase.is_empty() {
        return None;
    }
    let phrase_words: Vec<&str> = phrase.split(' ').collect();
    if phrase_words.len() > words.len() {
        return None;
    }
    words
        .windows(phrase_words.len())
        .any(|w| w == phrase_words.as_slice())
        .then_some(phrase_words.len())
}

/// Longitud (en palabras) de la frase más larga de `phrases` que aparece en
/// `normalized`, si alguna aparece.
fn longest_match_len(normalized: &str, phrases: &[String]) -> Option<usize> {
    let words: Vec<&str> = normalized.split(' ').collect();
    phrases
        .iter()
        .filter_map(|phrase| phrase_match_len(&words, phrase))
        .max()
}

fn matches_any(normalized: &str, phrases: &[String]) -> bool {
    longest_match_len(normalized, phrases).is_some()
}

/// Palabras de sobra toleradas fuera de la frase de "sí" reconocida, para
/// aceptar confirmaciones naturales ("sí, ciérralo ya, jarvis") sin aceptar
/// una frase larga y ajena que de casualidad contiene un "sí" suelto.
const MAX_EXTRA_WORDS_FOR_YES: usize = 4;

/// Interpreta la respuesta a un "¿Confirma, señor?". El "no" tiene prioridad sobre el "sí"
pub fn interpret(text: &str, cfg: &AgentConfig) -> ConfirmDecision {
    let normalized = normalize(text);
    if normalized.is_empty() {
        return ConfirmDecision::Unrelated;
    }
    if matches_any(&normalized, &cfg.confirm_no) {
        return ConfirmDecision::No;
    }
    if let Some(matched_len) = longest_match_len(&normalized, &cfg.confirm_yes) {
        let word_count = normalized.split(' ').count();
        if word_count - matched_len <= MAX_EXTRA_WORDS_FOR_YES {
            return ConfirmDecision::Yes;
        }
    }
    ConfirmDecision::Unrelated
}

/// Whisper suele transcribir códigos
/// como dígitos ("0201", "02 01") pero a veces como palabras ("cero dos
/// cero uno") se aceptan ambas formas mezcladas.
fn word_to_digit(word: &str) -> Option<char> {
    Some(match word {
        "cero" => '0',
        "uno" | "una" => '1',
        "dos" => '2',
        "tres" => '3',
        "cuatro" => '4',
        "cinco" => '5',
        "seis" => '6',
        "siete" => '7',
        "ocho" => '8',
        "nueve" => '9',
        _ => return None,
    })
}

fn extract_digits(normalized: &str) -> String {
    let mut digits = String::new();
    for token in normalized.split(' ') {
        if token.chars().all(|c| c.is_ascii_digit()) {
            digits.push_str(token);
        } else if let Some(d) = word_to_digit(token) {
            digits.push(d);
        }
    }
    digits
}

/// Interpreta la respuesta a la petición del código de aceptación de riesgos.
pub fn interpret_code(text: &str, cfg: &AgentConfig) -> CodeDecision {
    let normalized = normalize(text);
    if normalized.is_empty() {
        return CodeDecision::Unrelated;
    }
    if matches_any(&normalized, &cfg.confirm_no) {
        return CodeDecision::Cancelled;
    }
    let digits = extract_digits(&normalized);
    if digits.is_empty() {
        return if normalized.split(' ').count() > 4 {
            CodeDecision::Unrelated
        } else {
            CodeDecision::Wrong
        };
    }
    if digits == cfg.risk_code {
        CodeDecision::Correct
    } else {
        CodeDecision::Wrong
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentConfig;

    fn cfg() -> AgentConfig {
        AgentConfig::default() // risk_code = "0201" <-- ejemplo
    }

    #[test]
    fn si_y_variantes() {
        for frase in ["sí", "Sí, señor", "claro", "adelante", "hazlo", "dale"] {
            assert_eq!(interpret(frase, &cfg()), ConfirmDecision::Yes, "{frase}");
        }
    }

    #[test]
    fn no_y_variantes() {
        for frase in ["no", "No, cancela", "mejor no", "espera"] {
            assert_eq!(interpret(frase, &cfg()), ConfirmDecision::No, "{frase}");
        }
    }

    #[test]
    fn no_gana_sobre_si() {
        assert_eq!(interpret("no, no lo hagas", &cfg()), ConfirmDecision::No);
    }

    #[test]
    fn frase_larga_es_unrelated() {
        assert_eq!(
            interpret("mejor dime qué hora es en este momento", &cfg()),
            ConfirmDecision::Unrelated
        );
    }

    #[test]
    fn confirmacion_natural_con_palabras_de_sobra() {
        for frase in [
            "sí, ciérralo ya, jarvis",
            "sí, adelante, hazlo ya",
            "dale, ciérralo nomás",
        ] {
            assert_eq!(interpret(frase, &cfg()), ConfirmDecision::Yes, "{frase}");
        }
    }

    #[test]
    fn negacion_natural_con_palabras_de_sobra() {
        assert_eq!(
            interpret("no, mejor cancelalo todo", &cfg()),
            ConfirmDecision::No
        );
    }

    #[test]
    fn frase_larga_ajena_con_si_suelto_sigue_siendo_unrelated() {
        assert_eq!(
            interpret("sí, pero antes decime qué hora es", &cfg()),
            ConfirmDecision::Unrelated
        );
    }

    #[test]
    fn frase_multipalabra_matchea_con_texto_alrededor() {
        // "sí señor" está en confirm_yes como frase de dos palabras; debe
        // matchear aunque no sea la respuesta completa.
        assert_eq!(
            interpret("sí señor, adelante", &cfg()),
            ConfirmDecision::Yes
        );
    }

    #[test]
    fn codigo_en_digitos() {
        assert_eq!(interpret_code("0201", &cfg()), CodeDecision::Correct);
        assert_eq!(interpret_code("02 01", &cfg()), CodeDecision::Correct);
        assert_eq!(
            interpret_code("el código es 0201", &cfg()),
            CodeDecision::Correct
        );
    }

    #[test]
    fn codigo_en_palabras() {
        assert_eq!(
            interpret_code("cero dos cero uno", &cfg()),
            CodeDecision::Correct
        );
    }

    #[test]
    fn codigo_incorrecto() {
        assert_eq!(interpret_code("1234", &cfg()), CodeDecision::Wrong);
        assert_eq!(interpret_code("cero dos", &cfg()), CodeDecision::Wrong);
    }

    #[test]
    fn codigo_cancelado() {
        assert_eq!(
            interpret_code("no, cancela", &cfg()),
            CodeDecision::Cancelled
        );
    }

    #[test]
    fn codigo_unrelated() {
        assert_eq!(
            interpret_code("mejor cuéntame un chiste sobre gatos por favor", &cfg()),
            CodeDecision::Unrelated
        );
        assert_eq!(interpret_code("sí", &cfg()), CodeDecision::Wrong);
    }
}
