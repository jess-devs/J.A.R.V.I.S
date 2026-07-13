//! Gate de activación: decide si una transcripción merece respuesta.
//!
//! Jarvis responde solo si la frase contiene su nombre (matching normalizado
//! con tolerancia a errores de transcripción de Whisper) o si llega dentro de
//! la ventana de atención que se abre al terminar cada respuesta. Lo demás se
//! ignora, pero queda en un buffer acotado que se antepone como contexto a la
//! siguiente consulta real.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::config::WakeConfig;

#[derive(Debug, PartialEq, Eq)]
pub enum GateDecision {
    Respond,
    /// Ignorar, pero conservar como contexto ambiental (frase real dicha
    /// fuera de la ventana, sin el nombre).
    Ignore,
    /// Descartar por completo: probable alucinación de Whisper o frase-basura.
    /// Ni siquiera se guarda como contexto ambiental.
    Drop,
}

pub struct AttentionGate {
    config: WakeConfig,
    window_deadline: Option<Instant>,
    ambient: VecDeque<(Instant, String)>,
    /// Última transcripción respondida (normalizada) y cuándo, para descartar
    /// repeticiones inmediatas (Whisper entra en loops en silencio).
    last_responded: Option<(Instant, String)>,
}

impl AttentionGate {
    pub fn new(config: WakeConfig) -> Self {
        Self {
            config,
            window_deadline: None,
            ambient: VecDeque::new(),
            last_responded: None,
        }
    }

    pub fn decide(&self, text: &str) -> GateDecision {
        let normalized = normalize_phrase(text);
        if normalized.is_empty() {
            return GateDecision::Drop;
        }

        // Frase-basura conocida (alucinación típica en silencio/ruido).
        if self.is_ignore_phrase(&normalized) {
            return GateDecision::Drop;
        }

        // Repetición inmediata de lo último respondido: loop de Whisper.
        if self.is_recent_repeat(&normalized) {
            return GateDecision::Drop;
        }

        // El nombre siempre activa, sin importar la ventana.
        if self.contains_wake_word(text) {
            return GateDecision::Respond;
        }

        if !self.config.enabled {
            return GateDecision::Respond;
        }

        if self.window_open() {
            // Dentro de la ventana pero sin el nombre: exigir sustancia
            // mínima. Las alucinaciones en silencio son casi siempre de una
            // sola palabra.
            let word_count = normalized.split(' ').filter(|w| !w.is_empty()).count();
            if word_count < self.config.window_min_words {
                GateDecision::Drop
            } else {
                GateDecision::Respond
            }
        } else {
            GateDecision::Ignore
        }
    }

    /// Registra la transcripción como respondida, para el guard de repetición.
    /// Se llama al aceptar una frase que va a generar respuesta.
    pub fn mark_responded(&mut self, text: &str) {
        self.last_responded = Some((Instant::now(), normalize_phrase(text)));
    }

    fn is_ignore_phrase(&self, normalized: &str) -> bool {
        self.config
            .ignore_phrases
            .iter()
            .any(|p| normalize_phrase(p) == normalized)
    }

    fn is_recent_repeat(&self, normalized: &str) -> bool {
        self.last_responded.as_ref().is_some_and(|(at, prev)| {
            prev == normalized && Instant::now().duration_since(*at) <= Duration::from_secs(10)
        })
    }

    /// Abre (o extiende) la ventana de atención. Se llama al terminar cada
    /// respuesta, incluso si el pipeline falló: el usuario querrá reintentar
    /// sin repetir el nombre.
    pub fn open_window(&mut self) {
        self.window_deadline =
            Some(Instant::now() + Duration::from_secs(self.config.attention_window_secs));
    }

    pub fn push_ambient(&mut self, text: String) {
        if !self.config.ambient_context {
            return;
        }
        self.ambient.push_back((Instant::now(), text));
        while self.ambient.len() > self.config.ambient_context_max {
            self.ambient.pop_front();
        }
    }

    /// Drena el buffer de frases ignoradas (descartando las expiradas) y lo
    /// formatea para anteponerlo al siguiente mensaje real del usuario.
    pub fn take_ambient_context(&mut self) -> Option<String> {
        let ttl = Duration::from_secs(self.config.ambient_context_ttl_secs);
        let now = Instant::now();
        let phrases: Vec<String> = self
            .ambient
            .drain(..)
            .filter(|(at, _)| now.duration_since(*at) <= ttl)
            .map(|(_, text)| format!("«{text}»"))
            .collect();
        if phrases.is_empty() {
            None
        } else {
            Some(format!(
                "(Antes de dirigirse a ti, el usuario dijo: {})",
                phrases.join(" ")
            ))
        }
    }

    fn window_open(&self) -> bool {
        self.window_deadline
            .is_some_and(|deadline| Instant::now() < deadline)
    }

    /// Expuesto a `crate::echo_gate` para la detección de wake word durante
    /// una interrupción por voz (barge-in en modo `wake_word`), fuera del
    /// flujo normal de `decide()` (que también mira la ventana de atención).
    pub(crate) fn contains_wake_word(&self, text: &str) -> bool {
        tokens(text).iter().any(|token| {
            self.config
                .words
                .iter()
                .any(|word| levenshtein(token, &normalize(word)) <= 1)
        })
    }
}

/// Minúsculas, sin tildes ni puntuación, pero CONSERVANDO los espacios entre
/// palabras — "¡Sí, señor!" → "si senor". Para comparar frases completas
/// (frases-basura, repeticiones) y contar palabras.
fn normalize_phrase(text: &str) -> String {
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

/// Minúsculas, sin tildes/diéresis y solo letras — "¡Járvis!" → "jarvis".
fn normalize(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter_map(|c| match c {
            'á' => Some('a'),
            'é' => Some('e'),
            'í' => Some('i'),
            'ó' => Some('o'),
            'ú' | 'ü' => Some('u'),
            c if c.is_alphabetic() => Some(c),
            _ => None,
        })
        .collect()
}

/// Expuesto a `crate::echo_gate` para comparar por solapamiento de tokens.
pub(crate) fn tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(normalize)
        .filter(|t| !t.is_empty())
        .collect()
}

/// Distancia de Levenshtein con DP de una sola fila. Suficiente para comparar
/// tokens cortos contra el wake word; no vale la pena un crate para esto.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut prev_diag = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            let val = (prev_diag + cost).min(row[j] + 1).min(row[j + 1] + 1);
            prev_diag = row[j + 1];
            row[j + 1] = val;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(config: WakeConfig) -> AttentionGate {
        AttentionGate::new(config)
    }

    fn default_gate() -> AttentionGate {
        gate(WakeConfig::default())
    }

    #[test]
    fn responde_con_el_nombre_en_cualquier_posicion() {
        let g = default_gate();
        assert_eq!(g.decide("Oye Jarvis qué hora es"), GateDecision::Respond);
        assert_eq!(
            g.decide("qué opinas de esto, Jarvis?"),
            GateDecision::Respond
        );
        assert_eq!(g.decide("Jarvis."), GateDecision::Respond);
    }

    #[test]
    fn tolera_errores_de_transcripcion() {
        let g = default_gate();
        for variante in ["yarvis", "harvis", "jarbis", "jervis", "Járvis", "jarvi"] {
            assert_eq!(
                g.decide(&format!("{variante} enciende la luz")),
                GateDecision::Respond,
                "debería aceptar {variante}"
            );
        }
    }

    #[test]
    fn ignora_sin_nombre_y_sin_ventana() {
        let g = default_gate();
        assert_eq!(g.decide("voy a pedir una pizza"), GateDecision::Ignore);
    }

    #[test]
    fn descarta_vacio_y_frases_basura() {
        let g = default_gate();
        assert_eq!(g.decide(""), GateDecision::Drop);
        assert_eq!(g.decide("   "), GateDecision::Drop);
        // Alucinaciones típicas de Whisper, con y sin tildes/puntuación.
        assert_eq!(g.decide("Gracias."), GateDecision::Drop);
        assert_eq!(g.decide("¡Suscríbete!"), GateDecision::Drop);
        assert_eq!(
            g.decide("Subtítulos realizados por la comunidad de Amara.org"),
            GateDecision::Drop
        );
    }

    #[test]
    fn en_ventana_descarta_una_palabra_pero_responde_multipalabra() {
        let mut g = default_gate();
        g.open_window();
        // Alucinaciones de una palabra dentro de la ventana → Drop.
        for fantasma in ["Bip", "Liz", "Bienvenido", "Bien"] {
            assert_eq!(g.decide(fantasma), GateDecision::Drop, "{fantasma}");
        }
        // Un comando real multi-palabra dentro de la ventana → Respond.
        assert_eq!(g.decide("pon una canción de mora"), GateDecision::Respond);
    }

    #[test]
    fn el_nombre_activa_aunque_sea_una_palabra() {
        let g = default_gate();
        assert_eq!(g.decide("Jarvis"), GateDecision::Respond);
    }

    #[test]
    fn descarta_repeticion_inmediata() {
        let mut g = default_gate();
        g.open_window();
        let frase = "abre el bloc de notas";
        assert_eq!(g.decide(frase), GateDecision::Respond);
        g.mark_responded(frase);
        // Misma frase repetida de inmediato (loop de Whisper) → Drop.
        assert_eq!(g.decide(frase), GateDecision::Drop);
    }

    #[test]
    fn no_acepta_palabras_a_distancia_dos() {
        let g = default_gate();
        assert_eq!(g.decide("javier ven acá"), GateDecision::Ignore);
        assert_eq!(g.decide("qué buen servicio"), GateDecision::Ignore);
    }

    #[test]
    fn deshabilitado_responde_a_todo() {
        let g = gate(WakeConfig {
            enabled: false,
            ..WakeConfig::default()
        });
        assert_eq!(g.decide("cualquier cosa"), GateDecision::Respond);
    }

    #[test]
    fn ventana_abierta_responde_sin_nombre() {
        let mut g = default_gate();
        assert_eq!(g.decide("y a qué hora cierra"), GateDecision::Ignore);
        g.open_window();
        assert_eq!(g.decide("y a qué hora cierra"), GateDecision::Respond);
    }

    #[test]
    fn ventana_expirada_vuelve_a_ignorar() {
        let mut g = gate(WakeConfig {
            attention_window_secs: 0,
            ..WakeConfig::default()
        });
        g.open_window();
        assert_eq!(g.decide("y a qué hora cierra"), GateDecision::Ignore);
    }

    #[test]
    fn ambient_se_acota_y_se_drena() {
        let mut g = gate(WakeConfig {
            ambient_context_max: 2,
            ..WakeConfig::default()
        });
        g.push_ambient("uno".to_string());
        g.push_ambient("dos".to_string());
        g.push_ambient("tres".to_string());
        let ctx = g.take_ambient_context().unwrap();
        assert!(!ctx.contains("«uno»"), "la más vieja debió salir: {ctx}");
        assert!(ctx.contains("«dos»") && ctx.contains("«tres»"));
        assert_eq!(g.take_ambient_context(), None, "el drenaje debe vaciar");
    }

    #[test]
    fn ambient_expira_por_ttl() {
        let mut g = gate(WakeConfig {
            ambient_context_ttl_secs: 0,
            ..WakeConfig::default()
        });
        g.push_ambient("vieja".to_string());
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(g.take_ambient_context(), None);
    }

    #[test]
    fn ambient_deshabilitado_no_acumula() {
        let mut g = gate(WakeConfig {
            ambient_context: false,
            ..WakeConfig::default()
        });
        g.push_ambient("ruido".to_string());
        assert_eq!(g.take_ambient_context(), None);
    }

    #[test]
    fn levenshtein_basico() {
        assert_eq!(levenshtein("jarvis", "jarvis"), 0);
        assert_eq!(levenshtein("yarvis", "jarvis"), 1);
        assert_eq!(levenshtein("javier", "jarvis"), 3);
        assert_eq!(levenshtein("jarbis", "jarvis"), 1);
        assert_eq!(levenshtein("", "jarvis"), 6);
    }
}
