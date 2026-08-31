//! Custom `tracing_subscriber` event formatters for terminal-first
//! readability (issue #37 follow-up).
//!
//! Two related shapes:
//!
//! - [`RuuterTextFormat`] — the default `text` format. Compact
//!   single-line events, span fields filtered down to the ones a log
//!   reader actually needs (`trace_id`, `dsl.project`), Rust module
//!   target dropped, timestamp trimmed to `HH:MM:SS.mmm`. OTLP span
//!   export (when wired) still sees the full field set on the span —
//!   only the text rendering is trimmed.
//!
//! - [`RuuterPrettyFormat`] — same shape as text, plus ANSI colours
//!   (level, step marker, duration) and Unicode markers for the two
//!   most common event families (`Executed` steps, `http request
//!   completed` access log). For interactive local dev; do not pipe
//!   to a file or log aggregator (the colour escapes will confuse
//!   downstream tooling).
//!
//! Both formats intentionally elide the same span noise:
//! `otel.name`, `http.request.method`, `http.route`, `client.address`
//! are already OTel semantic-convention fields that appear on the
//! access log AND get duplicated onto every child event by
//! `tracing_subscriber`'s default fmt layer. That duplication is what
//! makes the default output unreadable on any terminal narrower than
//! ~250 columns; filtering it drops event lines from ~400 chars to
//! ~150.

use std::fmt::Write as _;

use tracing::{Event, Level, Subscriber};
use tracing_subscriber::field::{RecordFields, Visit};
use tracing_subscriber::fmt::format::{FmtSpan, Writer};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

const _UNUSED: FmtSpan = FmtSpan::NONE;

/// Compact terminal-friendly formatter. See module docs.
#[derive(Default)]
pub struct RuuterTextFormat {
    pub ansi: bool,
}

impl RuuterTextFormat {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Pretty formatter — same layout as text but ANSI-coloured and with
/// Unicode markers. See module docs.
#[derive(Default)]
pub struct RuuterPrettyFormat;

impl RuuterPrettyFormat {
    pub fn new() -> Self {
        Self
    }
}

/// Fields carried by the request span that we want to visually
/// surface. All other span fields are dropped from text rendering
/// (they remain on the span for OTLP export).
const ALLOWED_SPAN_FIELDS: &[&str] = &["trace_id", "dsl.project"];

/// Fields on the event itself that are already surfaced positionally
/// in the compact format (dsl.step / dsl.step.type / duration_ms /
/// dsl.next.step) or that the pretty renderer draws specially
/// (`attrs` gets its own trailing segment).
const POSITIONAL_EVENT_FIELDS: &[&str] = &[
    "dsl.step",
    "dsl.step.type",
    "duration_ms",
    "dsl.next.step",
    "attrs",
    "message",
    // Access log positional fields:
    "http.request.method",
    "http.route",
    "http.response.status_code",
    "dsl.project",
    "client.address",
    "trace_id",
];

/// Visitor that walks a tracing span/event field set and collects
/// only the fields on the allowlist. Used for span-field filtering.
struct AllowlistVisitor {
    out: Vec<(&'static str, String)>,
    allow: &'static [&'static str],
}

impl AllowlistVisitor {
    fn new(allow: &'static [&'static str]) -> Self {
        Self {
            out: Vec::new(),
            allow,
        }
    }
    fn take(&mut self, name: &'static str) -> Option<String> {
        if let Some(pos) = self.out.iter().position(|(n, _)| *n == name) {
            Some(self.out.remove(pos).1)
        } else {
            None
        }
    }
}

impl Visit for AllowlistVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        for &name in self.allow {
            if field.name() == name {
                self.out.push((name, format!("{:?}", value)));
                return;
            }
        }
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        for &name in self.allow {
            if field.name() == name {
                self.out.push((name, value.to_string()));
                return;
            }
        }
    }
}

/// Visitor that captures the standard `Executed` / access-log fields
/// positionally so the formatter can render them in a fixed order
/// rather than the semi-random insertion order tracing gives us.
#[derive(Default)]
struct EventFieldVisitor {
    message: Option<String>,
    // Executed line
    step: Option<String>,
    step_type: Option<String>,
    duration_ms: Option<f64>,
    next_step: Option<String>,
    attrs: Option<String>,
    // Access log
    http_method: Option<String>,
    http_route: Option<String>,
    http_status: Option<u64>,
    client_address: Option<String>,
    project: Option<String>,
    // Fallback bucket: any event field we didn't recognise gets
    // rendered as `k=v` at the end so we don't silently drop it.
    extras: Vec<(String, String)>,
}

impl Visit for EventFieldVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        match field.name() {
            "message" => self.message = Some(value.into()),
            "dsl.step" => self.step = Some(value.into()),
            "dsl.step.type" => self.step_type = Some(value.into()),
            "dsl.next.step" => self.next_step = Some(value.into()),
            "attrs" => self.attrs = Some(value.into()),
            "http.request.method" => self.http_method = Some(value.into()),
            "http.route" => self.http_route = Some(value.into()),
            "dsl.project" => self.project = Some(value.into()),
            "client.address" => self.client_address = Some(value.into()),
            other => self.extras.push((other.into(), value.into())),
        }
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{:?}", value);
        // Strip surrounding double-quotes ONLY when the whole value
        // is quoted (tracing wraps `&str` values in Debug quotes).
        // Do NOT use `trim_matches('"')` — that would also strip a
        // trailing quote from a value like `assign.keys="foo"` whose
        // interior contains quoted substrings.
        let cleaned = if rendered.len() >= 2 && rendered.starts_with('"') && rendered.ends_with('"')
        {
            rendered[1..rendered.len() - 1].to_string()
        } else {
            rendered
        };
        match field.name() {
            "message" => self.message = Some(cleaned),
            "dsl.step" => self.step = Some(cleaned),
            "dsl.step.type" => self.step_type = Some(cleaned),
            "dsl.next.step" => self.next_step = Some(cleaned),
            "attrs" => self.attrs = Some(cleaned),
            "http.request.method" => self.http_method = Some(cleaned),
            "http.route" => self.http_route = Some(cleaned),
            "dsl.project" => self.project = Some(cleaned),
            "client.address" => self.client_address = Some(cleaned),
            other => self.extras.push((other.into(), cleaned)),
        }
    }
    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        match field.name() {
            "duration_ms" => self.duration_ms = Some(value),
            other => self.extras.push((other.into(), format!("{}", value))),
        }
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        match field.name() {
            "http.response.status_code" => self.http_status = Some(value as u64),
            other => self.extras.push((other.into(), format!("{}", value))),
        }
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        match field.name() {
            "http.response.status_code" => self.http_status = Some(value),
            other => self.extras.push((other.into(), format!("{}", value))),
        }
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.extras
            .push((field.name().into(), format!("{}", value)));
    }
}

/// Render `duration_ms` in a human-friendly unit — µs / ms / s —
/// with a fixed 3-significant-digit precision. Terminals get "51µs"
/// not "0.051 ms".
fn render_duration(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.2}s", ms / 1000.0)
    } else if ms >= 1.0 {
        format!("{:.1}ms", ms)
    } else {
        // Sub-millisecond: display as µs. `ms` is already µs-precise
        // via `logging::duration_ms`, so `ms * 1000` is an integer.
        format!("{:.0}µs", ms * 1000.0)
    }
}

/// Extract the visible fields from the enclosing span stack.
/// Returns `(trace_id_short, project)`. Both `""` when unset.
fn read_span_fields<S, N>(ctx: &FmtContext<'_, S, N>) -> (String, String)
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    let mut short_trace = String::new();
    let mut project = String::new();
    if let Some(span) = ctx.lookup_current() {
        for span in span.scope().from_root() {
            let ext = span.extensions();
            if let Some(fs) = ext.get::<FormattedFields<AllowlistFormatter>>() {
                for entry in fs.entries.iter() {
                    match entry.0 {
                        "trace_id" => {
                            short_trace = entry.1.chars().take(8).collect();
                        }
                        "dsl.project" => {
                            project = entry.1.clone();
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    (short_trace, project)
}

/// Timestamp helper — `HH:MM:SS.mmm` (drops the date and nanosecond
/// tail). Local time would be nicer for humans but adds a
/// `chrono`/`time` dep + timezone handling; UTC is unambiguous and
/// terminal-readable.
fn render_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs_of_day = now.as_secs() % 86_400;
    let h = secs_of_day / 3600;
    let m = (secs_of_day % 3600) / 60;
    let s = secs_of_day % 60;
    let ms = now.subsec_millis();
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, ms)
}

fn level_label(level: &Level, ansi: bool) -> &'static str {
    if !ansi {
        return match *level {
            Level::ERROR => "ERROR",
            Level::WARN => "WARN ",
            Level::INFO => "INFO ",
            Level::DEBUG => "DEBUG",
            Level::TRACE => "TRACE",
        };
    }
    match *level {
        Level::ERROR => "\x1b[31mERROR\x1b[0m",
        Level::WARN => "\x1b[33mWARN \x1b[0m",
        Level::INFO => "\x1b[32mINFO \x1b[0m",
        Level::DEBUG => "\x1b[36mDEBUG\x1b[0m",
        Level::TRACE => "\x1b[35mTRACE\x1b[0m",
    }
}

fn dim(s: &str, ansi: bool) -> String {
    if ansi {
        format!("\x1b[2m{}\x1b[0m", s)
    } else {
        s.to_string()
    }
}
fn bold(s: &str, ansi: bool) -> String {
    if ansi {
        format!("\x1b[1m{}\x1b[0m", s)
    } else {
        s.to_string()
    }
}
fn cyan(s: &str, ansi: bool) -> String {
    if ansi {
        format!("\x1b[36m{}\x1b[0m", s)
    } else {
        s.to_string()
    }
}

/// Shared render body used by both text and pretty formatters. The
/// only difference between the two is ANSI colour on / off + a
/// Unicode marker vs plain text label.
fn write_event<S, N>(
    ctx: &FmtContext<'_, S, N>,
    mut writer: Writer<'_>,
    event: &Event<'_>,
    ansi: bool,
) -> std::fmt::Result
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    let (short_trace, project) = read_span_fields::<S, N>(ctx);
    let ts = render_timestamp();
    let level = level_label(event.metadata().level(), ansi);

    let mut ev = EventFieldVisitor::default();
    event.record(&mut ev);
    // Prefer span project over per-event project (they normally match).
    if ev.project.is_none() && !project.is_empty() {
        ev.project = Some(project.clone());
    }

    // Prefix: `HH:MM:SS.mmm LEVEL [t=xxxxxxxx project]`
    write!(writer, "{} {} ", dim(&ts, ansi), level)?;
    if !short_trace.is_empty() || ev.project.is_some() {
        let inside = format!(
            "t={} {}",
            if short_trace.is_empty() {
                "-"
            } else {
                &short_trace
            },
            ev.project.as_deref().unwrap_or("-"),
        );
        write!(writer, "{} ", dim(&format!("[{}]", inside), ansi))?;
    }

    let msg = ev.message.as_deref().unwrap_or("");

    // Route by event message so the compact format is deterministic.
    match msg {
        "Executed" => {
            let marker = if ansi { "\x1b[36m▸\x1b[0m " } else { "▸ " };
            let step = ev.step.as_deref().unwrap_or("?");
            let stype = ev.step_type.as_deref().unwrap_or("?");
            let dur = ev
                .duration_ms
                .map(render_duration)
                .unwrap_or_else(|| "?".into());
            let next = ev.next_step.as_deref().unwrap_or("-");
            write!(
                writer,
                "{}{} {} {} {} {}",
                marker,
                bold(step, ansi),
                dim(&format!("({})", stype), ansi),
                cyan(&dur, ansi),
                dim("→", ansi),
                next,
            )?;
            if let Some(a) = ev.attrs.as_deref() {
                if !a.is_empty() {
                    write!(writer, "  {}", a)?;
                }
            }
        }
        "http request completed" => {
            let marker = if ansi { "\x1b[35m⏹\x1b[0m " } else { "⏹ " };
            let method = ev.http_method.as_deref().unwrap_or("?");
            let route = ev.http_route.as_deref().unwrap_or("?");
            let status = ev
                .http_status
                .map(|s| s.to_string())
                .unwrap_or_else(|| "?".into());
            let dur = ev
                .duration_ms
                .map(render_duration)
                .unwrap_or_else(|| "?".into());
            let client = ev.client_address.as_deref().unwrap_or("-");
            let status_col = if ansi {
                match ev.http_status.unwrap_or(0) {
                    200..=299 => format!("\x1b[32m{}\x1b[0m", status),
                    300..=399 => format!("\x1b[33m{}\x1b[0m", status),
                    400..=599 => format!("\x1b[31m{}\x1b[0m", status),
                    _ => status,
                }
            } else {
                status
            };
            write!(
                writer,
                "{}{} {} {} {}  {}",
                marker,
                bold(method, ansi),
                route,
                status_col,
                cyan(&dur, ansi),
                dim(&format!("from {}", client), ansi),
            )?;
        }
        other => {
            // Generic fallback — boot logs, warnings, DSL log:, errors,
            // anything else. Render as `<message> k1=v1 k2=v2`.
            write!(writer, "{}", other)?;
            // Skip fields already captured positionally; include everything
            // else so we don't silently drop diagnostic info.
            let mut wrote_sep = false;
            let write_kv =
                |w: &mut Writer<'_>, k: &str, v: &str, wrote_sep: &mut bool| -> std::fmt::Result {
                    if !*wrote_sep {
                        write!(w, "  ")?;
                        *wrote_sep = true;
                    } else {
                        write!(w, " ")?;
                    }
                    write!(w, "{}={}", dim(k, ansi), v)
                };
            // Include positional fields that DIDN'T get consumed by a
            // recognised event shape — e.g. the "step failed" ERROR line
            // populates dsl.step / dsl.step.type / duration_ms.
            if let Some(s) = &ev.step {
                write_kv(&mut writer, "step", s, &mut wrote_sep)?;
            }
            if let Some(t) = &ev.step_type {
                write_kv(&mut writer, "type", t, &mut wrote_sep)?;
            }
            if let Some(d) = ev.duration_ms {
                write_kv(&mut writer, "took", &render_duration(d), &mut wrote_sep)?;
            }
            for (k, v) in &ev.extras {
                if POSITIONAL_EVENT_FIELDS.contains(&k.as_str()) {
                    continue;
                }
                write_kv(&mut writer, k, v, &mut wrote_sep)?;
            }
        }
    }

    writeln!(writer)?;
    Ok(())
}

impl<S, N> FormatEvent<S, N> for RuuterTextFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        write_event(ctx, writer, event, self.ansi)
    }
}

impl<S, N> FormatEvent<S, N> for RuuterPrettyFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        write_event(ctx, writer, event, true)
    }
}

/// Span-field formatter that captures the allowlisted fields into a
/// stashed `FormattedFields` extension on each span so the event
/// formatter can read them cheaply.
pub struct AllowlistFormatter;

impl<'writer> FormatFields<'writer> for AllowlistFormatter {
    fn format_fields<R: RecordFields>(
        &self,
        _writer: Writer<'writer>,
        _fields: R,
    ) -> std::fmt::Result {
        // The stashing happens via on_new_span; nothing to write to
        // the writer at format time. tracing_subscriber calls this
        // for span field rendering; we defer to the visitor stashed
        // on the span extension.
        Ok(())
    }
}

/// Extension attached to each span carrying the allowlisted fields.
/// Read back by [`read_span_fields`] at event-format time.
pub struct FormattedFields<F> {
    pub entries: Vec<(&'static str, String)>,
    _marker: std::marker::PhantomData<F>,
}

impl<F> FormattedFields<F> {
    fn from_visitor(v: AllowlistVisitor) -> Self {
        Self {
            entries: v.out,
            _marker: std::marker::PhantomData,
        }
    }
}

/// Layer that stashes allowlisted span fields on each new span. Must
/// be registered ahead of the fmt layer for [`AllowlistFormatter`] to
/// find the fields at event time.
pub struct AllowlistSpanCapture;

impl<S> tracing_subscriber::Layer<S> for AllowlistSpanCapture
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = AllowlistVisitor::new(ALLOWED_SPAN_FIELDS);
        attrs.record(&mut visitor);
        if let Some(span) = ctx.span(id) {
            let mut ext = span.extensions_mut();
            ext.insert(FormattedFields::<AllowlistFormatter>::from_visitor(visitor));
        }
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if let Some(span) = ctx.span(id) {
            let mut ext = span.extensions_mut();
            let stash = ext.get_mut::<FormattedFields<AllowlistFormatter>>();
            if let Some(stash) = stash {
                let mut visitor = AllowlistVisitor::new(ALLOWED_SPAN_FIELDS);
                values.record(&mut visitor);
                for (k, v) in visitor.out {
                    // Replace if present, else push.
                    if let Some(pos) = stash.entries.iter().position(|(n, _)| *n == k) {
                        stash.entries[pos].1 = v;
                    } else {
                        stash.entries.push((k, v));
                    }
                }
            }
        }
    }
}

// Silence dead-code warnings for helper items only used by the
// visitor tests below when compiled in the test profile.
#[allow(dead_code)]
fn _touch() {
    let mut v = AllowlistVisitor::new(ALLOWED_SPAN_FIELDS);
    let _ = v.take("trace_id");
    let _ = write!(String::new(), "");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_render_picks_right_unit() {
        assert_eq!(render_duration(0.051), "51µs");
        assert_eq!(render_duration(4.7), "4.7ms");
        assert_eq!(render_duration(1234.0), "1.23s");
    }

    #[test]
    fn duration_render_zero_is_clean() {
        assert_eq!(render_duration(0.0), "0µs");
    }

    #[test]
    fn timestamp_is_hh_mm_ss_mmm() {
        let ts = render_timestamp();
        assert_eq!(ts.len(), 12, "expected HH:MM:SS.mmm, got '{}'", ts);
        assert_eq!(ts.chars().nth(2), Some(':'));
        assert_eq!(ts.chars().nth(5), Some(':'));
        assert_eq!(ts.chars().nth(8), Some('.'));
    }

    #[test]
    fn level_label_ansi_wraps_but_plain_is_five_chars() {
        assert_eq!(level_label(&Level::INFO, false).trim(), "INFO");
        assert_eq!(level_label(&Level::WARN, false).trim(), "WARN");
        // ANSI variants are longer because of the escape codes.
        assert!(level_label(&Level::INFO, true).contains("INFO"));
        assert!(level_label(&Level::INFO, true).contains("\x1b["));
    }
}
