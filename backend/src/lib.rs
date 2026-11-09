#![forbid(unsafe_code)]
//! PullBoard's local API, ephemeral cache and read-only upstream adapters.

pub mod ai;
pub mod api;
pub mod attention;
pub mod cache;
pub mod config;
pub mod error;
pub mod forge;
pub mod jira;
pub mod model;
mod request_budget;
pub mod runtime_mode;
pub mod sync;
pub mod transport;
pub mod upstream;
