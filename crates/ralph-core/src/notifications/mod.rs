//! Notification primitives for loop-completion webhooks.
//!
//! This module is the runtime counterpart to the configuration types in
//! [`crate::config`] (`config/notifications.rs`). It exposes:
//!
//! - the pure `{{var}}` template renderer used to build webhook request
//!   bodies (plan KTD-5) — [`template`];
//! - the [`WebhookTransport`] abstraction with a real reqwest-backed
//!   implementation ([`ReqwestTransport`]) and a test fake ([`transport::FakeTransport`]) — [`transport`];
//! - the best-effort [`dispatch`] orchestrator that renders and POSTs each
//!   subscribed endpoint (plan U3) — [`dispatch`].
//!
//! The config file `config/notifications.rs` and this module are distinct:
//! the former describes *what* the user configured; this module provides
//! *how* a configured `body` template is rendered and delivered.

pub mod dispatch;
pub mod template;
pub mod transport;

pub use dispatch::{TerminationContext, dispatch, status_for_reason};
pub use template::{RenderError, json_string_escape, render};
pub use transport::{ReqwestTransport, TransportError, TransportOutcome, WebhookTransport};
