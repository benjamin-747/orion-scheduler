mod config;
mod handlers;
mod keep_alive;
mod orion_deployer;
mod state;
mod vm_manager;

use axum::Router;
use std::sync::Arc;
use tokio::signal::{ctrl_c, unix::SignalKind};
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use state::AppState;

/// Gracefully shutdown VM and clear state on service termination signals
async fn shutdown_vm(state: &AppState) {
    tracing::info!("[shutdown] Initiating VM shutdown");
    if let Some(machine) = state.get_machine().await {
        tracing::info!("[shutdown] VM found, calling shutdown...");
        match machine.shutdown().await {
            Ok(_) => tracing::info!("[shutdown] VM shutdown completed successfully"),
            Err(e) => tracing::error!("[shutdown] VM shutdown failed: {}", e),
        }
    } else {
        tracing::info!("[shutdown] No VM running");
    }
    state.clear_vm().await;
    tracing::info!("[shutdown] State cleared");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting orion-scheduler service");

    // Cleanup any residual processes from previous runs
    tracing::info!("[startup] Checking for residual QEMU processes");
    tokio::process::Command::new("pkill")
        .args(["-9", "-f", "qemu-system-x86"])
        .output()
        .await
        .ok(); // Ignore errors if no processes found

    // Load target configuration
    let config_path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "target_config.json".to_string());
    tracing::info!("[startup] Loading config from: {}", config_path);
    let config = config::Config::load(&config_path).await?;
    let config = Arc::new(tokio::sync::RwLock::new(config));
    tracing::info!("[startup] Config loaded, available targets: {:?}", config.read().await.target_names());

    // Create shared state
    let state = Arc::new(AppState::new(config));

    // Build router - use separate routes for GET and POST
    let app = Router::new()
        .route("/webhook", axum::routing::get(handlers::webhook_get_handler))
        .route("/webhook", axum::routing::post(handlers::webhook_post_handler))
        .route("/health", axum::routing::get(handlers::health_handler))
        .route("/status", axum::routing::get(handlers::status_handler))
        .route("/logs/orion/stream", axum::routing::get(handlers::logs_stream_handler))
        .route("/scorpio/status", axum::routing::get(handlers::scorpio_status_handler))
        .route("/scorpio/config", axum::routing::get(handlers::scorpio_config_handler))
        .route("/shutdown", axum::routing::post(handlers::shutdown_handler))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state.clone());

    // Start server
    let addr = "0.0.0.0:8080";
    tracing::info!("[startup] Listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    // Handle termination signals: stop VM and server
    let term_shutdown_state = state.clone();
    let term_shutdown_signal = async move {
        if let Some(()) = tokio::signal::unix::signal(SignalKind::terminate())
            .unwrap()
            .recv()
            .await
        {
            tracing::info!("[shutdown] Received SIGTERM");
            shutdown_vm(&term_shutdown_state).await;
        }
    };

    let quit_shutdown_state = state.clone();
    let quit_shutdown_signal = async move {
        if let Some(()) = tokio::signal::unix::signal(SignalKind::quit())
            .unwrap()
            .recv()
            .await
        {
            tracing::info!("[shutdown] Received SIGQUIT");
            shutdown_vm(&quit_shutdown_state).await;
        }
    };

    // Handle Ctrl+C: stop VM and server
    let ctrl_c_shutdown_state = state.clone();
    let ctrl_c_signal = async move {
        match ctrl_c().await {
            Ok(()) => {
                tracing::info!("[shutdown] Received SIGINT (Ctrl+C)");
                shutdown_vm(&ctrl_c_shutdown_state).await;
            }
            Err(e) => tracing::error!("[shutdown] Ctrl+C handler error: {}", e),
        }
    };

    tracing::info!("[startup] Server running. Use /shutdown to stop VM only");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::select! {
                _ = ctrl_c_signal => {}
                _ = term_shutdown_signal => {}
                _ = quit_shutdown_signal => {}
            }
        })
        .await?;

    tracing::info!("[shutdown] Server exiting");
    Ok(())
}