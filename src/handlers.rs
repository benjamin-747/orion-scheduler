use axum::{
    extract::State,
    response::{Json, Html, IntoResponse, sse::{Sse, Event}},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::time::interval;

use crate::state::AppState;
use crate::orion_deployer;

/// Image parameters that can be passed via webhook API to override config-based image selection.
#[derive(Debug, Clone, Default)]
pub struct ImageParams {
    pub path: Option<String>,
    pub url: Option<String>,
    pub digest: Option<String>,
    pub disk_gb: Option<u32>,
    pub cpus: Option<u32>,
    pub memory_mb: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct WebhookResponse {
    pub status: String,
    pub vm_id: Option<String>,
    pub error: Option<String>,
    /// Path to the log file (not the contents)
    pub orion_log_file: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GithubWebhookPayload {
    pub action: Option<String>,
    /// Target environment: "aws-gitmega", "aws-gitmono", "gcp-buck2hub" (required)
    pub target: String,
    /// Override image path (local qcow2 file). Overrides default_image from config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_path: Option<String>,
    /// Override image URL (remote HTTPS). Overrides default_image from config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    /// SHA256/SHA512 digest for the image (required when image_path or image_url is set).
    /// Format: "sha256:..." or "sha512:..."
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_digest: Option<String>,
    /// VM disk size in GB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_disk_gb: Option<u32>,
    /// Number of vCPUs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_cpus: Option<u32>,
    /// VM memory in MB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_memory_mb: Option<u32>,
}

/// GET /webhook
pub async fn webhook_get_handler() -> Json<WebhookResponse> {
    Json(WebhookResponse {
        status: "ok".to_string(),
        vm_id: None,
        error: None,
        orion_log_file: None,
    })
}

/// POST /webhook - receives update requests from GitHub Actions
pub async fn webhook_post_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<GithubWebhookPayload>,
) -> impl IntoResponse {
    tracing::info!("Received webhook: action={:?}, target={}", payload.action, payload.target);

    let image_params = ImageParams {
        path: payload.image_path.clone(),
        url: payload.image_url.clone(),
        digest: payload.image_digest.clone(),
        disk_gb: payload.image_disk_gb,
        cpus: payload.image_cpus,
        memory_mb: payload.image_memory_mb,
    };

    // Spawn the VM operation in a blocking task
    let target = payload.target.clone();
    let result = tokio::task::spawn_blocking(move || {
        // Use blocking synchronous call since VM operations are CPU-heavy
        let rt = tokio::runtime::Handle::current();
        rt.block_on(orion_deployer::handle_update(&state, &target, Some(image_params)))
    }).await;

    match result {
        Ok(Ok(vm_id)) => {
            tracing::info!("Successfully created VM: {}", vm_id);
            let response = WebhookResponse {
                status: "ok".to_string(),
                vm_id: Some(vm_id),
                error: None,
                orion_log_file: None,
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to handle update: {:?}", e);
            let response = WebhookResponse {
                status: "error".to_string(),
                vm_id: None,
                error: Some(e.to_string()),
                orion_log_file: None,
            };
            (StatusCode::INTERNAL_SERVER_ERROR, Json(response)).into_response()
        }
        Err(e) => {
            tracing::error!("Task join error: {:?}", e);
            let response = WebhookResponse {
                status: "error".to_string(),
                vm_id: None,
                error: Some(e.to_string()),
                orion_log_file: None,
            };
            (StatusCode::INTERNAL_SERVER_ERROR, Json(response)).into_response()
        }
    }
}

/// GET /health
pub async fn health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "healthy",
        "service": "orion-scheduler"
    }))
}

/// GET /status
pub async fn status_handler(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    match orion_deployer::get_status(&state).await {
        Some(vm) => Json(serde_json::json!({
            "status": "running",
            "vm_id": vm.id,
            "vm_ip": vm.ip,
            "uptime_secs": vm.created_at.elapsed().as_secs(),
            "log_file": vm.log_file
        })),
        None => Json(serde_json::json!({
            "status": "no_vm",
            "vm_id": null
        })),
    }
}

/// GET /logs/orion - Read Orion logs with formatting (text output for terminal)
pub async fn logs_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match orion_deployer::get_status(&state).await {
        Some(vm) => {
            if let Some(log_file) = &vm.log_file {
                match tokio::fs::read_to_string(log_file).await {
                    Ok(contents) => {
                        let formatted = format_logs(&contents);
                        Html(formatted).into_response()
                    }
                    Err(e) => {
                        let response = serde_json::json!({
                            "status": "error",
                            "error": format!("Failed to read log file: {}", e)
                        });
                        (StatusCode::INTERNAL_SERVER_ERROR, Json(response)).into_response()
                    }
                }
            } else {
                let response = serde_json::json!({
                    "status": "no_log_file",
                    "error": "No log file available for this VM"
                });
                (StatusCode::NOT_FOUND, Json(response)).into_response()
            }
        }
        None => {
            let response = serde_json::json!({
                "status": "no_vm",
                "error": "No VM is currently running"
            });
            (StatusCode::NOT_FOUND, Json(response)).into_response()
        }
    }
}

/// Format logs for terminal display with colors and sections
fn format_logs(logs: &str) -> String {
    let mut output = String::new();
    output.push_str("\n╔══════════════════════════════════════════════════════════════════════════════╗\n");
    output.push_str("║                        ORION STARTUP LOGS                                  ║\n");
    output.push_str("╚══════════════════════════════════════════════════════════════════════════════╝\n\n");

    let lines: Vec<&str> = logs.lines().collect();

    for line in &lines {
        let formatted = format_log_line(line);
        output.push_str(&formatted);
        output.push('\n');
    }

    output.push_str("\n╔══════════════════════════════════════════════════════════════════════════════╗\n");
    output.push_str("║                         END OF LOGS                                        ║\n");
    output.push_str("╚══════════════════════════════════════════════════════════════════════════════╝\n");

    output
}

/// Format a single log line with colors based on content type
fn format_log_line(line: &str) -> String {
    // Remove ANSI escape codes for clean formatting
    let clean_line = strip_ansi(line);

    // Determine line type and color
    if clean_line.contains("preflight.sh") || clean_line.contains("预检") {
        format!("  🔍 {}", colorize(&clean_line, "cyan"))
    } else if clean_line.contains("cleanup.sh") || clean_line.contains("清理") {
        format!("  🧹 {}", colorize(&clean_line, "yellow"))
    } else if clean_line.contains("systemd") || clean_line.contains("Started") {
        format!("  ✅ {}", colorize(&clean_line, "green"))
    } else if clean_line.contains("ORION_WORKER_ID") || clean_line.contains("Worker ID") {
        format!("  🆔 {}", colorize(&clean_line, "magenta"))
    } else if clean_line.contains("WebSocket") || clean_line.contains("Connecting") {
        format!("  🌐 {}", colorize(&clean_line, "blue"))
    } else if clean_line.contains("Antares") || clean_line.contains("Dicfuse") {
        format!("  📦 {}", colorize(&clean_line, "bright_blue"))
    } else if clean_line.contains("ERROR") || clean_line.contains("error") {
        format!("  ❌ {}", colorize(&clean_line, "red"))
    } else if clean_line.contains("WARN") || clean_line.contains("warn") {
        format!("  ⚠️  {}", colorize(&clean_line, "yellow"))
    } else if clean_line.contains("INFO") || clean_line.contains("info") {
        format!("  ℹ️  {}", colorize(&clean_line, "white"))
    } else if clean_line.starts_with("==>") {
        format!("  ▶️  {}", colorize(&clean_line, "bright_white"))
    } else if clean_line.contains("DEBUG") {
        format!("  🔧 {}", colorize(&clean_line, "dim"))
    } else if clean_line.is_empty() {
        "  ".to_string()
    } else {
        format!("  │  {}", clean_line)
    }
}

/// Apply ANSI color code to text
/// Colors: red, green, yellow, blue, magenta, cyan, white, bright_white, bright_blue, dim
fn colorize(text: &str, color: &str) -> String {
    let code = match color {
        "red" => "31",
        "green" => "32",
        "yellow" => "33",
        "blue" => "34",
        "magenta" => "35",
        "cyan" => "36",
        "white" => "37",
        "bright_white" => "97",
        "bright_blue" => "94",
        "dim" => "90",
        _ => "37",
    };
    format!("\x1b[{}m{}\x1b[0m", code, text)
}

/// Remove ANSI escape sequences (color codes) from text for clean formatting
fn strip_ansi(text: &str) -> String {
    let mut result = String::new();
    let chars = text.chars().collect::<Vec<_>>();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\x1b' && i + 1 < chars.len() && chars[i + 1] == '[' {
            // Skip until end of ANSI sequence
            i += 2;
            while i < chars.len() && !chars[i].is_ascii_alphabetic() {
                i += 1;
            }
            i += 1; // Skip the final letter
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

/// GET /logs/orion/live - Get live Orion logs from running VM
pub async fn logs_live_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match orion_deployer::get_live_logs(&state).await {
        Ok(logs) => {
            let response = serde_json::json!({
                "status": "ok",
                "logs": logs
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "status": "error",
                "error": e.to_string()
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(response)).into_response()
        }
    }
}

/// GET /scorpio/status - Check Scorpio mount status and directories
pub async fn scorpio_status_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match orion_deployer::get_scorpio_status(&state).await {
        Ok(status) => {
            (StatusCode::OK, Json(status)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "status": "error",
                "error": e.to_string()
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(response)).into_response()
        }
    }
}

/// GET /scorpio/config - Read scorpio.toml content from VM
pub async fn scorpio_config_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let machine = match state.get_machine().await {
        Some(m) => m,
        None => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                "status": "error",
                "error": "No VM is currently running"
            }))).into_response()
        }
    };

    match machine.exec("cat /home/orion/orion-runner/scorpio.toml").await {
        Ok(output) => {
            let content = String::from_utf8_lossy(&output.stdout).to_string();
            (StatusCode::OK, Json(serde_json::json!({
                "status": "ok",
                "path": "/home/orion/orion-runner/scorpio.toml",
                "content": content
            }))).into_response()
        }
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                "status": "error",
                "error": e.to_string()
            }))).into_response()
        }
    }
}

/// POST /shutdown - Shutdown VM only, server keeps running
pub async fn shutdown_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    tracing::info!("[http-shutdown] Received shutdown request via HTTP");

    // Shutdown VM if running
    if let Some(machine) = state.get_machine().await {
        tracing::info!("[http-shutdown] VM found, calling shutdown...");
        match machine.shutdown().await {
            Ok(_) => tracing::info!("[http-shutdown] VM shutdown completed successfully"),
            Err(e) => tracing::error!("[http-shutdown] VM shutdown failed: {}", e),
        }
    } else {
        tracing::info!("[http-shutdown] No VM running");
    }
    state.clear_vm().await;

    let response = serde_json::json!({
        "status": "ok",
        "message": "VM stopped, server is still running"
    });
    (StatusCode::OK, Json(response)).into_response()
}

/// GET /logs/orion/stream - SSE stream for real-time log viewing
/// First connect sends last 50 lines, then only new lines
pub async fn logs_stream_handler(
    State(state): State<Arc<AppState>>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    // Create a stream that yields only new log lines every 2 seconds
    let stream = async_stream::stream! {
        let mut ticker = interval(std::time::Duration::from_secs(2));
        let mut last_log_len: usize = 0;
        let mut last_journal_len: usize = 0;
        let mut is_initial = true;

        loop {
            ticker.tick().await;
            match orion_deployer::get_live_logs(&state).await {
                Ok(full_logs) => {
                    // Split into journalctl part and orion.log part
                    let (journal_part, orion_part) = split_logs(&full_logs);

                    // On first connect, only show last 50 lines to avoid overwhelming
                    let (new_journal_lines, new_orion_lines) = if is_initial {
                        is_initial = false;
                        // Take last 50 lines from each section
                        let jlines: Vec<&str> = journal_part.lines().collect();
                        let jstart = if jlines.len() > 50 { jlines.len() - 50 } else { 0 };
                        let jslice = jlines[jstart..].join("\n");

                        let olines: Vec<&str> = orion_part.lines().collect();
                        let ostart = if olines.len() > 50 { olines.len() - 50 } else { 0 };
                        let oslice = olines[ostart..].join("\n");

                        (jslice, oslice)
                    } else {
                        // Send only new lines since last check
                        let new_j = if journal_part.len() > last_journal_len {
                            journal_part[last_journal_len..].to_string()
                        } else {
                            String::new()
                        };
                        let new_o = if orion_part.len() > last_log_len {
                            orion_part[last_log_len..].to_string()
                        } else {
                            String::new()
                        };
                        (new_j, new_o)
                    };

                    last_journal_len = journal_part.len();
                    last_log_len = orion_part.len();

                    // Only send if there are new lines
                    if !new_journal_lines.is_empty() || !new_orion_lines.is_empty() {
                        let mut output = String::new();

                        if !new_journal_lines.is_empty() {
                            output.push_str(&format_logs_section("SYSTEM LOGS", &new_journal_lines));
                        }
                        if !new_orion_lines.is_empty() {
                            output.push_str(&format_logs_section("ORION LOGS", &new_orion_lines));
                        }

                        yield Ok(Event::default().comment("---").data(output))
                    } else if !is_initial {
                        // Silent on no new logs - don't send anything
                    }
                }
                Err(e) => {
                    yield Ok(Event::default().data(format!("Error: {}", e)))
                }
            }
        }
    };

    Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
}

/// Split combined logs into systemd journal and Orion log file sections
/// The separator is "========== Orion Log"
fn split_logs(full_logs: &str) -> (&str, &str) {
    // Find the separator "========== Orion Log"
    if let Some(pos) = full_logs.find("========== Orion Log") {
        let journal = &full_logs[..pos];
        let orion = &full_logs[pos..];
        return (journal, orion);
    }
    (full_logs, "")
}

/// Format a log section with a title header and colored log lines
fn format_logs_section(title: &str, content: &str) -> String {
    let mut output = String::new();
    output.push_str(&format!("\n─── {} ───\n", title));
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            output.push_str(&format_log_line(trimmed));
            output.push('\n');
        }
    }
    output
}