//! Cloud Logging output for the server, and the Error Reporting path built on
//! top of it.
//!
//! This replaces the Sentry client that used to live in main.rs. Sentry was
//! doing exactly one job here - crash and error reporting - through three call
//! sites: `sentry::init` (whose only real contribution was the panic hook),
//! `sentry_tracing::layer` (error-level events become issues) and
//! `sentry_tower::NewSentryLayer` (a fresh Hub per request). Nothing used
//! performance tracing, user context or sessions.
//!
//! Cloud Error Reporting covers the same ground and needs no client, no DSN and
//! no egress to a third party: it has no ingestion endpoint of its own and
//! instead reads what the process already writes to Cloud Logging. A log entry
//! is picked up as an error when it is `ERROR` or above and either contains a
//! recognisable stack trace or carries the `ReportedErrorEvent` type marker.
//! Rust backtraces are not in a format Error Reporting parses, so this module
//! takes the marker route - see `format::CloudLoggingFormatter`.
//!
//! The transport is stdout. The VM runs Container-Optimized OS with
//! `google-logging-enabled` (walnut-infra compute.tf), so the COS logging agent
//! ships the container's stdout to Cloud Logging and parses a single-line JSON
//! payload into `jsonPayload`, which is where the special field names below are
//! read from.
//!
//! Three pieces:
//!
//!   - `format` - the tracing layer. Maps levels to Cloud Logging severities and
//!     stamps error events with the Error Reporting marker.
//!   - `panic` - the panic hook, the one thing `sentry::init` gave us for free.
//!   - `request` - per-request context: `httpRequest` and the Cloud Trace id, so
//!     an error is attributed to the request that caused it. This is new. The
//!     Sentry tower layer only isolated Hubs; it never attached request data,
//!     because that is `SentryHttpLayer`, which was never wired up despite the
//!     `http`/`axum` features being enabled for it.

pub mod format;
pub mod panic;
pub mod request;

pub use format::layer;
pub use panic::init_panic_hook;
pub use request::record_request;

use std::sync::OnceLock;

/// Identifies this process to Error Reporting, which groups by service and
/// tracks whether a group regressed across versions.
pub struct ServiceContext {
    pub service: &'static str,
    pub version: &'static str,
}

/// Not `CARGO_PKG_NAME`, which is "server" - a name that says nothing in a
/// project that also runs cairovm-codes-server. There is one deployment of this
/// binary, so this is a constant rather than configuration.
const SERVICE: &str = "starknet-debugger-server";

/// The deployed image tag, resolved from instance metadata at startup.
static VERSION: OnceLock<String> = OnceLock::new();

pub fn service_context() -> ServiceContext {
    ServiceContext {
        service: SERVICE,
        // CARGO_PKG_VERSION only until metadata answers - and as the permanent
        // answer off-VM, where there is no image tag to report.
        version: VERSION
            .get()
            .map(String::as_str)
            .unwrap_or(env!("CARGO_PKG_VERSION")),
    }
}

/// Read the two things about this deployment that only the environment knows,
/// and that the log entries need.
///
/// Both come from the metadata server, which run.sh already queries for the
/// image tag and the access token, so there is nothing to configure and nothing
/// that can drift from what is actually running.
///
/// Neither is fatal. Without a project id the trace field is omitted and logs
/// simply do not link to Cloud Trace; without an image tag the version falls
/// back to the crate version. Everything else keeps working, which is why this
/// is not on the startup path that can refuse to boot.
pub async fn init_from_metadata() {
    // GOOGLE_CLOUD_PROJECT first: it is the standard GCP convention, set for you
    // on Cloud Run and App Engine, and the only way to get a project id when
    // running the release image somewhere without a metadata server.
    match std::env::var("GOOGLE_CLOUD_PROJECT") {
        Ok(project_id) => request::set_project_id(project_id),
        Err(_) => {
            if let Some(project_id) = metadata("project/project-id").await {
                request::set_project_id(project_id);
            }
        }
    }

    // The image tag is the git SHA the deploy was built from - see
    // walnut-infra terraform.tfvars - which is the only version of this service
    // that ever changes. `CARGO_PKG_VERSION` has been 0.1.0 since the repo
    // started, so reporting it would make every release look identical and
    // Error Reporting could never tell you which one introduced a group. This is
    // also what Sentry got wrong here: `release_name!()` reported `server@0.1.0`
    // on every deploy, forever.
    if let Some(tag) = metadata("instance/attributes/walnut-image")
        .await
        .as_deref()
        .and_then(image_tag)
    {
        let _ = VERSION.set(tag.to_string());
    }
}

/// The tag off a container image reference, or `None` if it carries no tag.
///
/// Only the last path segment can hold the tag separator, so a registry host
/// with a port does not look like one.
fn image_tag(image: &str) -> Option<&str> {
    let last_segment = image.rsplit('/').next()?;
    last_segment.rsplit_once(':').map(|(_, tag)| tag)
}

/// One metadata server lookup. `None` for anything other than a value, which
/// includes running off a GCE VM, where the host does not resolve at all.
async fn metadata(path: &str) -> Option<String> {
    let response = reqwest::Client::new()
        .get(format!(
            "http://metadata.google.internal/computeMetadata/v1/{path}"
        ))
        .header("Metadata-Flavor", "Google")
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;

    match response {
        Ok(response) => match response.text().await {
            Ok(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
            _ => None,
        },
        Err(error) => {
            tracing::debug!("no {path} from the metadata server: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::image_tag;

    #[test]
    fn the_image_tag_is_the_deployed_version() {
        // What walnut-infra actually puts in the walnut-image metadata key.
        assert_eq!(
            image_tag(
                "us-central1-docker.pkg.dev/software-mansion-dev/walnut/\
                 starknet-debugger-server:cb71d5f72b9f9b764f13a4d716567ff2fd3e8d68"
            ),
            Some("cb71d5f72b9f9b764f13a4d716567ff2fd3e8d68")
        );

        // An untagged reference has no version to report - better to fall back
        // to the crate version than to report a hostname.
        assert_eq!(image_tag("ubuntu"), None);
        assert_eq!(image_tag("registry.example.com:5000/walnut/server"), None);

        // A registry port must not be mistaken for a tag.
        assert_eq!(
            image_tag("registry.example.com:5000/walnut/server:abc123"),
            Some("abc123")
        );
    }
}
