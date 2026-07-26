//! nowayprompt — multi-purpose Wayland prompt utility (Rust rewrite).
//!
//! Library crate exposing the protocol, frontend, and config modules
//! for integration tests and external consumers.

#![allow(dead_code)]

pub mod command;
pub mod config;
pub mod frontend;
pub mod protocol;
pub mod secret;
