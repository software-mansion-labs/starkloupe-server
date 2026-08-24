//! Per-request context: `httpRequest` and the Cloud Trace id.
//!
//! This is the piece Sentry never actually provided. `sentry_tower::NewSentryLayer`
//! only bound a fresh Hub per request so scopes could not leak between
//! concurrent tasks; attaching the request itself is `SentryHttpLayer`, which
//! was never added to the router even though `sentry-tower`'s `http`, `axum` and
//! `axum-matched-path` features were switched on for it. Every issue therefore
//! arrived without a method, a path or a status.
//!
//! The context lives in a task-local rather than a tracing span because the
//! interesting `error!` call sites are deep inside the simulate and
//! internal-tracing crates, and a task-local is picked up there without those
//! crates having to be instrumented or to thread a context argument through.
//! The trade-off is `tokio::spawn`: a spawned task does not inherit task-locals,
//! so a log line from a detached background job carries no request even if the
//! request is what started it. That matches the schedulers in
//! binaries_manager_service, which genuinely have no request to attribute to.

use axum::{
    body::Body,
    http::{HeaderMap, Request},
    middleware::Next,
    response::Response,
};
use serde_json::{Map, Value};
use std::{
    sync::{
        atomic::{AtomicU16, AtomicU64, Ordering},
        Arc, OnceLock,
    },
    time::Instant,
};

/// Needed to build `projects/PROJECT_ID/traces/TRACE_ID`, the only form Cloud
/// Logging accepts for the trace field. Set once at startup from the metadata
/// server; until it is, trace ids are parsed but not emitted, because a bare
/// trace id in that field links to nothing.
static PROJECT_ID: OnceLock<String> = OnceLock::new();

pub fn set_project_id(project_id: String) {
    let _ = PROJECT_ID.set(project_id);
}

tokio::task_local! {
    static SCOPE: Arc<RequestScope>;
}

/// Run `f` against the current request's scope, if this task is serving one.
pub fn with_current<F: FnOnce(&RequestScope)>(f: F) {
    let _ = SCOPE.try_with(|scope| f(scope));
}

/// A trace context lifted out of the incoming headers, in the shape Cloud
/// Logging wants: a 32-hex trace id and a 16-hex span id.
struct TraceContext {
    trace_id: String,
    span_id: Option<String>,
    sampled: bool,
}

impl TraceContext {
    /// `X-Cloud-Trace-Context: TRACE_ID/SPAN_ID;o=TRACE_TRUE` - what the Google
    /// load balancer in front of this service sets. The span id is decimal here
    /// and hex everywhere else, hence the reformat.
    fn from_cloud_header(value: &str) -> Option<Self> {
        let (trace_id, rest) = match value.split_once('/') {
            Some((trace_id, rest)) => (trace_id, Some(rest)),
            None => (value.split(';').next()?, None),
        };

        if !is_hex(trace_id, 32) {
            return None;
        }

        let (span, options) = match rest {
            Some(rest) => match rest.split_once(';') {
                Some((span, options)) => (span, Some(options)),
                None => (rest, None),
            },
            None => ("", None),
        };

        Some(Self {
            trace_id: trace_id.to_string(),
            span_id: span.parse::<u64>().ok().map(|id| format!("{id:016x}")),
            sampled: options.is_some_and(|options| options.contains("o=1")),
        })
    }

    /// `traceparent: 00-TRACE_ID-SPAN_ID-FLAGS`. Not what the load balancer
    /// sends, but what any OpenTelemetry-instrumented caller will, and the
    /// collector already on this VM speaks it.
    fn from_traceparent(value: &str) -> Option<Self> {
        let mut parts = value.split('-');
        let _version = parts.next()?;
        let trace_id = parts.next()?;
        let span_id = parts.next()?;
        let flags = parts.next().unwrap_or("00");

        if !is_hex(trace_id, 32) || !is_hex(span_id, 16) {
            return None;
        }

        Some(Self {
            trace_id: trace_id.to_string(),
            span_id: Some(span_id.to_string()),
            sampled: u8::from_str_radix(flags, 16).is_ok_and(|flags| flags & 1 == 1),
        })
    }

    fn from_headers(headers: &HeaderMap) -> Option<Self> {
        headers
            .get("x-cloud-trace-context")
            .and_then(|value| value.to_str().ok())
            .and_then(Self::from_cloud_header)
            .or_else(|| {
                headers
                    .get("traceparent")
                    .and_then(|value| value.to_str().ok())
                    .and_then(Self::from_traceparent)
            })
    }
}

fn is_hex(value: &str, len: usize) -> bool {
    value.len() == len && value.chars().all(|c| c.is_ascii_hexdigit())
}

/// Sentinel for "the response has not been produced yet", so an error logged
/// mid-request omits the status rather than claiming a wrong one.
const STATUS_UNKNOWN: u16 = 0;
const SIZE_UNKNOWN: u64 = u64::MAX;

pub struct RequestScope {
    method: String,
    url: String,
    protocol: String,
    user_agent: Option<String>,
    referer: Option<String>,
    remote_ip: Option<String>,
    request_size: Option<u64>,
    trace: Option<TraceContext>,
    started: Instant,
    status: AtomicU16,
    response_size: AtomicU64,
}

impl RequestScope {
    fn from_request(request: &Request<Body>) -> Self {
        let headers = request.headers();
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };

        // The socket peer is the load balancer, so it is not worth recording;
        // the client is the first hop in X-Forwarded-For. Cloud Armor and the
        // LB own this header, so the value cannot be spoofed by the client.
        let remote_ip = header("x-forwarded-for").map(|forwarded| {
            forwarded
                .split(',')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string()
        });

        Self {
            method: request.method().to_string(),
            url: request.uri().to_string(),
            protocol: format!("{:?}", request.version()),
            user_agent: header("user-agent"),
            referer: header("referer"),
            remote_ip,
            request_size: header("content-length").and_then(|size| size.parse().ok()),
            trace: TraceContext::from_headers(headers),
            started: Instant::now(),
            status: AtomicU16::new(STATUS_UNKNOWN),
            response_size: AtomicU64::new(SIZE_UNKNOWN),
        }
    }

    fn status(&self) -> Option<u16> {
        match self.status.load(Ordering::Relaxed) {
            STATUS_UNKNOWN => None,
            status => Some(status),
        }
    }

    pub fn trace_field(&self) -> Option<String> {
        let trace = self.trace.as_ref()?;
        let project_id = PROJECT_ID.get()?;
        Some(format!("projects/{project_id}/traces/{}", trace.trace_id))
    }

    pub fn span_id(&self) -> Option<String> {
        self.trace.as_ref()?.span_id.clone()
    }

    pub fn trace_sampled(&self) -> bool {
        self.trace.as_ref().is_some_and(|trace| trace.sampled)
    }

    /// `LogEntry.httpRequest`.
    /// <https://cloud.google.com/logging/docs/reference/v2/rest/v2/LogEntry#HttpRequest>
    pub fn log_entry_http_request(&self) -> Value {
        let mut request = Map::new();
        request.insert("requestMethod".into(), self.method.as_str().into());
        request.insert("requestUrl".into(), self.url.as_str().into());
        request.insert("protocol".into(), self.protocol.as_str().into());
        request.insert(
            "latency".into(),
            format!("{}s", self.started.elapsed().as_secs_f64()).into(),
        );

        // requestSize and responseSize are int64 in the LogEntry schema, and
        // proto3 JSON encodes int64 as a string. status is int32, so it stays a
        // number.
        if let Some(size) = self.request_size {
            request.insert("requestSize".into(), size.to_string().into());
        }
        if let Some(user_agent) = &self.user_agent {
            request.insert("userAgent".into(), user_agent.as_str().into());
        }
        if let Some(referer) = &self.referer {
            request.insert("referer".into(), referer.as_str().into());
        }
        if let Some(remote_ip) = &self.remote_ip {
            request.insert("remoteIp".into(), remote_ip.as_str().into());
        }
        if let Some(status) = self.status() {
            request.insert("status".into(), status.into());
        }
        match self.response_size.load(Ordering::Relaxed) {
            SIZE_UNKNOWN => {}
            size => {
                request.insert("responseSize".into(), size.to_string().into());
            }
        }

        Value::Object(request)
    }

    /// `ReportedErrorEvent.context.httpRequest` - a different and smaller shape
    /// than the one above, with its own field names.
    /// <https://cloud.google.com/error-reporting/reference/rest/v1beta1/projects.events/report#httprequestcontext>
    pub fn error_context_http_request(&self) -> Value {
        let mut request = Map::new();
        request.insert("method".into(), self.method.as_str().into());
        request.insert("url".into(), self.url.as_str().into());

        if let Some(user_agent) = &self.user_agent {
            request.insert("userAgent".into(), user_agent.as_str().into());
        }
        if let Some(referer) = &self.referer {
            request.insert("referrer".into(), referer.as_str().into());
        }
        if let Some(remote_ip) = &self.remote_ip {
            request.insert("remoteIp".into(), remote_ip.as_str().into());
        }
        if let Some(status) = self.status() {
            request.insert("responseStatusCode".into(), status.into());
        }

        Value::Object(request)
    }
}

/// The middleware to hang on the router, in place of
/// `sentry_tower::NewSentryLayer`. Wrap it with `axum::middleware::from_fn`.
pub async fn record_request(request: Request<Body>, next: Next<Body>) -> Response {
    let scope = Arc::new(RequestScope::from_request(&request));

    let response = SCOPE.scope(scope.clone(), next.run(request)).await;

    // Fill in what only the response knows before the access line is written,
    // so that line carries a complete httpRequest.
    scope
        .status
        .store(response.status().as_u16(), Ordering::Relaxed);
    if let Some(size) = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        scope.response_size.store(size, Ordering::Relaxed);
    }

    // Emitted inside the scope so the formatter attaches the httpRequest and
    // trace id the same way it does for every other line. WARNING rather than
    // ERROR for a 5xx: the failure itself is reported by whatever `error!` call
    // produced it, and duplicating that here would double every group.
    if is_worth_logging(&scope.url, response.status()) {
        SCOPE.sync_scope(scope.clone(), || {
            if response.status().is_server_error() {
                tracing::warn!("{} {} -> {}", scope.method, scope.url, response.status());
            } else {
                tracing::info!("{} {} -> {}", scope.method, scope.url, response.status());
            }
        });
    }

    response
}

/// Endpoints polled by infrastructure rather than called by users. The load
/// balancer health-checks /health every 10 seconds (walnut-infra
/// loadbalancer.tf), so logging every one of those would put ~8.6k entries a day
/// into Cloud Logging that say nothing. They are still logged when they fail,
/// which is the only time they are interesting.
///
/// The request scope itself is always set, including for these - an error raised
/// while serving a health check is still attributed to it.
fn is_worth_logging(url: &str, status: axum::http::StatusCode) -> bool {
    const POLLED_BY_INFRASTRUCTURE: [&str; 3] = ["/health", "/metrics", "/_ah/warmup"];

    if status.is_client_error() || status.is_server_error() {
        return true;
    }

    let path = url.split('?').next().unwrap_or(url);
    !POLLED_BY_INFRASTRUCTURE.contains(&path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};
    use tower::ServiceExt;

    fn scope(headers: &[(&str, &str)]) -> RequestScope {
        let mut request = Request::builder()
            .method("POST")
            .uri("/v1/simulate-transaction");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        RequestScope::from_request(&request.body(Body::empty()).unwrap())
    }

    #[test]
    fn cloud_trace_header_is_parsed() {
        set_project_id("software-mansion-dev".to_string());

        let scope = scope(&[(
            "x-cloud-trace-context",
            "105445aa7843bc8bf206b12000100000/1;o=1",
        )]);

        assert_eq!(
            scope.trace_field().unwrap(),
            "projects/software-mansion-dev/traces/105445aa7843bc8bf206b12000100000"
        );
        // Decimal on the wire, hex in the log entry.
        assert_eq!(scope.span_id().unwrap(), "0000000000000001");
        assert!(scope.trace_sampled());
    }

    #[test]
    fn cloud_trace_header_without_a_span_or_options() {
        let scope = scope(&[("x-cloud-trace-context", "105445aa7843bc8bf206b12000100000")]);

        assert!(scope.span_id().is_none());
        assert!(!scope.trace_sampled());
    }

    #[test]
    fn traceparent_is_the_fallback() {
        let scope = scope(&[(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )]);

        assert_eq!(scope.span_id().unwrap(), "00f067aa0ba902b7");
        assert!(scope.trace_sampled());
    }

    #[test]
    fn a_malformed_trace_header_is_dropped_rather_than_emitted() {
        // A bad id would produce a trace link pointing at nothing.
        for header in [
            ("x-cloud-trace-context", "not-a-trace-id/1;o=1"),
            ("traceparent", "00-tooshort-00f067aa0ba902b7-01"),
            ("traceparent", "garbage"),
        ] {
            assert!(scope(&[header]).trace_field().is_none(), "{header:?}");
        }
    }

    #[test]
    fn http_request_takes_the_client_ip_from_the_forwarded_chain() {
        // The socket peer is the load balancer; the client is the first hop.
        let scope = scope(&[("x-forwarded-for", "203.0.113.7, 35.191.0.1")]);

        assert_eq!(
            scope.log_entry_http_request()["remoteIp"],
            Value::from("203.0.113.7")
        );
    }

    #[test]
    fn status_is_omitted_until_the_response_exists() {
        let scope = scope(&[]);

        // Mid-request: no status yet, in either shape.
        assert!(scope.log_entry_http_request().get("status").is_none());
        assert!(scope
            .error_context_http_request()
            .get("responseStatusCode")
            .is_none());

        scope.status.store(500, Ordering::Relaxed);

        assert_eq!(scope.log_entry_http_request()["status"], 500);
        assert_eq!(
            scope.error_context_http_request()["responseStatusCode"],
            500
        );
    }

    #[test]
    fn the_two_http_request_shapes_use_their_own_field_names() {
        let scope = scope(&[
            ("referer", "https://app.walnut.dev/"),
            ("user-agent", "curl/8"),
        ]);

        // LogEntry.httpRequest spells it "referer"; ReportedErrorEvent spells it
        // "referrer". They are specified by different APIs.
        assert!(scope.log_entry_http_request().get("referer").is_some());
        assert!(scope.error_context_http_request().get("referrer").is_some());
        assert!(scope.log_entry_http_request().get("method").is_none());
        assert!(scope
            .error_context_http_request()
            .get("requestMethod")
            .is_none());
    }

    #[test]
    fn infrastructure_polling_is_not_logged_unless_it_fails() {
        use axum::http::StatusCode;

        assert!(!is_worth_logging("/health", StatusCode::OK));
        assert!(!is_worth_logging("/metrics", StatusCode::OK));
        assert!(!is_worth_logging("/_ah/warmup", StatusCode::OK));

        // A failing health check is the whole point of having one.
        assert!(is_worth_logging("/health", StatusCode::SERVICE_UNAVAILABLE));

        // Real traffic always logs, and a query string must not smuggle a path
        // past the match either way.
        assert!(is_worth_logging("/v1/simulate-transaction", StatusCode::OK));
        assert!(is_worth_logging("/v1/search/0xabc?full=1", StatusCode::OK));
        assert!(!is_worth_logging("/health?probe=lb", StatusCode::OK));
    }

    #[tokio::test]
    async fn the_scope_is_visible_to_a_handler_and_absent_outside_one() {
        let seen = Arc::new(std::sync::Mutex::new(None));
        let captured = seen.clone();

        let app = Router::new()
            .route(
                "/v1/debug-transaction",
                get(move || {
                    let captured = captured.clone();
                    async move {
                        with_current(|scope| {
                            *captured.lock().unwrap() = Some(scope.log_entry_http_request());
                        });
                        "ok"
                    }
                }),
            )
            .layer(axum::middleware::from_fn(record_request));

        app.oneshot(
            Request::builder()
                .uri("/v1/debug-transaction")
                .header("user-agent", "walnut-tests")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        let http_request = seen
            .lock()
            .unwrap()
            .take()
            .expect("no scope in the handler");
        assert_eq!(http_request["requestMethod"], "GET");
        assert_eq!(http_request["requestUrl"], "/v1/debug-transaction");
        assert_eq!(http_request["userAgent"], "walnut-tests");

        // Outside a request - the startup path, the schedulers - there is no
        // scope, and reading it must be a no-op rather than a panic.
        let mut ran = false;
        with_current(|_| ran = true);
        assert!(!ran);
    }
}
