//! Proof-bound transport for existing Host-authorized managed Runtime proxies.

use std::{fmt, net::SocketAddr, time::Duration};

use axum::http::HeaderValue;

use crate::{
    process_ownership::{ProcessOwnership, ProcessRole},
    verified_control::{ControlResponse, ControlTransportError, VerifiedConnection},
};

/// Fixed Runtime endpoint and installation. No authority is retained here.
#[derive(Clone)]
pub struct ManagedDaemonTransport {
    ownership: ProcessOwnership,
    address: SocketAddr,
    timeout: Duration,
}

impl fmt::Debug for ManagedDaemonTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManagedDaemonTransport")
            .finish_non_exhaustive()
    }
}

impl ManagedDaemonTransport {
    /// Selects one installation and literal endpoint without connecting or mutating.
    #[must_use]
    pub const fn new(ownership: ProcessOwnership, address: SocketAddr, timeout: Duration) -> Self {
        Self {
            ownership,
            address,
            timeout,
        }
    }

    /// Proves the current Runtime on the same stream used for the Host request.
    /// Resolving authority remains the caller's lazy, post-proof responsibility.
    ///
    /// # Errors
    /// Rejects incomplete/unheld ownership, a different endpoint, invalid proof,
    /// unavailable authority, or an incomplete bounded response. Never reconnects.
    pub fn request<E>(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        maximum_response_bytes: usize,
        authorization: impl FnOnce() -> Result<HeaderValue, E>,
    ) -> Result<ControlResponse, ControlTransportError> {
        if self.ownership.is_leased(ProcessRole::Runtime) != Ok(true)
            || !self
                .ownership
                .snapshot(ProcessRole::Runtime)
                .is_ok_and(|snapshot| {
                    snapshot.is_some_and(|snapshot| snapshot.endpoint == Some(self.address))
                })
        {
            return Err(ControlTransportError::Unavailable);
        }
        VerifiedConnection::connect(&self.ownership, ProcessRole::Runtime, self.timeout)?
            .request_bounded(method, path, body, maximum_response_bytes, authorization)
    }
}
