#![forbid(unsafe_code)]
//! Shared foundations for the backend and disposable configuration installer.
//! No server, cache, forge SDK or native keyring is linked by default.

pub mod config;
pub mod error;
pub mod model;
pub mod request_budget;
pub mod transport;
