//! Response-body capture for the gateway client's buffered calls.

use super::{GatewayError, GatewayResponse};

/// Captures the status and raw body of a gateway response.
pub(super) async fn read(response: reqwest::Response) -> Result<GatewayResponse, GatewayError> {
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|source| GatewayError::ReadBody(Box::new(source)))?
        .to_vec();
    Ok(GatewayResponse { status, body })
}
