//! Capa de herramientas del agente: cada `Tool` expone su nombre, schema de
//! parámetros y nivel de riesgo, y el `ToolRegistry` las agrupa para
//! ofrecérselas al LLM.
//!
//! Los resultados de `execute` son texto plano compacto en español (no JSON
//! crudo): un modelo local de 7B resume mejor texto ya legible. El registro
//! trunca cada resultado a `agent.max_tool_result_chars`.

pub mod apps;
pub mod files;
pub mod input;
pub mod media;
pub mod memory;
pub mod music;
pub mod reminders;
pub mod screen;
pub mod scripted;
pub mod scripted_store;
pub mod shell;
pub mod silence;
pub mod system_info;
pub mod translate;
pub mod volume;
pub mod web;

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::Value;

use self::scripted_store::ScriptedToolStore;
use crate::audio::MusicShared;
use crate::config::{AgentConfig, McpServerConfig, ScriptedToolsConfig};
use crate::errors::ToolError;
use crate::llm::{ImageBlock, ToolSpec};
use crate::memory::MemoryStore;
use crate::reminders::ReminderStore;

/// Cuánto peligro implica ejecutar una herramienta con ciertos argumentos.
/// Se evalúa de forma determinista en Rust — nunca lo decide el LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// Lectura/consulta: se ejecuta directo.
    Safe,
    /// Modifica el sistema: requiere un "sí" por voz.
    Confirm,
    /// Riesgo extremo (borrado recursivo, apagado, registro...): requiere
    /// pronunciar el código de aceptación de riesgos.
    Code,
}

/// Resultado de ejecutar una tool: texto plano (el caso común) más,
/// opcionalmente, imágenes (hoy solo `take_screenshot` las produce).
#[derive(Debug, Clone, Default)]
pub struct ToolOutput {
    pub text: String,
    pub images: Vec<ImageBlock>,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
        }
    }

    pub fn with_images(text: impl Into<String>, images: Vec<ImageBlock>) -> Self {
        Self {
            text: text.into(),
            images,
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    /// `&str` (no `&'static str`): las tools "scripted" (`scripted.rs`)
    /// tienen nombre/descripción dinámicos, cargados de SQLite. Las tools
    /// built-in siguen devolviendo literales `&'static str` sin cambios —
    /// coercionan bien al tipo más general del trait.
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// JSON Schema del objeto de parámetros (`{"type": "object", ...}`).
    fn parameters_schema(&self) -> Value;
    /// Clasifica el riesgo SEGÚN LOS ARGUMENTOS, no estático: p.ej.
    /// `run_powershell` es `Confirm` por defecto pero `Code` si el comando
    /// matchea patrones peligrosos.
    fn assess_risk(&self, args: &Value) -> RiskLevel;
    /// Descripción hablada de la acción (más los riesgos identificados, si
    /// los hay) para el diálogo de confirmación: "cerrar Chrome", "ejecutar
    /// el comando ... — esto borra archivos de forma irreversible".
    fn describe_action(&self, args: &Value) -> String;
    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError>;
}

/// Nombres bajo los cuales el LLM puede pedir que se recargue el conjunto de
/// tools "scripted" tras `create_tool`/`delete_custom_tool` (ver
/// `execute_and_record` en `agent/turn.rs`).
pub const SCRIPTED_TOOL_MUTATORS: &[&str] = &["create_tool", "delete_custom_tool"];

/// Qué porcentaje del presupuesto de `max_tool_result_chars` se reserva para
/// el principio del resultado; el resto va al final.
///
/// Se conservan las dos puntas y no solo el principio porque el final es lo
/// que el modelo suele necesitar (el cierre de un archivo, las últimas líneas
/// de una salida). Cortándolo, su única salida era volver a ejecutar el
/// comando con `tail` — y cada reintento gasta una iteración del turno, hasta
/// agotar `agent.max_iterations` leyendo el mismo archivo.
const TRUNCATE_HEAD_PERCENT: usize = 60;

/// Por debajo de esto no tiene sentido repartir el presupuesto en dos puntas:
/// quedarían dos fragmentos demasiado cortos para significar algo.
const MIN_CHARS_FOR_SPLIT_TRUNCATION: usize = 400;

pub struct ToolRegistry {
    /// Tools incorporadas + scripted vigentes, recalculado en cada
    /// `reload_scripted`. Detrás de un `RwLock` (no `Mutex`: lecturas
    /// concurrentes triviales) para que `create_tool`/`delete_custom_tool`
    /// puedan registrar cambios sin necesitar `&mut ToolRegistry` a través
    /// de todo el loop agéntico.
    tools: RwLock<Vec<Arc<dyn Tool>>>,
    specs: RwLock<Arc<Vec<ToolSpec>>>,
    /// Tools incorporadas (no cambian tras el arranque): se preservan aparte
    /// para reconstruir `tools` en cada `reload_scripted` sin tener que
    /// volver a instanciarlas.
    static_tools: Vec<Arc<dyn Tool>>,
    scripted_store: Option<Arc<ScriptedToolStore>>,
    scripted_cfg: ScriptedToolsConfig,
    disabled_tools: Vec<String>,
    max_result_chars: usize,
}

impl ToolRegistry {
    pub async fn build(
        cfg: &AgentConfig,
        memory: Arc<MemoryStore>,
        reminder_store: Arc<ReminderStore>,
        scripted_store: Arc<ScriptedToolStore>,
        music: Option<Arc<MusicShared>>,
        silence_flag: Arc<AtomicBool>,
        mcp_servers: &[McpServerConfig],
    ) -> Self {
        let mut static_tools: Vec<Arc<dyn Tool>> = Vec::new();
        if cfg.enabled {
            static_tools.push(Arc::new(system_info::GetDatetime));
            static_tools.push(Arc::new(system_info::SystemStatus));
            static_tools.push(Arc::new(system_info::ListProcesses));
            static_tools.push(Arc::new(apps::OpenApp::new(&cfg.apps)));
            static_tools.push(Arc::new(apps::OpenUrl));
            static_tools.push(Arc::new(apps::CloseApp));
            static_tools.push(Arc::new(files::FindFiles::new(&cfg.files)));
            static_tools.push(Arc::new(files::OpenFile));
            #[cfg(windows)]
            static_tools.push(Arc::new(shell::RunPowershell::new(&cfg.high_risk_patterns)));
            #[cfg(unix)]
            static_tools.push(Arc::new(shell::RunShell::new(&cfg.high_risk_patterns)));
            static_tools.push(Arc::new(volume::GetVolume));
            static_tools.push(Arc::new(volume::SetVolume));
            static_tools.push(Arc::new(web::WebSearch::new(&cfg.web)));
            static_tools.push(Arc::new(web::FetchPage::new(&cfg.web)));
            static_tools.push(Arc::new(memory::Remember::new(memory.clone())));
            static_tools.push(Arc::new(memory::Recall::new(memory.clone())));
            static_tools.push(Arc::new(memory::Forget::new(memory.clone())));
            static_tools.push(Arc::new(translate::Translate::new(&cfg.translate)));
            static_tools.push(Arc::new(media::MediaControl));
            if let Some(shared) = &music {
                static_tools.push(Arc::new(music::StopMusic {
                    shared: shared.clone(),
                }));
            }
            static_tools.push(Arc::new(reminders::CreateReminder::new(
                reminder_store.clone(),
                &cfg.reminders,
            )));
            static_tools.push(Arc::new(reminders::ListReminders::new(
                reminder_store.clone(),
            )));
            static_tools.push(Arc::new(reminders::CancelReminder::new(
                reminder_store.clone(),
            )));
            static_tools.push(Arc::new(screen::TakeScreenshot));
            static_tools.push(Arc::new(screen::MouseMove));
            static_tools.push(Arc::new(screen::MouseClick));
            static_tools.push(Arc::new(screen::ClickAt));
            static_tools.push(Arc::new(scripted::CreateTool::new(
                scripted_store.clone(),
                &cfg.scripted_tools,
            )));
            static_tools.push(Arc::new(scripted::ListCustomTools::new(
                scripted_store.clone(),
            )));
            static_tools.push(Arc::new(scripted::DeleteCustomTool::new(
                scripted_store.clone(),
            )));
            static_tools.push(Arc::new(silence::EnterSilenceMode { flag: silence_flag }));

            // Ata la lista manual de `scripted::BUILTIN_NAMES` al registro real:
            // una built-in nueva que no se agregue ahí podría ser ocultada para
            // siempre por una tool personalizada homónima.
            debug_assert!(
                static_tools
                    .iter()
                    .all(|t| scripted::BUILTIN_NAMES.contains(&t.name())),
                "BUILTIN_NAMES desincronizado con las tools registradas"
            );

            if !mcp_servers.is_empty() {
                let reserved: HashSet<String> = scripted::BUILTIN_NAMES
                    .iter()
                    .map(|s| s.to_string())
                    .chain(static_tools.iter().map(|t| t.name().to_string()))
                    .collect();
                static_tools.extend(crate::mcp::discover_tools(mcp_servers, &reserved).await);
            }
        }

        let registry = Self {
            tools: RwLock::new(Vec::new()),
            specs: RwLock::new(Arc::new(Vec::new())),
            static_tools,
            scripted_store: if cfg.enabled {
                Some(scripted_store)
            } else {
                None
            },
            scripted_cfg: cfg.scripted_tools.clone(),
            disabled_tools: cfg.disabled_tools.clone(),
            max_result_chars: cfg.max_tool_result_chars,
        };
        if let Err(e) = registry.reload_scripted().await {
            tracing::warn!(error = %e, "no se pudieron cargar las tools personalizadas al arrancar");
        }
        registry
    }

    /// Recarga las tools scripted desde SQLite y recalcula `tools`/`specs`.
    /// Se puede llamar en caliente (no requiere `&mut self`): la llama
    /// `execute_and_record` justo después de que `create_tool` o
    /// `delete_custom_tool` corren con éxito, así la tool nueva/borrada
    /// queda disponible desde el siguiente turno sin reiniciar Jarvis.
    pub async fn reload_scripted(&self) -> Result<(), ToolError> {
        let mut tools = self.static_tools.clone();
        if let Some(store) = &self.scripted_store {
            for def in store.list().await? {
                tools.push(Arc::new(scripted::ScriptedTool::new(
                    def,
                    &self.scripted_cfg,
                )));
            }
        }
        tools.retain(|t| !self.disabled_tools.iter().any(|d| d == t.name()));
        let specs = Arc::new(
            tools
                .iter()
                .map(|t| ToolSpec {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters: t.parameters_schema(),
                })
                .collect(),
        );
        *self.tools.write().unwrap() = tools;
        *self.specs.write().unwrap() = specs;
        Ok(())
    }

    pub fn specs(&self) -> Arc<Vec<ToolSpec>> {
        self.specs.read().unwrap().clone()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .read()
            .unwrap()
            .iter()
            .find(|t| t.name() == name)
            .cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.read().unwrap().is_empty()
    }

    /// Recorta un resultado en una frontera de carácter válida, anotando el
    /// corte para que el LLM sepa que falta contenido.
    pub fn truncate_result(&self, result: String) -> String {
        truncate_to(result, self.max_result_chars)
    }
}

/// Lee un argumento string obligatorio del objeto de argumentos.
pub fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ToolError::InvalidArgs(format!("falta el parámetro '{key}'")))
}

/// Recorta `result` a `max_chars` conservando las dos puntas. Ver
/// `TRUNCATE_HEAD_PERCENT` para por qué no se queda solo con el principio.
fn truncate_to(result: String, max_chars: usize) -> String {
    let total = result.chars().count();
    if total <= max_chars {
        return result;
    }
    // Con un presupuesto muy chico no hay dos puntas que valga la pena
    // repartir: quedarían dos fragmentos que no dicen nada.
    if max_chars < MIN_CHARS_FOR_SPLIT_TRUNCATION {
        let head: String = result.chars().take(max_chars).collect();
        return format!("{head}\n(...resultado truncado)");
    }

    let head_len = max_chars * TRUNCATE_HEAD_PERCENT / 100;
    let tail_len = max_chars - head_len;
    let omitted = total - max_chars;
    let head: String = result.chars().take(head_len).collect();
    let tail: String = result.chars().skip(total - tail_len).collect();

    format!(
        "{head}\n(...se omitieron {omitted} caracteres del medio, de {total} en total. Volver a \
         ejecutar lo mismo trunca igual: pedí solo el fragmento que necesitás.)\n{tail}"
    )
}

#[cfg(test)]
mod truncate_tests {
    use super::*;

    fn largo(n: usize, c: char) -> String {
        std::iter::repeat_n(c, n).collect()
    }

    #[test]
    fn lo_que_entra_no_se_toca() {
        let corto = "salida breve".to_string();
        assert_eq!(truncate_to(corto.clone(), 3000), corto);
    }

    /// El caso del log: el modelo necesitaba el final de cada SKILL.md y,
    /// como se cortaba, volvía a ejecutar el comando con `tail` gastando una
    /// iteración por intento. Con las dos puntas lo tiene de una.
    #[test]
    fn conserva_el_principio_y_el_final() {
        let original = format!("INICIO{}FINAL", largo(10_000, 'x'));
        let cortado = truncate_to(original, 3000);

        assert!(cortado.starts_with("INICIO"), "se perdió el principio");
        assert!(cortado.ends_with("FINAL"), "se perdió el final");
        assert!(cortado.contains("se omitieron"));
    }

    /// El aviso tiene que decir que reintentar no sirve: sin eso, el modelo
    /// vuelve a pedir lo mismo esperando un resultado distinto.
    #[test]
    fn el_aviso_desalienta_el_reintento_a_ciegas() {
        let cortado = truncate_to(largo(10_000, 'x'), 3000);
        assert!(cortado.contains("Volver a ejecutar lo mismo trunca igual"));
    }

    #[test]
    fn respeta_el_presupuesto() {
        for max in [400, 1000, 3000] {
            let cortado = truncate_to(largo(50_000, 'x'), max);
            let contenido = cortado.chars().count();
            assert!(
                contenido <= max + 200,
                "con max={max} devolvió {contenido} caracteres"
            );
        }
    }

    /// Un presupuesto diminuto no tiene que romper ni producir dos fragmentos
    /// inservibles: cae al corte simple de siempre.
    #[test]
    fn presupuesto_diminuto_cae_al_corte_simple() {
        let cortado = truncate_to(largo(10_000, 'x'), 50);
        assert!(cortado.contains("resultado truncado"));
        assert!(!cortado.contains("se omitieron"));
    }
}
