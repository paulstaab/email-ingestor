use email_ingestor::{
    config::Config,
    error::{Error, Result},
    supervisor,
};
use std::time::Duration;

async fn signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|_| Error::fatal("cannot install shutdown handler"))?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.map_err(|_| Error::fatal("shutdown handler failed")),
            _ = terminate.recv() => Ok(())
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .map_err(|_| Error::fatal("shutdown handler failed"))
}
#[tokio::main]
async fn main() -> std::process::ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("configuration failed: {}", error.reason);
            return std::process::ExitCode::FAILURE;
        }
    };
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(config.log_level.clone())
        .with_target(false)
        .init();
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let work = supervisor::run(config, receiver);
    tokio::pin!(work);
    let result = tokio::select! {
        biased;
        result = &mut work => result,
        result = signal() => {
            match result {
                Err(error) => Err(error),
                Ok(()) => {
                    tracing::info!(event = "shutdown_requested");
                    let _ = shutdown.send(true);
                    match tokio::time::timeout(Duration::from_secs(30), &mut work).await {
                        Ok(result) => result,
                        Err(_) => { tracing::info!(event = "shutdown_budget_expired"); Ok(()) }
                    }
                }
            }
        }
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(event = "terminal_failure", reason = error.reason);
            std::process::ExitCode::FAILURE
        }
    }
}
