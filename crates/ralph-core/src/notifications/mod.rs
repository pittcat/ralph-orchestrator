//! Notification primitives for loop-completion webhooks.
//!
//! This module is the runtime counterpart to the configuration types in
//! [`crate::config`] (`config/notifications.rs`). It currently exposes the
//! pure `{{var}}` template renderer used to build webhook request bodies
//! (plan KTD-5). It has **no** async / network code and **no** extra crate
//! dependencies — rendering is a pure, synchronous string transformation.
//!
//! The config file `config/notifications.rs` and this module are distinct:
//! the former describes *what* the user configured; this module provides
//! *how* a configured `body` template is rendered into a concrete payload.

pub mod template;

pub use template::{RenderError, json_string_escape, render};
