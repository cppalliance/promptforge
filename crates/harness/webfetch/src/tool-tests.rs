//! Fixtures and descriptor tests for [`WebFetch`]: the loopback article,
//! table, and JSON pages, the injected [`Lookup`] map, the mock servers
//! and their routes, and the loopback policy builders. The policy tests
//! (redirects, credentials, URL admission, status codes) and the body
//! tests (size caps, truncation, content types, charsets) sit in the
//! child modules and share these fixtures.

use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use flate2::Compression;
use flate2::write::GzEncoder;
use harness_capabilities::Tool;
use harness_runner::spawn::spawn_tagged;
use harness_runner::test_support::mock_tag;
use promptforge_api_types::tools::ToolId;

use super::WebFetch;
use crate::config::{FetchConfig, FetchConfigBuilder};
use crate::resolver::{Lookup, LookupFuture};

/// An article page long enough for readability extraction to fire.
const ARTICLE_HTML: &str = r"
    <html><body>
      <article>
        <h1>Loopback Test Article</h1>
        <p>This is the first substantial paragraph of a loopback test page,
           deliberately long enough to be treated as real article content.</p>
        <p>A second paragraph continues the prose so the extractor keeps the
           body and the character count stays comfortably above threshold.</p>
      </article>
    </body></html>
";

/// An article whose prose is full of multibyte characters.
const UNICODE_HTML: &str = r"
    <html><body>
      <article>
        <h1>Café Résumé Naïve</h1>
        <p>Café résumé naïve façade jalapeño piñata. Café résumé naïve façade
           jalapeño piñata. Café résumé naïve façade jalapeño piñata.</p>
        <p>Café résumé naïve façade jalapeño piñata. Café résumé naïve façade
           jalapeño piñata. Café résumé naïve façade jalapeño piñata.</p>
      </article>
    </body></html>
";

/// An HTML table page that readability would discard.
const TABLE_HTML: &str = r"
    <html><body>
      <p>Prices.</p>
      <table>
        <tr><th>Item</th><th>Cost</th></tr>
        <tr><td>WIDGETROW</td><td>4.20</td></tr>
        <tr><td>GADGETROW</td><td>6.90</td></tr>
      </table>
    </body></html>
";

/// A JSON document served with an `application/json` type.
const JSON_BODY: &str = r#"{"key":"value","numbers":[1,2,3],"nested":{"ok":true}}"#;

/// A [`Lookup`] that maps host names to fixed addresses for injected tests.
struct MapLookup {
    entries: Vec<(String, IpAddr)>,
}

impl Lookup for MapLookup {
    fn lookup(&self, host: String) -> LookupFuture {
        let addrs: Vec<SocketAddr> = self
            .entries
            .iter()
            .filter(|(h, _)| *h == host)
            .map(|(_, ip)| SocketAddr::new(*ip, 0))
            .collect();
        Box::pin(async move { Ok(addrs) })
    }
}

#[test]
fn descriptor_is_stable_and_faithful() {
    let tool = WebFetch::new();

    assert_eq!(
        tool.id(),
        ToolId::parse("promptforge/web/fetch").expect("valid id")
    );
    assert_eq!(tool.wire_name(), "web_fetch");
    assert_eq!(
        tool.description(),
        "Fetch a web page and return its main content as markdown."
    );
    let schema = tool.parameters_schema();
    assert_eq!(schema["properties"]["max_chars"]["maximum"], 40_000);
    assert_eq!(schema["required"], serde_json::json!(["url"]));
    assert_eq!(schema["properties"]["url"]["type"], "string");
}

#[test]
fn the_migrated_id_names_its_contributing_capability() {
    // promptforge/web_fetch migrated to promptforge/web/fetch: dropping the
    // last segment must yield the contributing capability's id.
    let id = WebFetch::new().id();
    assert_eq!(id.name(), "fetch");
    assert_eq!(
        id.capability(),
        promptforge_api_types::capabilities::CapabilityId::parse("promptforge/web")
            .expect("a valid capability id")
    );
}

#[derive(Clone)]
struct AppState {
    port: u16,
    hits: Arc<AtomicUsize>,
}

async fn root() -> Html<&'static str> {
    Html(ARTICLE_HTML)
}

async fn redir(State(state): State<AppState>) -> Redirect {
    Redirect::temporary(&format!("http://127.0.0.1:{}/target", state.port))
}

async fn target(State(state): State<AppState>) -> &'static str {
    state.hits.fetch_add(1, Ordering::SeqCst);
    "reached the internal target"
}

async fn unicode() -> Html<&'static str> {
    Html(UNICODE_HTML)
}

async fn large() -> Html<String> {
    let filler = "x".repeat(200_000);
    Html(format!("<html><body><p>{filler}</p></body></html>"))
}

async fn gzip_bomb() -> impl IntoResponse {
    let raw = "A".repeat(200_000);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(raw.as_bytes())
        .expect("writing to an in-memory gzip encoder must succeed");
    let compressed = encoder
        .finish()
        .expect("finishing an in-memory gzip encoder must succeed");
    (
        [(CONTENT_ENCODING, "gzip"), (CONTENT_TYPE, "text/html")],
        compressed,
    )
}

/// Declares a `Content-Length` far over any cap while its body never ends,
/// so `send` resolves on the headers and the precheck refuses on the
/// declared length alone. The tail never completes, so no timing crutch is
/// needed for the precheck to fire first.
async fn liar_content_length() -> Response {
    use futures_util::StreamExt as _;

    let head = futures_util::stream::once(async {
        Ok::<_, std::io::Error>("<html><body><p>x</p></body></html>")
    });
    let tail = futures_util::stream::once(async {
        std::future::pending::<()>().await;
        Ok::<_, std::io::Error>("")
    });
    Response::builder()
        .header(CONTENT_LENGTH, "1000000")
        .header(CONTENT_TYPE, "text/html")
        .body(Body::from_stream(head.chain(tail)))
        .expect("building the oversized-content-length response must succeed")
}

async fn table() -> Response {
    Response::builder()
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(TABLE_HTML))
        .expect("building the table html response must succeed")
}

async fn json_route() -> Response {
    Response::builder()
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(JSON_BODY))
        .expect("building the json response must succeed")
}

async fn jsonbig_route() -> Response {
    let filler = "x".repeat(200_000);
    let body = format!(r#"{{"filler":"{filler}"}}"#);
    Response::builder()
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("building the large json response must succeed")
}

async fn badcharset_route() -> Response {
    Response::builder()
        .header(CONTENT_TYPE, "text/plain; charset=not-a-charset")
        .body(Body::from("some plain body text"))
        .expect("building the bad-charset response must succeed")
}

async fn pdf_route() -> Response {
    Response::builder()
        .header(CONTENT_TYPE, "application/pdf")
        .body(Body::from(&b"%PDF-1.4 not a real pdf"[..]))
        .expect("building the pdf response must succeed")
}

async fn octet_route() -> Response {
    Response::builder()
        .header(CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(vec![0u8, 1, 2, 3, 4, 5]))
        .expect("building the octet-stream response must succeed")
}

async fn notype_route() -> Response {
    Response::builder()
        .body(Body::from("a body with no declared content type"))
        .expect("building the no-content-type response must succeed")
}

async fn not_found_route() -> Response {
    Response::builder()
        .status(axum::http::StatusCode::NOT_FOUND)
        .header(CONTENT_TYPE, "text/html")
        .body(Body::from("<html><body>Not Found</body></html>"))
        .expect("building the 404 response must succeed")
}

async fn internal_error_route() -> Response {
    Response::builder()
        .status(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
        .header(CONTENT_TYPE, "text/html")
        .body(Body::from("<html><body>Server Error</body></html>"))
        .expect("building the 500 response must succeed")
}

async fn latin1_route() -> Response {
    let body = vec![b'C', b'a', b'f', 0xE9];
    Response::builder()
        .header(CONTENT_TYPE, "text/plain; charset=ISO-8859-1")
        .body(Body::from(body))
        .expect("building the latin-1 response must succeed")
}

async fn plainbig_route() -> Response {
    Response::builder()
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from("y".repeat(200_000)))
        .expect("building the large text response must succeed")
}

/// Serves a `text/plain` body that fails mid-stream: one chunk, then an I/O
/// error, so the client's body read fails deterministically without any
/// timing crutch.
async fn plain_broken_route() -> Response {
    use futures_util::StreamExt as _;

    let head = futures_util::stream::once(async { Ok::<_, std::io::Error>("partial body ") });
    let boom = futures_util::stream::once(async {
        Err::<&'static str, std::io::Error>(std::io::Error::other("mid-stream failure"))
    });
    Response::builder()
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from_stream(head.chain(boom)))
        .expect("building the broken plain response must succeed")
}

async fn spawn_server() -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding a loopback listener must succeed");
    let port = listener
        .local_addr()
        .expect("the listener must have a local address")
        .port();
    let hits = Arc::new(AtomicUsize::new(0));
    let state = AppState {
        port,
        hits: Arc::clone(&hits),
    };
    let app = Router::new()
        .route("/", get(root))
        .route("/redir", get(redir))
        .route("/target", get(target))
        .route("/unicode", get(unicode))
        .route("/large", get(large))
        .route("/gzip", get(gzip_bomb))
        .route("/liar", get(liar_content_length))
        .route("/table", get(table))
        .route("/json", get(json_route))
        .route("/jsonbig", get(jsonbig_route))
        .route("/badcharset", get(badcharset_route))
        .route("/pdf", get(pdf_route))
        .route("/octet", get(octet_route))
        .route("/notype", get(notype_route))
        .route("/notfound", get(not_found_route))
        .route("/error500", get(internal_error_route))
        .route("/latin1", get(latin1_route))
        .route("/plainbig", get(plainbig_route))
        .route("/plainbroken", get(plain_broken_route))
        .with_state(state);
    spawn_tagged(mock_tag(), async move {
        axum::serve(listener, app)
            .await
            .expect("the loopback server must serve");
    });
    (port, hits)
}

/// A builder that can reach the loopback server: http allowed, its port on
/// the allowlist, and `localhost` pinned to `127.0.0.1`.
fn loopback_builder(port: u16) -> FetchConfigBuilder {
    let loopback: IpAddr = "127.0.0.1".parse().expect("loopback literal parses");
    FetchConfig::builder()
        .allow_http(true)
        .allow_ports([80, 443, port])
        .allow_host_address("localhost", loopback)
}

/// The built loopback policy.
fn loopback_config(port: u16) -> FetchConfig {
    loopback_builder(port)
        .build()
        .expect("loopback config is valid")
}

/// Builds a `WebFetch` over the loopback policy.
fn loopback_tool(port: u16) -> WebFetch {
    WebFetch::try_with_config(loopback_config(port)).expect("the loopback client builds")
}

#[derive(Clone)]
struct RecordingState {
    port: u16,
    recorded: Arc<Mutex<Vec<HeaderMap>>>,
}

async fn record_headers(
    State(state): State<RecordingState>,
    headers: HeaderMap,
) -> Html<&'static str> {
    state
        .recorded
        .lock()
        .expect("the recorded-headers mutex must not be poisoned")
        .push(headers);
    Html(ARTICLE_HTML)
}

async fn redirect_to_record(State(state): State<RecordingState>) -> Redirect {
    Redirect::temporary(&format!("http://localhost:{}/record", state.port))
}

/// Never responds, so a short total timeout aborts the request.
async fn hang() -> Html<&'static str> {
    std::future::pending::<()>().await;
    Html(ARTICLE_HTML)
}

async fn spawn_recording_server() -> (u16, Arc<Mutex<Vec<HeaderMap>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding a loopback listener must succeed");
    let port = listener
        .local_addr()
        .expect("the listener must have a local address")
        .port();
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let state = RecordingState {
        port,
        recorded: Arc::clone(&recorded),
    };
    let app = Router::new()
        .route("/record", get(record_headers))
        .route("/redir-record", get(redirect_to_record))
        .route("/slow", get(hang))
        .with_state(state);
    spawn_tagged(mock_tag(), async move {
        axum::serve(listener, app)
            .await
            .expect("the loopback recording server must serve");
    });
    (port, recorded)
}

fn split_header(out: &str) -> (&str, &str) {
    out.split_once("\n\n")
        .expect("the return must include a header and a blank-line separator")
}

#[path = "tool-tests-body.rs"]
mod body;
#[path = "tool-tests-policy.rs"]
mod policy;
