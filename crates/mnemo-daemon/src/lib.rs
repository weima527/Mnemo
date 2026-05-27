//! The Mnemo daemon: a resident process that holds the [`mnemo_tenant`]
//! `TenantManager` and answers requests over a cross-platform local socket
//! (Unix domain socket / Windows named pipe) using length-prefixed JSON-RPC 2.0.
//!
//! - [`protocol`] — wire envelopes, method param/result types, framing.
//! - [`transport`] — the cross-platform local-socket endpoint.
//! - [`server`] — accept loop + request dispatch ([`run`], [`run_default`]).
//! - [`client`] — the one-shot client the CLI uses.

pub mod client;
pub mod protocol;
pub mod server;
pub mod transport;

pub use server::{run, run_default};
