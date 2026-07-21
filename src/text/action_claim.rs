//! Detección barata (sin LLM) de frases que reclaman haber completado una
//! acción, para telemetría: JARVIS puede llamar tools reales, pero un
//! modelo local chico a veces alucina haber hecho algo que nunca ejecutó
//! (ver `pipeline::streaming::run_speaking_turn`, que compara esto contra
//! los tool calls reales de cada turno).

/// Patrones de reclamo de acción ya completada, en español. Comparación por
/// substring sobre texto normalizado a minúsculas — sin regex, alcanza para
/// una lista curada y chica. Intencionalmente permisivo: para telemetría un
/// falso positivo solo ensucia un log, no rompe nada (ver el comentario en
/// `run_speaking_turn` sobre por qué esto no bloquea audio todavía).
const ACTION_CLAIM_PATTERNS: &[&str] = &[
    "se ha activado",
    "se ha suspendido",
    "se ha desactivado",
    "se ha guardado",
    "se ha eliminado",
    "se ha cerrado",
    "se ha abierto",
    "se ha completado",
    "ya lo hice",
    "ya está hecho",
    "listo, ya",
    "acabo de",
    "activé",
    "desactivé",
    "suspendí",
    "cerré",
    "guardé",
    "eliminé",
];

/// `true` si `phrase` suena a que el modelo está afirmando haber completado
/// una acción en primera persona (más allá de si realmente pasó o no).
pub fn looks_like_completed_action_claim(phrase: &str) -> bool {
    let normalized = phrase.to_lowercase();
    ACTION_CLAIM_PATTERNS.iter().any(|p| normalized.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detecta_el_reclamo_real_reportado() {
        assert!(looks_like_completed_action_claim(
            "Entendido señor/a, se ha suspendido la actividad. Por favor, mantenga este silencio."
        ));
    }

    #[test]
    fn no_marca_una_respuesta_normal() {
        assert!(!looks_like_completed_action_claim(
            "El clima de hoy es soleado con veinte grados."
        ));
        assert!(!looks_like_completed_action_claim(
            "¿Te refieres a un problema específico relacionado con tu proyecto?"
        ));
    }

    #[test]
    fn detecta_variantes_en_primera_persona() {
        assert!(looks_like_completed_action_claim(
            "Listo, ya guardé el recordatorio."
        ));
        assert!(looks_like_completed_action_claim(
            "Acabo de cerrar el navegador."
        ));
    }
}
