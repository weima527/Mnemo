//! Cross-platform local-socket transport.
//!
//! One abstraction over a Unix domain socket (Unix) and a named pipe (Windows),
//! via the `interprocess` crate. The OS split is confined to [`make_name`] and
//! [`default_endpoint`]; everything above this module is platform-agnostic.
//!
//! The `endpoint` string is interpreted per-OS: on Windows it is a namespaced
//! pipe key (e.g. `"mnemo-daemon"` → `\\.\pipe\mnemo-daemon`); on Unix it is a
//! filesystem path to the socket (e.g. `~/.mnemo/daemon.sock`).

use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::{tokio::Listener, tokio::Stream as IpcStream, ListenerOptions};
use std::io;

#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};

/// The duplex byte stream type (impls tokio `AsyncRead + AsyncWrite`).
pub type Stream = IpcStream;

/// The production endpoint: a named pipe key on Windows, the `daemon.sock` path
/// on Unix.
pub fn default_endpoint() -> String {
    #[cfg(windows)]
    {
        "mnemo-daemon".to_string()
    }
    #[cfg(unix)]
    {
        mnemo_store::paths::home_mnemo()
            .join("daemon.sock")
            .to_string_lossy()
            .into_owned()
    }
}

/// Build a platform `Name` from the endpoint string.
#[cfg(windows)]
fn make_name(endpoint: &str) -> io::Result<interprocess::local_socket::Name<'_>> {
    endpoint.to_ns_name::<GenericNamespaced>()
}

#[cfg(unix)]
fn make_name(endpoint: &str) -> io::Result<interprocess::local_socket::Name<'_>> {
    endpoint.to_fs_name::<GenericFilePath>()
}

/// Bind a listener at `endpoint`.
pub fn bind(endpoint: &str) -> io::Result<Listener> {
    // On Unix, clear a stale socket file left by a crashed daemon (best-effort).
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(endpoint);
    }
    let name = make_name(endpoint)?;
    ListenerOptions::new().name(name).create_tokio()
}

/// Accept the next inbound connection (keeps the `interprocess` trait imports
/// confined to this module).
pub async fn accept(listener: &Listener) -> io::Result<Stream> {
    listener.accept().await
}

/// Connect to a listener at `endpoint`.
pub async fn connect(endpoint: &str) -> io::Result<Stream> {
    let name = make_name(endpoint)?;
    Stream::connect(name).await
}
