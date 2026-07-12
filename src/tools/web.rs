//! Herramientas web: búsqueda vía DuckDuckGo (scraping del endpoint HTML,
//! sin API key, con fallback automático al endpoint "lite" si el markup no
//! matchea) y lectura de páginas con extracción de texto legible.
//!
//! Nota de fiabilidad: el scraping de DDG es suficientemente estable para
//! uso personal pero puede romperse si cambian el HTML — por eso el doble
//! endpoint. Ambas tools son de solo lectura → `Safe`.

use std::time::Duration;

use async_trait::async_trait;
use scraper::{Html, Selector};
use serde_json::{json, Value};

use crate::config::WebToolConfig;
use crate::errors::ToolError;

use super::{required_str, RiskLevel, Tool, ToolOutput};

const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

fn build_client(cfg: &WebToolConfig) -> reqwest::Client {
    crate::http::client_builder(FETCH_TIMEOUT)
        .user_agent(cfg.user_agent.clone())
        .build()
        .expect("configuración de cliente reqwest válida")
}

struct SearchHit {
    title: String,
    url: String,
    snippet: String,
}

/// Los resultados de DDG html vienen como redirects
/// (`//duckduckgo.com/l/?uddg=<url codificada>`); se extrae la URL real.
fn decode_ddg_href(href: &str) -> String {
    let absolute = if href.starts_with("//") {
        format!("https:{href}")
    } else {
        href.to_string()
    };
    if let Ok(parsed) = url::Url::parse(&absolute) {
        if parsed.path().starts_with("/l/") {
            if let Some((_, real)) = parsed.query_pairs().find(|(k, _)| k == "uddg") {
                return real.into_owned();
            }
        }
    }
    absolute
}

fn clean_text(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parser del endpoint `html.duckduckgo.com/html`.
fn parse_ddg_html(body: &str, max: usize) -> Vec<SearchHit> {
    let doc = Html::parse_document(body);
    let result_sel = Selector::parse(".result").unwrap();
    let link_sel = Selector::parse("a.result__a").unwrap();
    let snippet_sel = Selector::parse(".result__snippet").unwrap();

    let mut hits = Vec::new();
    for result in doc.select(&result_sel).take(max * 2) {
        let Some(link) = result.select(&link_sel).next() else {
            continue;
        };
        let title = clean_text(&link.text().collect::<String>());
        let href = link.value().attr("href").unwrap_or_default();
        if title.is_empty() || href.is_empty() {
            continue;
        }
        let snippet = result
            .select(&snippet_sel)
            .next()
            .map(|s| clean_text(&s.text().collect::<String>()))
            .unwrap_or_default();
        hits.push(SearchHit {
            title,
            url: decode_ddg_href(href),
            snippet,
        });
        if hits.len() >= max {
            break;
        }
    }
    hits
}

/// Parser del endpoint `lite.duckduckgo.com/lite` (tabla plana: el snippet
/// va en una fila aparte, a continuación del link).
fn parse_ddg_lite(body: &str, max: usize) -> Vec<SearchHit> {
    let doc = Html::parse_document(body);
    let link_sel = Selector::parse("a.result-link").unwrap();
    let snippet_sel = Selector::parse("td.result-snippet").unwrap();

    let links: Vec<_> = doc.select(&link_sel).take(max).collect();
    let snippets: Vec<String> = doc
        .select(&snippet_sel)
        .take(max)
        .map(|s| clean_text(&s.text().collect::<String>()))
        .collect();

    links
        .into_iter()
        .enumerate()
        .filter_map(|(i, link)| {
            let title = clean_text(&link.text().collect::<String>());
            let href = link.value().attr("href")?;
            if title.is_empty() {
                return None;
            }
            Some(SearchHit {
                title,
                url: decode_ddg_href(href),
                snippet: snippets.get(i).cloned().unwrap_or_default(),
            })
        })
        .collect()
}

fn format_hits(query: &str, hits: &[SearchHit]) -> String {
    let mut out = format!("Resultados de la búsqueda '{query}':\n");
    for (i, hit) in hits.iter().enumerate() {
        out.push_str(&format!("{}. {} — {}\n", i + 1, hit.title, hit.url));
        if !hit.snippet.is_empty() {
            out.push_str(&format!("   {}\n", hit.snippet));
        }
    }
    out.push_str("(Usa fetch_page con una URL si necesitas el contenido completo.)");
    out
}

pub struct WebSearch {
    client: reqwest::Client,
    cfg: WebToolConfig,
}

impl WebSearch {
    pub fn new(cfg: &WebToolConfig) -> Self {
        Self {
            client: build_client(cfg),
            cfg: cfg.clone(),
        }
    }

    async fn get_body(&self, url: &str) -> Result<String, ToolError> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("error de red: {e}")))?;
        if !response.status().is_success() {
            return Err(ToolError::Execution(format!(
                "el buscador respondió {}",
                response.status()
            )));
        }
        response
            .text()
            .await
            .map_err(|e| ToolError::Execution(format!("error leyendo la respuesta: {e}")))
    }
}

#[async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "Busca en la web y devuelve títulos, URLs y resúmenes de los primeros \
         resultados. Para información actual que no conoces (noticias, datos \
         recientes, precios)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Términos de búsqueda"
                }
            },
            "required": ["query"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, args: &Value) -> String {
        let query = args.get("query").and_then(Value::as_str).unwrap_or("?");
        format!("buscar en la web '{query}'")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let query = required_str(&args, "query")?;
        let max = self.cfg.max_results;

        let encoded: String =
            url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        let html_url = format!("https://html.duckduckgo.com/html/?q={encoded}");
        let body = self.get_body(&html_url).await?;
        let hits = parse_ddg_html(&body, max);
        if !hits.is_empty() {
            return Ok(ToolOutput::text(format_hits(query, &hits)));
        }

        // El endpoint html a veces sirve un captcha o variante: probar lite.
        tracing::info!("DDG html sin resultados parseables, probando endpoint lite");
        let lite_url = format!("https://lite.duckduckgo.com/lite/?q={encoded}");
        let body = self.get_body(&lite_url).await?;
        let hits = parse_ddg_lite(&body, max);
        if hits.is_empty() {
            return Ok(ToolOutput::text(format!(
                "La búsqueda de '{query}' no devolvió resultados (o el buscador \
                 bloqueó la consulta)."
            )));
        }
        Ok(ToolOutput::text(format_hits(query, &hits)))
    }
}

pub struct FetchPage {
    client: reqwest::Client,
    cfg: WebToolConfig,
}

impl FetchPage {
    pub fn new(cfg: &WebToolConfig) -> Self {
        Self {
            client: build_client(cfg),
            cfg: cfg.clone(),
        }
    }
}

/// Extrae el texto legible de un documento HTML: prioriza `<article>`/
/// `<main>` y junta párrafos y encabezados — lo que naturalmente descarta
/// scripts, menús y pies de página.
fn extract_readable_text(body: &str) -> (String, String) {
    let doc = Html::parse_document(body);
    let title = Selector::parse("title")
        .ok()
        .and_then(|sel| doc.select(&sel).next())
        .map(|t| clean_text(&t.text().collect::<String>()))
        .unwrap_or_default();

    let content_sel = Selector::parse("article, main").unwrap();
    let text_sel = Selector::parse("p, h1, h2, h3, li").unwrap();

    let mut parts: Vec<String> = Vec::new();
    if let Some(container) = doc.select(&content_sel).next() {
        for node in container.select(&text_sel) {
            let text = clean_text(&node.text().collect::<String>());
            if !text.is_empty() {
                parts.push(text);
            }
        }
    } else {
        for node in doc.select(&text_sel) {
            let text = clean_text(&node.text().collect::<String>());
            if text.split(' ').count() >= 4 {
                // Sin <article>/<main>, filtrar fragmentos cortos de menús.
                parts.push(text);
            }
        }
    }
    (title, parts.join("\n"))
}

#[async_trait]
impl Tool for FetchPage {
    fn name(&self) -> &'static str {
        "fetch_page"
    }

    fn description(&self) -> &'static str {
        "Descarga una página web y devuelve su texto legible (sin menús ni \
         scripts). Úsala tras web_search para leer un resultado."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL completa de la página (https://...)"
                }
            },
            "required": ["url"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, args: &Value) -> String {
        let url = args.get("url").and_then(Value::as_str).unwrap_or("?");
        format!("leer la página {url}")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let page_url = required_str(&args, "url")?;
        url::Url::parse(page_url)
            .map_err(|_| ToolError::InvalidArgs(format!("URL inválida: {page_url}")))?;

        let response = self
            .client
            .get(page_url)
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("error de red: {e}")))?;
        if !response.status().is_success() {
            return Err(ToolError::Execution(format!(
                "la página respondió {}",
                response.status()
            )));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| ToolError::Execution(format!("error descargando: {e}")))?;
        let body = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_BODY_BYTES)]).into_owned();

        let (title, text) = extract_readable_text(&body);
        if text.trim().is_empty() {
            return Ok(ToolOutput::text(format!(
                "La página {page_url} no tiene texto legible extraíble \
                 (puede ser una app dinámica)."
            )));
        }
        let truncated: String = text.chars().take(self.cfg.max_page_chars).collect();
        let suffix = if truncated.len() < text.len() {
            "\n(...contenido truncado)"
        } else {
            ""
        };
        Ok(ToolOutput::text(if title.is_empty() {
            format!("{truncated}{suffix}")
        } else {
            format!("Título: {title}\n\n{truncated}{suffix}")
        }))
    }
}
