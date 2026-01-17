//! FileWatchService - Background service for file system monitoring
//!
//! This service uses the `notify` crate to watch directories for changes,
//! automatically detecting when tracks are added, modified, or deleted.
//! File events are broadcast to subscribers for UI updates and database sync.

use super::messages::{WatchCommand, AppEvent, ServiceHandle};
use crossbeam::channel::{Receiver, Sender};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Configuration for the FileWatchService
#[derive(Debug, Clone)]
pub struct WatchServiceConfig {
    /// Debounce duration for file events
    pub debounce_duration: Duration,
    /// File extensions to watch (e.g., ["wav", "WAV"])
    pub extensions: Vec<String>,
}

impl Default for WatchServiceConfig {
    fn default() -> Self {
        Self {
            debounce_duration: Duration::from_millis(500),
            extensions: vec!["wav".to_string(), "WAV".to_string()],
        }
    }
}

/// FileWatchService monitors directories for file changes
pub struct FileWatchService {
    command_rx: Receiver<WatchCommand>,
    event_tx: Sender<AppEvent>,
    config: WatchServiceConfig,
    watched_paths: Arc<Mutex<HashSet<PathBuf>>>,
}

impl FileWatchService {
    /// Spawn a new FileWatchService in a background thread
    pub fn spawn(
        config: WatchServiceConfig,
        event_tx: Sender<AppEvent>,
    ) -> Result<ServiceHandle<WatchCommand>, String> {
        let (command_tx, command_rx) = crossbeam::channel::unbounded();
        let watched_paths = Arc::new(Mutex::new(HashSet::new()));

        let service = FileWatchService {
            command_rx,
            event_tx: event_tx.clone(),
            config,
            watched_paths,
        };

        let handle = thread::Builder::new()
            .name("file-watch-service".into())
            .spawn(move || {
                service.run();
            })
            .map_err(|e| format!("Failed to spawn file watch service thread: {}", e))?;

        let _ = event_tx.send(AppEvent::ServiceStarted {
            service_name: "FileWatchService".to_string(),
        });

        Ok(ServiceHandle {
            command_tx,
            thread_handle: Some(handle),
        })
    }

    /// Main service loop
    fn run(self) {
        log::info!("FileWatchService started");

        // Create the file watcher
        let extensions = self.config.extensions.clone();

        let (watcher_tx, watcher_rx) = crossbeam::channel::unbounded();

        let mut watcher: RecommendedWatcher = match notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = watcher_tx.send(event);
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                log::error!("Failed to create file watcher: {}", e);
                let _ = self.event_tx.send(AppEvent::ServiceError {
                    service_name: "FileWatchService".to_string(),
                    error: e.to_string(),
                });
                return;
            }
        };

        // Configure watcher
        let _ = watcher.configure(Config::default().with_poll_interval(self.config.debounce_duration));

        loop {
            // Use select to handle both commands and file events
            crossbeam::select! {
                recv(self.command_rx) -> cmd => {
                    match cmd {
                        Ok(WatchCommand::Shutdown) => {
                            log::info!("FileWatchService shutting down");
                            break;
                        }
                        Ok(cmd) => self.handle_command(cmd, &mut watcher),
                        Err(_) => {
                            log::info!("Command channel closed, shutting down");
                            break;
                        }
                    }
                }
                recv(watcher_rx) -> event => {
                    if let Ok(event) = event {
                        self.handle_file_event(event, &extensions);
                    }
                }
                default(Duration::from_millis(100)) => {
                    // Periodic check - allows clean shutdown
                }
            }
        }

        let _ = self.event_tx.send(AppEvent::ServiceStopped {
            service_name: "FileWatchService".to_string(),
        });

        log::info!("FileWatchService stopped");
    }

    /// Handle a command
    fn handle_command(&self, cmd: WatchCommand, watcher: &mut RecommendedWatcher) {
        match cmd {
            WatchCommand::Watch { path, reply } => {
                let result = self.add_watch(&path, watcher);
                let _ = reply.send(result);
            }

            WatchCommand::Unwatch { path, reply } => {
                let result = self.remove_watch(&path, watcher);
                let _ = reply.send(result);
            }

            WatchCommand::GetWatchedPaths { reply } => {
                let paths = self.watched_paths.lock()
                    .map(|guard| guard.iter().cloned().collect())
                    .unwrap_or_default();
                let _ = reply.send(paths);
            }

            WatchCommand::Shutdown => {
                // Handled in main loop
            }
        }
    }

    /// Add a directory to watch
    fn add_watch(&self, path: &PathBuf, watcher: &mut RecommendedWatcher) -> Result<(), String> {
        if !path.exists() {
            return Err(format!("Path does not exist: {}", path.display()));
        }

        if !path.is_dir() {
            return Err(format!("Path is not a directory: {}", path.display()));
        }

        // Add to watcher
        watcher
            .watch(path, RecursiveMode::Recursive)
            .map_err(|e| format!("Failed to watch path: {}", e))?;

        // Track watched path
        if let Ok(mut paths) = self.watched_paths.lock() {
            paths.insert(path.clone());
        }

        log::info!("Now watching: {}", path.display());
        Ok(())
    }

    /// Remove a directory from watch
    fn remove_watch(&self, path: &PathBuf, watcher: &mut RecommendedWatcher) -> Result<(), String> {
        watcher
            .unwatch(path)
            .map_err(|e| format!("Failed to unwatch path: {}", e))?;

        if let Ok(mut paths) = self.watched_paths.lock() {
            paths.remove(path);
        }

        log::info!("Stopped watching: {}", path.display());
        Ok(())
    }

    /// Handle a file system event
    fn handle_file_event(&self, event: Event, extensions: &[String]) {
        let event_kind = event.kind.clone();

        // Separate into files and directories
        let mut file_paths = Vec::new();
        let mut dir_paths = Vec::new();

        for path in event.paths {
            if path.is_dir() {
                dir_paths.push(path);
            } else if path.extension()
                .and_then(|e| e.to_str())
                .map(|e| extensions.iter().any(|ext| ext.eq_ignore_ascii_case(e)))
                .unwrap_or(false)
            {
                file_paths.push(path);
            }
        }

        // Handle directory events
        for path in dir_paths {
            match event_kind {
                EventKind::Create(_) => {
                    let _ = self.event_tx.send(AppEvent::DirectoryCreated(path));
                }
                EventKind::Remove(_) => {
                    let _ = self.event_tx.send(AppEvent::DirectoryDeleted(path));
                }
                _ => {}
            }
        }

        // Handle file events
        for path in file_paths {
            let app_event = match event_kind {
                EventKind::Create(_) => AppEvent::FileCreated(path),
                EventKind::Modify(_) => AppEvent::FileModified(path),
                EventKind::Remove(_) => AppEvent::FileDeleted(path),
                _ => continue,
            };

            if let Err(e) = self.event_tx.send(app_event) {
                log::warn!("Failed to send file event: {}", e);
            }
        }
    }
}

/// Client for interacting with the FileWatchService
pub struct WatchClient {
    command_tx: crossbeam::channel::Sender<WatchCommand>,
}

impl WatchClient {
    /// Create a new client from a service handle
    pub fn new(handle: &ServiceHandle<WatchCommand>) -> Self {
        Self {
            command_tx: handle.command_tx.clone(),
        }
    }

    /// Start watching a directory (blocking)
    pub fn watch(&self, path: PathBuf) -> Result<(), String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(WatchCommand::Watch { path, reply: tx })
            .map_err(|e| e.to_string())?;

        rx.blocking_recv().map_err(|e| e.to_string())?
    }

    /// Stop watching a directory (blocking)
    pub fn unwatch(&self, path: PathBuf) -> Result<(), String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(WatchCommand::Unwatch { path, reply: tx })
            .map_err(|e| e.to_string())?;

        rx.blocking_recv().map_err(|e| e.to_string())?
    }

    /// Get list of watched paths (blocking)
    pub fn get_watched_paths(&self) -> Result<Vec<PathBuf>, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(WatchCommand::GetWatchedPaths { reply: tx })
            .map_err(|e| e.to_string())?;

        rx.blocking_recv().map_err(|e| e.to_string())
    }

    /// Shutdown the service
    pub fn shutdown(&self) -> Result<(), String> {
        self.command_tx
            .send(WatchCommand::Shutdown)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::messages::EventBus;
    use tempfile::TempDir;

    #[test]
    fn test_watch_service_lifecycle() {
        let event_bus = EventBus::new(16);
        let config = WatchServiceConfig::default();

        let handle = FileWatchService::spawn(config, event_bus.sender()).unwrap();
        let client = WatchClient::new(&handle);

        // Get initial watched paths (should be empty)
        let paths = client.get_watched_paths().unwrap();
        assert!(paths.is_empty());

        // Shutdown
        client.shutdown().unwrap();

        // Wait for thread to finish
        if let Some(h) = handle.thread_handle {
            h.join().unwrap();
        }
    }

    #[test]
    fn test_watch_directory() {
        let temp_dir = TempDir::new().unwrap();
        let event_bus = EventBus::new(16);
        let config = WatchServiceConfig::default();

        let handle = FileWatchService::spawn(config, event_bus.sender()).unwrap();
        let client = WatchClient::new(&handle);

        // Watch the temp directory
        client.watch(temp_dir.path().to_path_buf()).unwrap();

        // Verify it's being watched
        let paths = client.get_watched_paths().unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], temp_dir.path());

        // Unwatch
        client.unwatch(temp_dir.path().to_path_buf()).unwrap();
        let paths = client.get_watched_paths().unwrap();
        assert!(paths.is_empty());

        // Cleanup
        client.shutdown().unwrap();
    }
}
