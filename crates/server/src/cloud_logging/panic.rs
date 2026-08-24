//! The panic hook.
//!
//! This is the one thing `sentry::init` contributed that nothing else in the
//! process replaced: its default integrations install a panic hook, so a panic
//! reached Sentry as an issue with a symbolicated backtrace. The standard
//! library's default hook writes an unstructured multi-line block to stderr,
//! which the logging agent records as several unrelated text entries at no
//! particular severity - invisible to Error Reporting and awkward to read.
//!
//! So the hook is explicit now. It writes one JSON entry carrying the panic
//! message and the backtrace, marked as a `ReportedErrorEvent`.
//!
//! Backtrace quality is the real difference from Sentry, which symbolicated
//! server-side from uploaded debug information. Here the frames are only as good
//! as what is compiled into the binary: `Backtrace::force_capture` captures
//! regardless of `RUST_BACKTRACE`, but the release profile has to keep debug
//! info for the frames to carry names and line numbers.

use serde_json::{Map, Value};
use std::backtrace::Backtrace;
use std::io::Write;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use super::format::decorate_as_reported_error;

/// Install the hook. Call once, before the runtime starts - the same position
/// `sentry::init` occupied, and for the same reason: a panic before this point
/// is not reported.
pub fn init_panic_hook() {
    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        let backtrace = Backtrace::force_capture();
        let location = info.location();

        let mut entry = Map::new();
        entry.insert("severity".into(), "CRITICAL".into());

        // Error Reporting groups on the message when there is no stack trace it
        // can parse, and a Rust backtrace is not one - but including it is still
        // what makes the entry diagnosable, so it goes in the message rather
        // than a side field where nobody would read it.
        entry.insert(
            "message".into(),
            format!("panic: {}\n{backtrace}", panic_message(info)).into(),
        );

        if let Ok(time) = OffsetDateTime::now_utc().format(&Rfc3339) {
            entry.insert("time".into(), time.into());
        }

        decorate_as_reported_error(
            &mut entry,
            location.map(|location| location.file()),
            location.map(|location| location.line()),
            "panic",
        );

        // Straight to stdout rather than through `tracing`: a panic can happen
        // while a tracing layer is on the stack, and re-entering the subscriber
        // from inside the panic hook risks a second panic that would abort the
        // process before anything is written. `writeln!` on a locked stdout is
        // one syscall's worth of machinery.
        let line = Value::Object(entry).to_string();
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{line}");
        let _ = stdout.flush();

        // Still run the default hook. It writes the human-readable form to
        // stderr, which is what you want when running the binary locally.
        default_hook(info);
    }));
}

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();

    // The two shapes `panic!` produces: a formatted String, or a literal &str.
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        "Box<dyn Any>".to_string()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use std::process::Command;

    /// Marks the child process below. A panic hook is global and a backtrace is
    /// only real when it comes from a real panic, so this runs the whole thing
    /// end to end in a subprocess rather than faking a `PanicHookInfo`.
    const CHILD: &str = "WALNUT_PANIC_HOOK_CHILD";

    #[test]
    fn panic_hook_writes_a_reported_error_entry() {
        if std::env::var(CHILD).is_ok() {
            super::init_panic_hook();
            panic!("simulation state is inconsistent");
        }

        // Re-run this same test as a child, which takes the branch above.
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "cloud_logging::panic::tests::panic_hook_writes_a_reported_error_entry",
                "--exact",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .expect("could not re-run the test binary");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let entry: Value = stdout
            .lines()
            .filter(|line| line.starts_with('{'))
            .find_map(|line| serde_json::from_str(line).ok())
            .unwrap_or_else(|| panic!("no JSON entry on the child's stdout:\n{stdout}"));

        assert_eq!(entry["severity"], "CRITICAL");
        assert_eq!(
            entry["@type"],
            super::super::format::REPORTED_ERROR_EVENT_TYPE
        );
        assert!(entry["serviceContext"]["service"].is_string());

        let message = entry["message"].as_str().unwrap();
        assert!(
            message.starts_with("panic: simulation state is inconsistent"),
            "{message}"
        );
        // The backtrace has to be in `message` - Error Reporting reads it from
        // nowhere else.
        assert!(
            message.contains("panic_hook_writes_a_reported_error_entry"),
            "{message}"
        );

        // The panic location, which is what the entry groups on.
        assert_eq!(entry["context"]["reportLocation"]["functionName"], "panic");
        assert!(entry["context"]["reportLocation"]["filePath"]
            .as_str()
            .unwrap()
            .ends_with("panic.rs"));

        // The default hook still runs, so a developer watching the terminal sees
        // the familiar message too.
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("simulation state is inconsistent"),
            "{stderr}"
        );
    }
}
