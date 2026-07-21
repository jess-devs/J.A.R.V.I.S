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

/// Palabras funcionales tan comunes en español que compartirlas con lo que
/// Jarvis acaba de decir no es señal de eco: infla el solapamiento y
/// descarta respuestas cortas reales (ej. Jarvis pregunta "...a una hora
/// concreta de la mañana?" y el usuario responde "a la una..." — se
/// descartaba como eco por compartir "a"/"la"/"una", no contenido real).
const STOPWORDS: &[&str] = &[
    "a", "al", "de", "del", "el", "la", "los", "las", "un", "una", "unos", "unas", "y", "o",
    "en", "por", "para", "con", "sin", "se", "su", "sus", "lo", "le", "les", "es", "no", "si",
    "que", "mi", "tu",
];

fn is_stopword(token: &str) -> bool {
    STOPWORDS.contains(&token)
}

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
        if !self.config.enabled || !self.within_window() {
            return false;
        }
        let candidate = tokens(text);
        if candidate.is_empty() {
            return false;
        }
        // Solo las palabras de contenido del usuario cuentan como señal de
        // eco; `combined` (lo que dijo Jarvis) se deja intacto. Si no queda
        // ninguna tras filtrar, no hay nada real que comparar — mejor un
        // falso negativo ocasional que perder una respuesta corta genuina.
        let content_candidate: Vec<&String> =
            candidate.iter().filter(|t| !is_stopword(t)).collect();
        if content_candidate.is_empty() {
            return false;
        }

        // Todo lo que sigue en el buffer cuenta como contexto, sin refiltrar
        // por la edad de cada frase individual (ver `within_window`/bug
        // reportado: una respuesta larga tarda en decirse, y si cada frase
        // se mide por su propia antigüedad, las primeras "expiran" antes de
        // que el eco llegue a transcribirse — el usuario repitió una
        // respuesta de ~15s y no se detectó como eco porque las primeras
        // frases ya habían quedado fuera de la ventana individualmente).
        let combined: HashSet<String> = self
            .recent
            .iter()
            .flat_map(|(_, phrase)| tokens(phrase))
            .collect();
        if combined.is_empty() {
            return false;
        }

        let overlap = content_candidate
            .iter()
            .filter(|t| combined.contains(**t))
            .count();
        let similarity = overlap as f32 / content_candidate.len() as f32;
        similarity >= self.config.similarity_threshold
    }

    /// Frases dichas recientemente, en orden, para dar contexto a un chequeo
    /// de relevancia de barge-in (ver `agent::relevance`). Reutiliza la
    /// misma ventana que el eco (medida desde la última frase, no por
    /// frase — ver `within_window`).
    pub fn recent_spoken_text(&self) -> String {
        if !self.within_window() {
            return String::new();
        }
        self.recent
            .iter()
            .map(|(_, phrase)| phrase.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `true` si Jarvis terminó de decir la ÚLTIMA frase registrada hace
    /// menos de `recent_tts_window_secs` — la ventana se mide desde ahí, no
    /// desde cada frase por separado (ver el comentario en `is_echo`).
    fn within_window(&self) -> bool {
        let Some((last_at, _)) = self.recent.back() else {
            return false;
        };
        let window = Duration::from_secs(self.config.recent_tts_window_secs);
        Instant::now().duration_since(*last_at) <= window
    }

    /// Poda frases cuya antigüedad ya excede la ventana en el momento en que
    /// se registra una nueva — solo un límite de memoria para no acumular
    /// turnos viejos indefinidamente; la decisión de qué cuenta como "eco
    /// reciente" la toma `within_window`/`is_echo` de arriba.
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
    fn respuesta_corta_con_solo_stopwords_compartidos_no_es_eco() {
        let mut g = gate();
        g.note_spoken(
            "¿Quiere que le establezca un recordatorio a una hora concreta de la mañana?",
        );
        // Bug reportado: estas respuestas cortas se descartaban como eco por
        // compartir solo "a"/"la"/"una"/"de" con la pregunta de Jarvis.
        assert!(!g.is_echo("a la una la otra"));
        assert!(!g.is_echo("La una a la tarde"));
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
    fn recent_spoken_text_junta_las_frases_en_orden() {
        let mut g = gate();
        g.note_spoken("Hola");
        g.note_spoken("¿cómo estás?");
        assert_eq!(g.recent_spoken_text(), "Hola ¿cómo estás?");
    }

    #[test]
    fn eco_de_respuesta_larga_cuenta_frases_tempranas_ya_vencidas_individualmente() {
        // Bug reportado: en una respuesta larga, Jarvis registra cada frase
        // cuando termina de decirla. Si la ventana se mide por frase, la
        // primera frase "vence" antes de que el eco llegue a transcribirse
        // y su contenido queda afuera del solapamiento — aunque la frase
        // nunca se podó del buffer (el gap entre frases fue corto). La
        // ventana debe medirse desde la ÚLTIMA frase, no desde cada una.
        let mut g = EchoGate::new(EchoGuardConfig {
            recent_tts_window_secs: 1, // 1s: alcanza para el gap corto entre frases
            ..EchoGuardConfig::default()
        });
        g.note_spoken("corazón estómago manos");
        std::thread::sleep(Duration::from_millis(400));
        // Al notar la segunda frase, prune() ve que la primera solo tiene
        // 400ms (< 1s de ventana): sobrevive en el buffer.
        g.note_spoken("virus clínica hoy");
        std::thread::sleep(Duration::from_millis(700));
        // Ahora la primera frase tiene ~1100ms (> 1s medido por su cuenta),
        // pero la última tiene ~700ms (< 1s): dentro de la ventana medida
        // desde la última frase, así que ambas deben contar como contexto.
        assert!(g.is_echo("corazón estómago manos virus clínica hoy"));
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
