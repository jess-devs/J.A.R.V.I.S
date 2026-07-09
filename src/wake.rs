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
    Ignore,
}

pub struct AttentionGate {
    config: WakeConfig,
    window_deadline: Option<Instant>,
    ambient: VecDeque<(Instant, String)>,
}

impl AttentionGate {
    pub fn new(config: WakeConfig) -> Self {
        Self {
            config,
            window_deadline: None,
            ambient: VecDeque::new(),
        }
    }

    pub fn decide(&self, text: &str) -> GateDecision {
        if !self.config.enabled || self.contains_wake_word(text) || self.window_open() {
            GateDecision::Respond
        } else {
            GateDecision::Ignore
        }
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

    fn contains_wake_word(&self, text: &str) -> bool {
        tokens(text).iter().any(|token| {
            self.config
                .words
                .iter()
                .any(|word| levenshtein(token, &normalize(word)) <= 1)
        })
    }
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

fn tokens(text: &str) -> Vec<String> {
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
        assert_eq!(g.decide("qué opinas de esto, Jarvis?"), GateDecision::Respond);
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
        assert_eq!(g.decide(""), GateDecision::Ignore);
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
