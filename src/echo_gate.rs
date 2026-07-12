//! Filtro de eco para barge-in: cuando el motor STT nativo reporta que el
//! usuario "habló" mientras Jarvis sonaba, esa transcripción puede ser el
//! propio audio de Jarvis captado por el micrófono (con altavoces y sin
//! AEC) en vez de habla real del usuario. Se compara por solapamiento de
//! tokens normalizados (reutilizando `crate::wake::tokens`) contra las
//! frases que Jarvis efectivamente dijo hace poco — si coincide lo
//! suficiente, se descarta como eco propio.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use crate::config::EchoGuardConfig;
use crate::wake::tokens;

pub struct EchoGate {
    config: EchoGuardConfig,
    recent: VecDeque<(Instant, String)>,
}

impl EchoGate {
    pub fn new(config: EchoGuardConfig) -> Self {
        Self {
            config,
            recent: VecDeque::new(),
        }
    }

    /// Registra una frase que Jarvis efectivamente empezó a reproducir.
    pub fn note_spoken(&mut self, phrase: &str) {
        if !self.config.enabled || phrase.trim().is_empty() {
            return;
        }
        self.prune();
        self.recent.push_back((Instant::now(), phrase.to_string()));
    }

    /// true si `text` se parece lo bastante a algo que Jarvis dijo hace poco
    /// como para ser su propio eco en vez de habla real del usuario.
    pub fn is_echo(&self, text: &str) -> bool {
        if !self.config.enabled || self.recent.is_empty() {
            return false;
        }
        let candidate = tokens(text);
        if candidate.is_empty() {
            return false;
        }

        let window = Duration::from_secs(self.config.recent_tts_window_secs);
        let now = Instant::now();
        let combined: HashSet<String> = self
            .recent
            .iter()
            .filter(|(at, _)| now.duration_since(*at) <= window)
            .flat_map(|(_, phrase)| tokens(phrase))
            .collect();
        if combined.is_empty() {
            return false;
        }

        let overlap = candidate.iter().filter(|t| combined.contains(*t)).count();
        let similarity = overlap as f32 / candidate.len() as f32;
        similarity >= self.config.similarity_threshold
    }

    fn prune(&mut self) {
        let window = Duration::from_secs(self.config.recent_tts_window_secs);
        let now = Instant::now();
        while let Some((at, _)) = self.recent.front() {
            if now.duration_since(*at) > window {
                self.recent.pop_front();
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate() -> EchoGate {
        EchoGate::new(EchoGuardConfig::default())
    }

    #[test]
    fn frase_identica_es_eco() {
        let mut g = gate();
        g.note_spoken("El clima de hoy es soleado con veinte grados");
        assert!(g.is_echo("el clima de hoy es soleado con veinte grados"));
    }

    #[test]
    fn frase_no_relacionada_no_es_eco() {
        let mut g = gate();
        g.note_spoken("El clima de hoy es soleado con veinte grados");
        assert!(!g.is_echo("jarvis para de hablar un momento"));
    }

    #[test]
    fn solapamiento_parcial_bajo_el_umbral_no_es_eco() {
        let mut g = gate();
        g.note_spoken("Puedo ayudarte a revisar el calendario de mañana");
        // Comparte pocas palabras ("el", "de") con lo dicho por Jarvis.
        assert!(!g.is_echo("oye jarvis abrí el navegador de una vez"));
    }

    #[test]
    fn sin_frases_recientes_nunca_es_eco() {
        let g = gate();
        assert!(!g.is_echo("cualquier cosa que diga el usuario"));
    }

    #[test]
    fn deshabilitado_nunca_marca_eco() {
        let mut g = EchoGate::new(EchoGuardConfig {
            enabled: false,
            ..EchoGuardConfig::default()
        });
        g.note_spoken("El clima de hoy es soleado con veinte grados");
        assert!(!g.is_echo("el clima de hoy es soleado con veinte grados"));
    }

    #[test]
    fn frase_vieja_fuera_de_la_ventana_no_cuenta() {
        let mut g = EchoGate::new(EchoGuardConfig {
            recent_tts_window_secs: 0,
            ..EchoGuardConfig::default()
        });
        g.note_spoken("El clima de hoy es soleado con veinte grados");
        std::thread::sleep(Duration::from_millis(5));
        assert!(!g.is_echo("el clima de hoy es soleado con veinte grados"));
    }
}
