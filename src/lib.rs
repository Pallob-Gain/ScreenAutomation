//! Node graph automation driven by screen regions.
pub mod backend;
pub mod graph;
pub mod node;
pub mod screen_events;
pub mod event_sources;
pub mod preview;
#[cfg(feature = "gui")]
pub mod workspace;
#[cfg(feature = "gui")]
pub mod desktop;
#[cfg(feature = "gui")]
pub mod session_log;
