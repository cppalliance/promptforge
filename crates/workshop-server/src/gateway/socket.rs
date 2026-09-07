//! Authenticated WebSocket connections from Workshop to Gateway.

use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

use super::{GatewayClient, GatewayError};

type GatewaySocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// An authenticated WebSocket connection to Gateway Realtime transcription.
pub(crate) type GatewayRealtimeSocket = GatewaySocket;

impl GatewayClient {
    /// Opens the gateway's authenticated Realtime transcription socket.
    ///
    /// The target is fixed to `/v1/realtime?intent=transcription`; browser
    /// query parameters and handshake policy headers never cross the relay.
    pub(crate) async fn connect_realtime(&self) -> Result<GatewayRealtimeSocket, GatewayError> {
        self.connect_socket().await
    }

    async fn connect_socket(&self) -> Result<GatewaySocket, GatewayError> {
        let mut url = url::Url::parse(&self.base_url)
            .map_err(|source| GatewayError::Transport(Box::new(source)))?;
        let scheme = match url.scheme() {
            "http" => "ws",
            "https" => "wss",
            scheme => {
                return Err(GatewayError::Transport(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("gateway URL scheme {scheme:?} cannot carry a WebSocket"),
                ))));
            }
        };
        url.set_scheme(scheme).map_err(|()| {
            GatewayError::Transport(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "gateway URL scheme cannot be converted to WebSocket",
            )))
        })?;
        let path = format!("{}/v1/realtime", url.path().trim_end_matches('/'));
        url.set_path(&path);
        url.set_query(Some("intent=transcription"));
        url.set_fragment(None);
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|source| GatewayError::Transport(Box::new(source)))?;
        if !self.api_key.is_empty() {
            let value = format!("Bearer {}", self.api_key)
                .parse()
                .map_err(|source| GatewayError::Transport(Box::new(source)))?;
            request.headers_mut().insert(
                tokio_tungstenite::tungstenite::http::header::AUTHORIZATION,
                value,
            );
        }
        match tokio::time::timeout(
            self.request_timeout,
            tokio_tungstenite::connect_async(request),
        )
        .await
        {
            Ok(Ok((socket, _response))) => Ok(socket),
            Ok(Err(source)) => Err(GatewayError::Transport(Box::new(source))),
            Err(elapsed) => Err(GatewayError::Transport(Box::new(elapsed))),
        }
    }
}
