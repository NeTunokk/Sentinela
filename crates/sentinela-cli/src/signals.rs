use tracing::warn;

/// Aguarda SIGINT ou SIGTERM (Unix) ou Ctrl-C.
#[cfg(unix)]
pub async fn shutdown() -> &'static str {
    use tokio::signal::unix::{signal, SignalKind};

    let interrupt = signal(SignalKind::interrupt());
    let terminate = signal(SignalKind::terminate());

    match (interrupt, terminate) {
        (Ok(mut interrupt), Ok(mut terminate)) => {
            tokio::select! {
                _ = interrupt.recv() => "SIGINT",
                _ = terminate.recv() => "SIGTERM",
            }
        }
        (interrupt, terminate) => {
            if let Err(error) = interrupt {
                warn!(%error, "não foi possível registrar handler de SIGINT");
            }
            if let Err(error) = terminate {
                warn!(%error, "não foi possível registrar handler de SIGTERM");
            }
            std::future::pending::<()>().await;
            "sinal"
        }
    }
}

/// Aguarda Ctrl-C em plataformas não Unix.
#[cfg(not(unix))]
pub async fn shutdown() -> &'static str {
    match tokio::signal::ctrl_c().await {
        Ok(()) => "Ctrl-C",
        Err(error) => {
            warn!(%error, "não foi possível registrar handler de Ctrl-C");
            std::future::pending::<()>().await;
            "sinal"
        }
    }
}
