//! Interpretación de las respuestas de confirmación por voz: sí/no por voz
//! palabras clave normalizadas (sin LLM — menor latencia)

use crate::config::AgentConfig;

#[derive(Debug, PartialEq, Eq)]
pub enum ConfirmDecision {
    Yes,
    No,
    /// Llegó algo, pero no se entendió: ruido, una tos, media palabra, una
    /// transcripción vacía. No es motivo para cancelar — hay que repreguntar.
    Unintelligible,
    /// Una petición distinta: el usuario cambió de tema y la acción muere.
    Unrelated,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CodeDecision {
    Correct,
    /// Dijo un código, pero no es el correcto. Cuenta como intento.
    Wrong,
    Cancelled,
    /// No se entendió ningún número. No cuenta como intento fallido: el
    /// código no llegó a pronunciarse, así que repreguntar no es una
    /// oportunidad extra de adivinar.
    Unintelligible,
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

/// Hasta acá una respuesta que no se entendió se trata como ruido y se
/// repregunta; más larga que esto ya parece una frase con intención propia,
/// o sea que el usuario cambió de tema y la acción pendiente se cancela.
const MAX_WORDS_FOR_NOISE: usize = 4;

fn unmatched(normalized: &str) -> bool {
    normalized.split(' ').count() > MAX_WORDS_FOR_NOISE
}

/// Interpreta la respuesta a un "¿Confirma, señor?". El "no" tiene prioridad sobre el "sí"
pub fn interpret(text: &str, cfg: &AgentConfig) -> ConfirmDecision {
    let normalized = normalize(text);
    if normalized.is_empty() {
        return ConfirmDecision::Unintelligible;
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
    if unmatched(&normalized) {
        ConfirmDecision::Unrelated
    } else {
        ConfirmDecision::Unintelligible
    }
}

/// Valor de una palabra numérica en español. `normalize` ya sacó los
/// acentos, así que acá van sin ellos ("dieciseis", "veintidos").
fn number_word(word: &str) -> Option<u32> {
    Some(match word {
        "cero" => 0,
        "uno" | "una" | "un" => 1,
        "dos" => 2,
        "tres" => 3,
        "cuatro" => 4,
        "cinco" => 5,
        "seis" => 6,
        "siete" => 7,
        "ocho" => 8,
        "nueve" => 9,
        "diez" => 10,
        "once" => 11,
        "doce" => 12,
        "trece" => 13,
        "catorce" => 14,
        "quince" => 15,
        "dieciseis" => 16,
        "diecisiete" => 17,
        "dieciocho" => 18,
        "diecinueve" => 19,
        "veinte" => 20,
        "veintiuno" | "veintiun" | "veintiuna" => 21,
        "veintidos" => 22,
        "veintitres" => 23,
        "veinticuatro" => 24,
        "veinticinco" => 25,
        "veintiseis" => 26,
        "veintisiete" => 27,
        "veintiocho" => 28,
        "veintinueve" => 29,
        "treinta" => 30,
        "cuarenta" => 40,
        "cincuenta" => 50,
        "sesenta" => 60,
        "setenta" => 70,
        "ochenta" => 80,
        "noventa" => 90,
        "cien" | "ciento" => 100,
        "doscientos" | "doscientas" => 200,
        "trescientos" | "trescientas" => 300,
        "cuatrocientos" | "cuatrocientas" => 400,
        "quinientos" | "quinientas" => 500,
        "seiscientos" | "seiscientas" => 600,
        "setecientos" | "setecientas" => 700,
        "ochocientos" | "ochocientas" => 800,
        "novecientos" | "novecientas" => 900,
        "mil" => 1000,
        _ => return None,
    })
}

/// Extrae la secuencia de dígitos que el usuario pronunció, sin importar
/// cómo la haya transcrito Whisper. El mismo código `0201` puede llegar como
/// `"0201"`, `"02 01"`, `"cero dos cero uno"` o `"cero doscientos uno"`, y
/// las tres últimas dependen de cómo agrupó el modelo lo que oyó — que es
/// justo lo que el usuario no controla al hablar.
///
/// Las palabras numéricas contiguas se acumulan como un solo número
/// ("doscientos uno" = 201, "treinta y dos" = 32) y se vuelcan a dígitos al
/// cortarse la racha. Un "cero" siempre corta: es el único que no compone.
fn extract_digits(normalized: &str) -> String {
    let mut out = String::new();
    let mut acc: Option<u32> = None;

    fn flush(acc: &mut Option<u32>, out: &mut String) {
        if let Some(v) = acc.take() {
            out.push_str(&v.to_string());
        }
    }

    for token in normalized.split(' ') {
        if token.is_empty() {
            continue;
        }
        if token.chars().all(|c| c.is_ascii_digit()) {
            flush(&mut acc, &mut out);
            out.push_str(token);
            continue;
        }
        // "treinta y dos": la conjunción no corta la racha.
        if token == "y" && acc.is_some() {
            continue;
        }
        match number_word(token) {
            Some(0) => {
                flush(&mut acc, &mut out);
                out.push('0');
            }
            Some(1000) => acc = Some(acc.unwrap_or(1) * 1000),
            // Solo compone hacia abajo ("doscientos uno"). Si el nuevo valor
            // es mayor o igual son dos números distintos dichos seguidos
            // ("dos cinco" = 2 y 5, no 7).
            Some(v) => match acc {
                Some(prev) if prev > v => acc = Some(prev + v),
                Some(_) => {
                    flush(&mut acc, &mut out);
                    acc = Some(v);
                }
                None => acc = Some(v),
            },
            None => flush(&mut acc, &mut out),
        }
    }
    flush(&mut acc, &mut out);
    out
}

/// Interpreta la respuesta a la petición del código de aceptación de riesgos.
pub fn interpret_code(text: &str, cfg: &AgentConfig) -> CodeDecision {
    let normalized = normalize(text);
    if normalized.is_empty() {
        return CodeDecision::Unintelligible;
    }
    if matches_any(&normalized, &cfg.confirm_no) {
        return CodeDecision::Cancelled;
    }
    let digits = extract_digits(&normalized);
    if digits == cfg.risk_code {
        return CodeDecision::Correct;
    }
    // Una frase larga que no es el código es el usuario hablando de otra
    // cosa, no un intento fallido. Cancelarla (en vez de repreguntar) es
    // además lo que impide adivinar sin gastar intentos.
    if unmatched(&normalized) {
        return CodeDecision::Unrelated;
    }
    if digits.is_empty() {
        CodeDecision::Unintelligible
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
    }

    /// El caso que hacía inusable el nivel `Code`: cualquier ruido corto se
    /// tomaba como código incorrecto y cancelaba la acción de una.
    #[test]
    fn ruido_no_es_codigo_incorrecto() {
        for frase in ["", "  ", "eh", "mmm", "¿qué?", "sí", "ajá señor"] {
            assert_eq!(
                interpret_code(frase, &cfg()),
                CodeDecision::Unintelligible,
                "{frase}"
            );
        }
    }

    #[test]
    fn ruido_en_confirmacion_no_cancela() {
        for frase in ["", "eh", "mmm", "¿cómo?"] {
            assert_eq!(
                interpret(frase, &cfg()),
                ConfirmDecision::Unintelligible,
                "{frase}"
            );
        }
    }

    /// Whisper agrupa los dígitos hablados como se le antoja; todas estas
    /// son la misma persona diciendo "cero, dos, cero, uno".
    #[test]
    fn codigo_en_cualquier_agrupacion() {
        for frase in [
            "0201",
            "02 01",
            "0 2 0 1",
            "cero dos cero uno",
            "cero doscientos uno",
            "cero dos cero una",
            "el código es cero doscientos uno",
        ] {
            assert_eq!(
                interpret_code(frase, &cfg()),
                CodeDecision::Correct,
                "{frase}"
            );
        }
    }

    #[test]
    fn numeros_compuestos() {
        assert_eq!(extract_digits(&normalize("treinta y dos")), "32");
        assert_eq!(extract_digits(&normalize("doscientos uno")), "201");
        assert_eq!(extract_digits(&normalize("dieciséis")), "16");
        assert_eq!(extract_digits(&normalize("veintiuno")), "21");
        // Dos números seguidos, no una suma.
        assert_eq!(extract_digits(&normalize("dos cinco")), "25");
        assert_eq!(extract_digits(&normalize("mil doscientos")), "1200");
    }
}
