//! Request extractors whose rejections render in the OpenAI error
//! envelope.

use axum::Json;

use super::GatewayError;

/// A JSON body extractor whose rejections render in the OpenAI error
/// envelope: a malformed body, a wrong content type, or a failed
/// deserialize all answer [`GatewayError::MalformedRequest`] with the
/// rejection's detail, never axum's plain-text rejection.
///
/// [`WireQuery`] and [`WirePath`] are its siblings for the query string
/// and path captures. All three are fallible extractors, so a handler
/// lists them after its auth extractor: extractors run in argument order,
/// and an unauthenticated caller must get 401 before its malformed input
/// gets 400.
pub(crate) struct WireJson<T>(pub(crate) T);

impl<T, S> axum::extract::FromRequest<S> for WireJson<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = GatewayError;

    async fn from_request(
        request: axum::extract::Request,
        state: &S,
    ) -> Result<WireJson<T>, GatewayError> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(WireJson(value)),
            Err(rejection) => Err(GatewayError::MalformedRequest(rejection.body_text())),
        }
    }
}

/// A query-string extractor whose rejection renders in the OpenAI error
/// envelope as [`GatewayError::MalformedRequest`]; see [`WireJson`].
pub(crate) struct WireQuery<T>(pub(crate) T);

impl<T, S> axum::extract::FromRequestParts<S> for WireQuery<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = GatewayError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<WireQuery<T>, GatewayError> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(WireQuery(value)),
            Err(rejection) => Err(GatewayError::MalformedRequest(rejection.body_text())),
        }
    }
}

/// A path-capture extractor whose rejection renders in the OpenAI error
/// envelope as [`GatewayError::MalformedRequest`]; see [`WireJson`].
pub(crate) struct WirePath<T>(pub(crate) T);

impl<T, S> axum::extract::FromRequestParts<S> for WirePath<T>
where
    T: serde::de::DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = GatewayError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<WirePath<T>, GatewayError> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(WirePath(value)),
            Err(rejection) => Err(GatewayError::MalformedRequest(rejection.body_text())),
        }
    }
}
