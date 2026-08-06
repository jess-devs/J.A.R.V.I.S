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

/// Palabras mínimas que tiene que dejar la cola para tomarla como habla real.
///
/// Una alcanza: la respuesta más común a un "¿Necesita algo más?" es
/// justamente de una palabra, y llega pegada al eco. El riesgo de tomar un
/// artefacto del STT por respuesta ya lo cubre el wake gate aguas abajo, que
/// solo acepta una palabra suelta si Jarvis venía de preguntar algo.
const MIN_REMAINDER_WORDS: usize = 1;

/// Cuántos tokens del final de la frase de Jarvis se usan como ancla para
/// ubicar dónde termina el eco dentro de la transcripción. Pocos hacen que
/// enganche en cualquier lado; muchos hacen que un solo error del STT en ese
/// tramo impida encontrarlo.
const ECHO_TAIL_ANCHOR: usize = 4;

/// Proporción de tokens de `words` que Jarvis ya había dicho. Misma medida
/// que usa `is_echo`, para que la frontera se decida con el mismo criterio.
fn similarity_of(words: &[&str], spoken: &HashSet<String>) -> f32 {
    let (mut total, mut hits) = (0usize, 0usize);
    for word in words {
        for token in tokens(word) {
            total += 1;
            if spoken.contains(&token) {
                hits += 1;
            }
        }
    }
    if total == 0 {
        return 0.0;
    }
    hits as f32 / total as f32
}

pub struct EchoGate {
    config: EchoGuardConfig,
    recent: VecDeque<(Instant, String)>,
    /// Cuándo empezó a sonar la última frase. La ventana se mide desde acá y
    /// no desde cada frase por separado: una respuesta larga tarda en
    /// reproducirse más que la ventana entera, y sus primeras frases quedaban
    /// fuera justo cuando el micrófono devolvía el eco de la respuesta
    /// completa. El solapamiento caía por debajo del umbral, el eco pasaba
    /// por habla del usuario, y Jarvis se contestaba a sí mismo.
    last_spoken: Option<Instant>,
}

impl EchoGate {
    pub fn new(config: EchoGuardConfig) -> Self {
        Self {
            config,
            recent: VecDeque::new(),
            last_spoken: None,
        }
    }

    /// Registra una frase que Jarvis efectivamente empezó a reproducir.
    pub fn note_spoken(&mut self, phrase: &str) {
        if !self.config.enabled || phrase.trim().is_empty() {
            return;
        }
        self.prune();
        self.recent.push_back((Instant::now(), phrase.to_string()));
        self.last_spoken = Some(Instant::now());
    }

    /// `true` mientras el eco de lo último que dijo Jarvis siga siendo
    /// plausible: hasta `recent_tts_window_secs` después de que dejó de
    /// hablar, no después de cada frase suelta.
    fn window_open(&self) -> bool {
        let window = Duration::from_secs(self.config.recent_tts_window_secs);
        self.last_spoken
            .is_some_and(|at| Instant::now().duration_since(at) <= window)
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

        let combined = self.recent_tokens();
        if combined.is_empty() {
            return false;
        }

        let overlap = candidate.iter().filter(|t| combined.contains(*t)).count();
        let similarity = overlap as f32 / candidate.len() as f32;
        similarity >= self.config.similarity_threshold
    }

    /// Cuando el eco arrastra pegada la respuesta del usuario, devuelve solo
    /// esa cola. `None` si todo es eco — el caso normal.
    ///
    /// Whisper a veces mete en un mismo segmento el final de lo que dijo
    /// Jarvis y lo que el usuario contestó encima ("...en el lugar correcto.
    /// Exacto, Cloud Code"). Como el conjunto se parece muchísimo a lo que
    /// Jarvis dijo, `is_echo` lo descarta entero y la respuesta se pierde: el
    /// usuario tiene que repetirla.
    ///
    /// El eco siempre es prefijo (el micrófono capta primero a Jarvis), así
    /// que se busca la cola más larga que por sí sola ya no parezca eco. Solo
    /// baja del umbral cuando contiene palabras que Jarvis no dijo, o sea
    /// habla real.
    pub fn user_speech_after_echo(&self, text: &str) -> Option<String> {
        if !self.config.enabled {
            return None;
        }
        let combined = self.recent_tokens();
        if combined.is_empty() {
            return None;
        }

        // Se ubica el final del eco por posición, no por parecido: una cola
        // corta baja del umbral por pura aritmética (dos de tres palabras ya
        // dan 0.67), así que medir similitud rescataba pedazos del propio
        // eco. Lo que sí distingue es dónde terminó de hablar Jarvis.
        let spoken = tokens(&self.last_spoken_in_window()?);
        let anchor = &spoken[spoken.len().saturating_sub(ECHO_TAIL_ANCHOR)..];
        if anchor.is_empty() {
            return None;
        }

        let words: Vec<&str> = text.split_whitespace().collect();
        let mut matched = 0usize;
        let mut echo_ends_at = None;
        for (i, word) in words.iter().enumerate() {
            for token in tokens(word) {
                if matched < anchor.len() && token == anchor[matched] {
                    matched += 1;
                    echo_ends_at = Some(i);
                }
            }
        }
        // Sin el cierre completo de la frase de Jarvis no se sabe dónde
        // termina el eco, y cortar a ojo partiría la frase del usuario.
        if matched < anchor.len() {
            return None;
        }

        let remainder = &words[echo_ends_at? + 1..];
        if remainder.len() < MIN_REMAINDER_WORDS {
            return None;
        }
        // Red de seguridad contra los loops de Whisper, que repiten la frase
        // entera: si la cola sigue pareciéndose a lo que Jarvis dijo, es eco.
        if similarity_of(remainder, &combined) >= self.config.similarity_threshold {
            return None;
        }
        Some(remainder.join(" "))
    }

    /// Última frase que Jarvis dijo, si sigue dentro de la ventana. Es contra
    /// su final que se alinea el eco.
    fn last_spoken_in_window(&self) -> Option<String> {
        if !self.window_open() {
            return None;
        }
        self.recent.back().map(|(_, phrase)| phrase.clone())
    }

    /// Tokens de todo lo que Jarvis dijo dentro de la ventana vigente.
    fn recent_tokens(&self) -> HashSet<String> {
        if !self.window_open() {
            return HashSet::new();
        }
        self.recent
            .iter()
            .flat_map(|(_, phrase)| tokens(phrase))
            .collect()
    }

    /// Frases dichas recientemente (dentro de `recent_tts_window_secs`), en
    /// orden, para dar contexto a un chequeo de relevancia de barge-in (ver
    /// `agent::relevance`). Reutiliza la misma ventana que el eco.
    pub fn recent_spoken_text(&self) -> String {
        if !self.window_open() {
            return String::new();
        }
        self.recent
            .iter()
            .map(|(_, phrase)| phrase.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Un turno de habla se descarta entero, no frase por frase: mientras
    /// Jarvis siga hablando todas sus frases siguen siendo eco posible.
    fn prune(&mut self) {
        if !self.window_open() {
            self.recent.clear();
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

    /// El caso del log: Whisper metió en un solo segmento el final de lo que
    /// dijo Jarvis y la respuesta del usuario encima. Antes se descartaba
    /// entero y el usuario tenía que repetir.
    #[test]
    fn rescata_la_respuesta_pegada_al_final_del_eco() {
        let mut g = gate();
        g.note_spoken(
            "me aclara si se refiere a Claude Code en esta máquina, a un servicio como \
             GitHub Codespaces, o a otra cosa. Así instalo las seis skills en el lugar correcto",
        );

        let transcripcion = "Me aclaras y se refiere a Cloud Code en esta máquina a un servicio \
                             como GitHub con espaces o a otra cosa. Así instalo las seis skills \
                             en el lugar correcto. Exacto, Cloud Code.";
        assert!(
            g.is_echo(transcripcion),
            "debería seguir detectándose como eco"
        );

        let rescatado = g
            .user_speech_after_echo(transcripcion)
            .expect("tenía que rescatar la cola del usuario");
        assert!(
            rescatado.contains("Exacto"),
            "se perdió la respuesta del usuario: {rescatado:?}"
        );
        assert!(
            !rescatado.contains("GitHub"),
            "se coló parte del eco: {rescatado:?}"
        );
    }

    /// Lo habitual: el eco es eco y nada más. Rescatar algo acá sería peor
    /// que no rescatar nada, porque dispararía un turno con la propia voz.
    #[test]
    fn un_eco_puro_no_deja_nada_que_rescatar() {
        let mut g = gate();
        g.note_spoken("El clima de hoy es soleado con veinte grados");
        assert_eq!(
            g.user_speech_after_echo("el clima de hoy es soleado con veinte grados"),
            None
        );
    }

    /// Sin el cierre de la frase de Jarvis no se sabe dónde termina el eco.
    /// Cortar a ojo partiría la frase del usuario por la mitad, así que se
    /// prefiere no rescatar nada.
    #[test]
    fn sin_ubicar_el_final_del_eco_no_rescata_nada() {
        let mut g = gate();
        g.note_spoken("El clima de hoy es soleado con veinte grados");
        // La transcripción se cortó antes del final: el ancla no aparece.
        assert_eq!(g.user_speech_after_echo("el clima de hoy es soleado"), None);
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
    fn recent_spoken_text_junta_las_frases_en_orden() {
        let mut g = gate();
        g.note_spoken("Hola");
        g.note_spoken("¿cómo estás?");
        assert_eq!(g.recent_spoken_text(), "Hola ¿cómo estás?");
    }

    /// El bug del log: una respuesta de 52 segundos tarda en reproducirse
    /// mucho más que la ventana de 12, así que sus primeras frases quedaban
    /// fuera justo cuando el micrófono devolvía el eco completo. El
    /// solapamiento caía y Jarvis se contestaba a sí mismo.
    #[test]
    fn una_respuesta_mas_larga_que_la_ventana_sigue_contando_entera() {
        let mut g = EchoGate::new(EchoGuardConfig {
            recent_tts_window_secs: 1,
            ..EchoGuardConfig::default()
        });
        g.note_spoken("El proyecto es un asistente de voz en español");
        std::thread::sleep(Duration::from_millis(600));
        g.note_spoken("y también reacciona a un doble aplauso");
        // La primera frase ya quedó más vieja que la ventana, pero el turno
        // sigue vivo porque Jarvis habló hace nada.
        std::thread::sleep(Duration::from_millis(600));

        assert!(
            g.is_echo(
                "el proyecto es un asistente de voz en español y también reacciona a un doble aplauso"
            ),
            "la primera frase del turno quedó fuera de la ventana"
        );
    }

    /// La respuesta más común a "¿Necesita algo más?" es una sola palabra, y
    /// llega pegada al eco. En el log se perdía y había que repetirla.
    #[test]
    fn rescata_una_sola_palabra_pegada_al_eco() {
        let mut g = gate();
        g.note_spoken("Listo, señor. Ya tiene abierta la carpeta de descargas");
        g.note_spoken("¿Necesita algo más?");

        let transcripcion =
            "¡Listo, señor! Ya tiene abierta la carpeta de descargas. ¿Necesita algo más? No.";
        assert!(g.is_echo(transcripcion));
        assert_eq!(
            g.user_speech_after_echo(transcripcion).as_deref(),
            Some("No.")
        );
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
