//! Interpretación de las respuestas de confirmación por voz: sí/no por
//! palabras clave normalizadas (sin LLM — latencia cero y determinista) y el
//! código de aceptación de riesgos hablado en dígitos o en palabras.

use crate::config::AgentConfig;

#[derive(Debug, PartialEq, Eq)]
pub enum ConfirmDecision {
    Yes,
    No,
    /// La frase no parece una respuesta a la pregunta: se cancela la acción
    /// y se procesa como una petición nueva.
    Unrelated,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CodeDecision {
    Correct,
    /// Dijo algo con dígitos pero no coincide: un solo intento, se cancela.
    Wrong,
    /// Pidió cancelar explícitamente.
    Cancelled,
    /// Sin dígitos y frase larga: parece una petición nueva.
    Unrelated,
}

/// Minúsculas, sin tildes, solo letras/dígitos/espacios — "¡Sí, señor!" →
/// "si señor". A diferencia del normalizador del wake word, conserva dígitos.
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

fn matches_any(normalized: &str, phrases: &[String]) -> bool {
    let words: Vec<&str> = normalized.split(' ').collect();
    phrases.iter().any(|phrase| {
        let phrase = normalize(phrase);
        if phrase.is_empty() {
            return false;
        }
        // Frase completa exacta, o palabra suelta presente como token.
        normalized == phrase || (!phrase.contains(' ') && words.contains(&phrase.as_str()))
    })
}

/// Interpreta la respuesta a un "¿Confirma, señor?". El "no" tiene prioridad
/// sobre el "sí" ("no, no lo hagas" contiene tokens de ambos tipos en listas
/// laxas). Frases largas sin match se tratan como una petición nueva.
pub fn interpret(text: &str, cfg: &AgentConfig) -> ConfirmDecision {
    let normalized = normalize(text);
    if normalized.is_empty() {
        return ConfirmDecision::Unrelated;
    }
    let word_count = normalized.split(' ').count();
    if matches_any(&normalized, &cfg.confirm_no) {
        return ConfirmDecision::No;
    }
    if word_count <= 4 && matches_any(&normalized, &cfg.confirm_yes) {
        return ConfirmDecision::Yes;
    }
    ConfirmDecision::Unrelated
}

/// Dígito hablado en español → carácter. Whisper suele transcribir códigos
/// como dígitos ("0201", "02 01") pero a veces como palabras ("cero dos
/// cero uno"); se aceptan ambas formas mezcladas.
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
        // Otras palabras ("el código es...") se ignoran.
    }
    digits
}

/// Interpreta la respuesta a la petición del código de aceptación de riesgos.
/// La comparación ocurre acá, en Rust — el código jamás pasa por el LLM.
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
        // Sin dígitos: frase larga = petición nueva; corta = respuesta
        // fallida al desafío (p.ej. "sí" no basta en nivel Code).
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
        AgentConfig::default() // risk_code = "0201"
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
    fn codigo_en_digitos() {
        assert_eq!(interpret_code("0201", &cfg()), CodeDecision::Correct);
        assert_eq!(interpret_code("02 01", &cfg()), CodeDecision::Correct);
        assert_eq!(interpret_code("el código es 0201", &cfg()), CodeDecision::Correct);
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
        assert_eq!(interpret_code("no, cancela", &cfg()), CodeDecision::Cancelled);
    }

    #[test]
    fn codigo_unrelated() {
        assert_eq!(
            interpret_code("mejor cuéntame un chiste sobre gatos por favor", &cfg()),
            CodeDecision::Unrelated
        );
        // Un "sí" a secas no basta en nivel Code.
        assert_eq!(interpret_code("sí", &cfg()), CodeDecision::Wrong);
    }
}
