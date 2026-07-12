//! Herramientas de solo lectura sobre el sistema: fecha/hora, estado
//! general (CPU/RAM/batería) y procesos. Todas son `RiskLevel::Safe`.

use async_trait::async_trait;
use chrono::{Datelike, Local, Timelike};
use serde_json::{json, Value};

use crate::errors::ToolError;

use super::{RiskLevel, Tool, ToolOutput};

const DIAS: [&str; 7] = [
    "lunes",
    "martes",
    "miércoles",
    "jueves",
    "viernes",
    "sábado",
    "domingo",
];
const MESES: [&str; 12] = [
    "enero",
    "febrero",
    "marzo",
    "abril",
    "mayo",
    "junio",
    "julio",
    "agosto",
    "septiembre",
    "octubre",
    "noviembre",
    "diciembre",
];

/// "viernes 10 de julio de 2026, 14:32" — para el system prompt y la tool.
pub fn fecha_hora_es() -> String {
    let now = Local::now();
    let dia = DIAS[now.weekday().num_days_from_monday() as usize];
    let mes = MESES[now.month0() as usize];
    format!(
        "{dia} {} de {mes} de {}, {:02}:{:02}",
        now.day(),
        now.year(),
        now.hour(),
        now.minute()
    )
}

pub struct GetDatetime;

#[async_trait]
impl Tool for GetDatetime {
    fn name(&self) -> &'static str {
        "get_datetime"
    }

    fn description(&self) -> &'static str {
        "Devuelve la fecha y hora local actual. Úsala para cálculos de fechas \
         (qué día cae en dos semanas, cuántos días faltan para una fecha)."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, _args: &Value) -> String {
        "consultar la fecha y hora".to_string()
    }

    async fn execute(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        let now = Local::now();
        Ok(ToolOutput::text(format!(
            "Ahora es {} (fecha ISO: {}).",
            fecha_hora_es(),
            now.format("%Y-%m-%d %H:%M:%S")
        )))
    }
}

pub struct SystemStatus;

#[async_trait]
impl Tool for SystemStatus {
    fn name(&self) -> &'static str {
        "system_status"
    }

    fn description(&self) -> &'static str {
        "Estado general de la computadora: uso de CPU, memoria RAM usada y \
         total, y nivel de batería si existe."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, _args: &Value) -> String {
        "consultar el estado del sistema".to_string()
    }

    async fn execute(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        tokio::task::spawn_blocking(|| {
            use sysinfo::System;
            let mut sys = System::new();
            sys.refresh_memory();
            // La medición de CPU necesita dos muestras separadas en el tiempo.
            sys.refresh_cpu_usage();
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
            sys.refresh_cpu_usage();

            let cpu = sys.global_cpu_usage();
            let used_gb = sys.used_memory() as f64 / 1e9;
            let total_gb = sys.total_memory() as f64 / 1e9;
            let mut lines = vec![
                format!("CPU: {cpu:.0}% de uso."),
                format!(
                    "RAM: {used_gb:.1} de {total_gb:.1} GB en uso ({:.0}%).",
                    used_gb / total_gb * 100.0
                ),
            ];
            lines.push(battery_line());
            lines.join("\n")
        })
        .await
        .map(|text| Ok(ToolOutput::text(text)))
        .unwrap_or_else(|e| Err(ToolError::Execution(e.to_string())))
    }
}

fn battery_line() -> String {
    let manager = match starship_battery::Manager::new() {
        Ok(m) => m,
        Err(e) => return format!("Batería: no se pudo consultar ({e})."),
    };
    let mut batteries = match manager.batteries() {
        Ok(b) => b.peekable(),
        Err(e) => return format!("Batería: no se pudo consultar ({e})."),
    };
    match batteries.next() {
        Some(Ok(battery)) => {
            let pct = battery.state_of_charge().value * 100.0;
            let estado = match battery.state() {
                starship_battery::State::Charging => "cargando",
                starship_battery::State::Discharging => "descargando",
                starship_battery::State::Full => "carga completa",
                _ => "estado desconocido",
            };
            format!("Batería: {pct:.0}% ({estado}).")
        }
        Some(Err(e)) => format!("Batería: no se pudo leer ({e})."),
        None => "Batería: no hay (equipo de escritorio).".to_string(),
    }
}

pub struct ListProcesses;

#[async_trait]
impl Tool for ListProcesses {
    fn name(&self) -> &'static str {
        "list_processes"
    }

    fn description(&self) -> &'static str {
        "Lista los procesos que más recursos consumen. Parámetro sort_by: \
         'cpu' o 'memory' (default 'cpu')."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "sort_by": {
                    "type": "string",
                    "enum": ["cpu", "memory"],
                    "description": "Criterio de orden: cpu o memory"
                }
            }
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, _args: &Value) -> String {
        "listar los procesos activos".to_string()
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let by_memory = args.get("sort_by").and_then(Value::as_str) == Some("memory");
        tokio::task::spawn_blocking(move || {
            use sysinfo::System;
            let mut sys = System::new_all();
            if !by_memory {
                std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
                sys.refresh_all();
            }

            // Agrupa por nombre: un navegador con 40 procesos debe contar una vez.
            let mut grouped: std::collections::HashMap<String, (f32, u64)> =
                std::collections::HashMap::new();
            for process in sys.processes().values() {
                let name = process.name().to_string_lossy().to_string();
                let entry = grouped.entry(name).or_insert((0.0, 0));
                entry.0 += process.cpu_usage();
                entry.1 += process.memory();
            }
            let mut rows: Vec<(String, f32, u64)> = grouped
                .into_iter()
                .map(|(name, (cpu, mem))| (name, cpu, mem))
                .collect();
            if by_memory {
                rows.sort_by(|a, b| b.2.cmp(&a.2));
            } else {
                rows.sort_by(|a, b| b.1.total_cmp(&a.1));
            }

            let criterio = if by_memory { "memoria" } else { "CPU" };
            let mut out = format!("Top 8 procesos por {criterio}:\n");
            for (name, cpu, mem) in rows.into_iter().take(8) {
                out.push_str(&format!(
                    "- {name}: CPU {cpu:.0}%, RAM {:.1} GB\n",
                    mem as f64 / 1e9
                ));
            }
            out
        })
        .await
        .map(|text| Ok(ToolOutput::text(text)))
        .unwrap_or_else(|e| Err(ToolError::Execution(e.to_string())))
    }
}
