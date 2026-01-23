//! Background services for mesh-core
//!
//! This module provides message-driven services that handle long-running
//! operations in background threads, keeping the UI responsive.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────┐     Commands      ┌──────────────┐
//! │   UI Thread │ ───────────────►  │ QueryService │
//! │             │ ◄─────────────── │   (CozoDB)   │
//! └─────────────┘     Replies       └──────────────┘
//!       │                                  │
//!       │ Subscribe                        │ Publish
//!       ▼                                  ▼
//! ┌─────────────────────────────────────────────┐
//! │               Event Bus                      │
//! │  (crossbeam broadcast - fan-out to all)     │
//! └─────────────────────────────────────────────┘
//!                      ▲
//!                      │ Publish
//!               ┌──────────────┐
//!               │ WatchService │
//!               │   (notify)   │
//!               └──────────────┘
//! ```
//!
//! # Services
//!
//! - [`QueryService`] - Handles all database operations
//! - [`FileWatchService`] - Monitors directories for file changes
//!
//! # Usage
//!
//! ```no_run
//! use mesh_core::services::{
//!     EventBus, QueryService, QueryClient,
//!     FileWatchService, WatchServiceConfig, WatchClient,
//! };
//! use mesh_core::db::DatabaseService;
//!
//! // Create event bus for inter-service communication
//! let event_bus = EventBus::default();
//!
//! // Create database service
//! let db_service = DatabaseService::new("/path/to/collection").unwrap();
//!
//! // Start QueryService with shared database service
//! let query_handle = QueryService::spawn(
//!     db_service,
//!     event_bus.sender(),
//! ).unwrap();
//!
//! // Create client for queries
//! let query_client = QueryClient::new(&query_handle);
//!
//! // Query the database
//! let count = query_client.get_track_count().unwrap();
//! println!("Track count: {}", count);
//!
//! // Shutdown services
//! query_client.shutdown().unwrap();
//! ```

pub mod messages;
pub mod query;
pub mod watch;

pub use messages::{
    // Commands
    QueryCommand, WatchCommand, MigrationCommand,
    // Events
    AppEvent, AnalysisPhase,
    // Types
    EnergyDirection, MixSuggestion, MixReason, MigrationResult,
    // Infrastructure
    ServiceHandle, EventBus,
};

pub use query::{QueryService, QueryClient};
pub use watch::{FileWatchService, WatchServiceConfig, WatchClient};
