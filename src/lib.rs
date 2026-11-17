//! Node graph automation driven by screen regions.
pub mod backend;
pub mod graph;
pub mod node;
pub mod screen_events;
#[cfg(feature = "gui")]
pub mod desktop;
#[cfg(feature = "gui")]
pub mod session_log;
