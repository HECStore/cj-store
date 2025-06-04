use std::fs::File;
use tokio::sync::mpsc;
use tracing::{debug, error, info};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

/// Message sent to the logger with a level and content.
#[derive(Debug)]
pub struct LogMessage {
    pub level: tracing::Level,
    pub message: String,
}

/// Logger that processes LogMessage events.
pub struct Logger {
    receiver: mpsc::Receiver<LogMessage>,
}

impl Logger {
    /// Creates a new Logger with the given receiver.
    pub fn new(receiver: mpsc::Receiver<LogMessage>) -> Self {
        Logger { receiver }
    }

    /// Runs the logger, processing messages and writing to file and stdout.
    pub async fn run(mut self) {
        // Create log file
        let file = File::create("store.log").expect("Failed to create store.log");

        // Configure file and stdout layers
        let file_layer = fmt::layer().with_writer(file).with_ansi(false);
        let stdout_layer = fmt::layer().with_ansi(true);

        // Set up tracing with file and stdout output
        tracing_subscriber::registry()
            .with(file_layer)
            .with(stdout_layer)
            .with(EnvFilter::from_default_env())
            .init();

        // Process incoming log messages
        while let Some(log) = self.receiver.recv().await {
            match log.level {
                tracing::Level::INFO => info!("{}", log.message),
                tracing::Level::ERROR => error!("{}", log.message),
                tracing::Level::DEBUG => debug!("{}", log.message),
                tracing::Level::WARN => tracing::warn!("{}", log.message),
                tracing::Level::TRACE => tracing::trace!("{}", log.message),
            }
        }
    }
}

/// Initializes the logger and returns a sender for log messages.
pub fn init_logger() -> mpsc::Sender<LogMessage> {
    let (sender, receiver) = mpsc::channel(100);
    let logger = Logger::new(receiver);
    tokio::spawn(logger.run());
    sender
}
