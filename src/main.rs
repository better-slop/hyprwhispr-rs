use anyhow::{Context, Result};
use hyprwhspr_rs::{
    logging::TextPipelineFormatter,
    whisper::{GroqClient, LocalWhisper, Transcriber},
    BackendKind, Config, ConfigManager, HyprwhsprApp,
};
use std::env;
use tokio::signal;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hyprwhspr=info".into()),
        )
        .with(tracing_subscriber::fmt::layer().event_format(TextPipelineFormatter::new()))
        .init();

    // Check for test mode
    let args: Vec<String> = env::args().collect();
    let test_mode = args.iter().any(|arg| arg == "--test");
    let cli_groq = args.iter().any(|arg| arg == "--groq");

    if test_mode {
        return run_test_mode(cli_groq).await;
    }

    info!("🚀 hyprwhspr-rs starting up!");
    info!("{}", "=".repeat(50));

    // Load configuration
    let config_manager = ConfigManager::load()?;
    config_manager.start_watching();
    let config = config_manager.get();
    info!("✅ Configuration loaded");
    info!("   Model: {}", config.model);
    if let Some(shortcut) = config.press_shortcut() {
        info!("   Press shortcut: {}", shortcut);
    } else {
        info!("   Press shortcut: disabled");
    }
    if let Some(shortcut) = config.hold_shortcut() {
        info!("   Hold shortcut: {}", shortcut);
    } else {
        info!("   Hold shortcut: disabled");
    }
    info!("   Audio feedback: {}", config.audio_feedback);

    // Initialize application
    let backend_override = cli_groq;
    let backend_kind = if backend_override || config.use_groq {
        BackendKind::Groq
    } else {
        BackendKind::Local
    };

    if backend_override {
        info!("CLI flag --groq provided; using Groq backend");
    }

    let transcriber = make_transcriber(&config_manager, &config, backend_kind)
        .context("Failed to initialize transcription backend")?;

    let app = HyprwhsprApp::new(config_manager, transcriber, backend_kind, backend_override)?;

    // Set up signal handling
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

    #[cfg(unix)]
    {
        tokio::spawn(async move {
            let ctrl_c = signal::ctrl_c();
            let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("Failed to set up SIGTERM handler");

            tokio::select! {
                _ = ctrl_c => {
                    info!("Received SIGINT (Ctrl+C)");
                }
                _ = sigterm.recv() => {
                    info!("Received SIGTERM");
                }
            }

            let _ = shutdown_tx.send(());
        });
    }

    #[cfg(not(unix))]
    {
        tokio::spawn(async move {
            signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
            info!("Received SIGINT (Ctrl+C)");
            let _ = shutdown_tx.send(());
        });
    }

    // Run app until shutdown signal
    tokio::select! {
        result = app.run() => {
            if let Err(e) = result {
                info!("App error: {}", e);
            }
        }
        _ = shutdown_rx => {
            info!("Shutdown signal received");
        }
    }

    // Cleanup
    info!("🛑 Shutting down hyprwhspr-rs...");
    info!("✅ Shutdown complete");

    Ok(())
}

fn make_transcriber(
    config_manager: &ConfigManager,
    config: &Config,
    backend_kind: BackendKind,
) -> Result<Box<dyn Transcriber>> {
    match backend_kind {
        BackendKind::Local => {
            let vad_options = config_manager.build_vad_options(config);
            let local = LocalWhisper::new(
                config_manager.get_model_path(),
                config_manager.get_whisper_binary_path(),
                config.threads,
                config.whisper_prompt.clone(),
                config_manager.get_temp_dir(),
                config.gpu_layers,
                vad_options,
                config.no_speech_threshold,
            )?;
            local.initialize()?;
            info!("Backend: LocalWhisper");
            Ok(Box::new(local))
        }
        BackendKind::Groq => {
            let client = GroqClient::new().context("Failed to initialize Groq client")?;
            info!("Backend: Groq (model=whisper-large-v3)");
            Ok(Box::new(client))
        }
    }
}

async fn run_test_mode(cli_groq: bool) -> Result<()> {
    use hyprwhspr_rs::app_test::HyprwhsprAppTest;
    use tokio::io::{AsyncBufReadExt, BufReader};

    info!("🧪 Test Mode - Press Enter to toggle recording, Ctrl+C to quit");
    info!("{}", "=".repeat(50));

    // Load configuration
    let config_manager = ConfigManager::load()?;
    config_manager.start_watching();
    let mut config_rx = config_manager.subscribe();
    let config = config_manager.get();
    info!("✅ Configuration loaded");
    info!("   Model: {}", config.model);
    info!("   Audio feedback: {}", config.audio_feedback);

    let backend_override = cli_groq;
    let backend_kind = if backend_override || config.use_groq {
        BackendKind::Groq
    } else {
        BackendKind::Local
    };

    if backend_override {
        info!("CLI flag --groq provided; using Groq backend");
    }

    let transcriber = make_transcriber(&config_manager, &config, backend_kind)
        .context("Failed to initialize transcription backend")?;

    // Initialize application
    let mut app =
        HyprwhsprAppTest::new(config_manager, transcriber, backend_kind, backend_override)?;

    info!("");
    info!("📝 Instructions:");
    info!("   1. Press Enter to START recording");
    info!("   2. Speak something");
    info!("   3. Press Enter to STOP recording");
    info!("   4. Text will be transcribed and injected");
    info!("");

    // Set up stdin reader
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();

    // Set up signal handling
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
        info!("Received SIGINT (Ctrl+C)");
        let _ = shutdown_tx.send(());
    });

    // Main loop
    loop {
        tokio::select! {
            line = reader.next_line() => {
                match line {
                    Ok(Some(_)) => {
                        // Toggle recording on Enter
                        if let Err(e) = app.toggle_recording().await {
                            info!("Error: {}", e);
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        info!("Error reading input: {}", e);
                        break;
                    }
                }
            }
            result = config_rx.changed() => {
                match result {
                    Ok(()) => {
                        let updated = config_rx.borrow().clone();
                        if let Err(err) = app.apply_config_update(updated) {
                            info!("Failed to apply config update: {}", err);
                        }
                    }
                    Err(_) => {
                        info!("Configuration watcher closed");
                        break;
                    }
                }
            }
            _ = &mut shutdown_rx => {
                info!("Shutdown signal received");
                break;
            }
        }
    }

    // Cleanup
    info!("🛑 Shutting down test mode...");
    app.cleanup().await?;
    info!("✅ Shutdown complete");

    Ok(())
}
