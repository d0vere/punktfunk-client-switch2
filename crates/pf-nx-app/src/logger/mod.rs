//! Routes `tracing` to the log file (or TCP to a dev machine, named at launch), to the
//! platform's own log (nxlink's stdout on the console), and to an in-memory ring for the log
//! overlay.
//! One shared level filter; submodules are leaves and only this one wires them together.
mod launch;
mod level;
mod ring;
mod sink;

use std::path::Path;
use std::sync::Mutex;
#[cfg(target_os = "horizon")]
use std::sync::Arc;
#[cfg(target_os = "horizon")]
use std::io::{self, Write};
#[cfg(target_os = "horizon")]
use tracing_subscriber::fmt::MakeWriter;

use anyhow::{Context, Result};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

pub use level::resolved_level;
pub use ring::{recent_lines, set_ring_capture};
pub use sink::{latest_log_file, previous_log_file, MAX_LOG_BYTES};

/// Host-console bundle format: `<RFC3339-Z> <LEVEL> <target> <message>`.
struct HostLogFormat;

impl<S, N> FormatEvent<S, N> for HostLogFormat
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        tracing_subscriber::fmt::time::SystemTime.format_time(&mut writer)?;
        let meta = event.metadata();
        write!(writer, " {:5} {} ", meta.level(), meta.target())?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

// On Switch 2 an async worker can leave a freshly created log file empty if
// the main thread blocks inside a broken network syscall or exits abruptly.
// Format an entire event first, then persist it in one synchronous write/flush.
// The worker-based logger remains unchanged on desktop platforms.
#[cfg(target_os = "horizon")]
#[derive(Clone)]
struct DurableWriter(Arc<Mutex<sink::Sink>>);

#[cfg(target_os = "horizon")]
struct DurableEvent {
    sink: Arc<Mutex<sink::Sink>>,
    data: Vec<u8>,
}

#[cfg(target_os = "horizon")]
impl<'a> MakeWriter<'a> for DurableWriter {
    type Writer = DurableEvent;

    fn make_writer(&'a self) -> Self::Writer {
        DurableEvent { sink: Arc::clone(&self.0), data: Vec::with_capacity(256) }
    }
}

#[cfg(target_os = "horizon")]
impl Write for DurableEvent {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.data.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.data.is_empty() { return Ok(()); }
        let mut guard = self.sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.write_all(&self.data)?;
        guard.flush()?;
        self.data.clear();
        Ok(())
    }
}

#[cfg(target_os = "horizon")]
impl Drop for DurableEvent {
    fn drop(&mut self) {
        if let Err(e) = self.flush() {
            // A failing SD write must be visible even if tracing cannot log it.
            eprintln!("punktfunk: failed to persist log event: {e}");
        }
    }
}

/// The appender's writer thread, held here rather than by `run` so [`flush`] can reach it from the
/// panic hook: with `panic = "abort"` no destructor runs, and the PANIC line would die in the queue.
static GUARD: Mutex<Vec<tracing_appender::non_blocking::WorkerGuard>> = Mutex::new(Vec::new());

/// Flushes the sink on drop. Hold it for the life of the process.
pub struct FlushOnDrop;

impl Drop for FlushOnDrop {
    fn drop(&mut self) {
        flush();
    }
}

/// Drains queued lines to the sink and stops the writer thread. Later events are dropped.
pub fn flush() {
    let guards = std::mem::take(&mut *GUARD.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    drop(guards);
}

/// Installs the global subscriber: the file (or TCP) sink, `extra` when the platform has a log
/// of its own, and the ring, all under one level filter.
pub fn init_subscriber(logs_dir: &Path, extra: Option<Box<dyn std::io::Write + Send>>) -> Result<FlushOnDrop> {
    let sink = sink::open(logs_dir).context("open log sink")?;
    let level = resolved_level();
    level::install(level);

    #[cfg(target_os = "horizon")]
    {
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_writer(DurableWriter(Arc::new(Mutex::new(sink))))
            .with_ansi(false)
            .event_format(HostLogFormat)
            .with_filter(LevelFilter::from_level(level));
        let mut guards = Vec::new();
        let extra_layer = extra.map(|extra| {
            let (writer, guard) = tracing_appender::non_blocking(extra);
            guards.push(guard);
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .event_format(HostLogFormat)
                .with_filter(LevelFilter::from_level(level))
        });
        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(extra_layer)
            .with(ring::layer())
            .try_init()
            .context("install log subscriber")?;
        *GUARD.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = guards;
    }

    #[cfg(not(target_os = "horizon"))]
    {
        let (writer, guard) = tracing_appender::non_blocking(sink);
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_ansi(false)
            .event_format(HostLogFormat)
            .with_filter(LevelFilter::from_level(level));
        let mut guards = vec![guard];
        let extra_layer = extra.map(|extra| {
            let (writer, guard) = tracing_appender::non_blocking(extra);
            guards.push(guard);
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .event_format(HostLogFormat)
                .with_filter(LevelFilter::from_level(level))
        });
        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(extra_layer)
            .with(ring::layer())
            .try_init()
            .context("install log subscriber")?;
        *GUARD.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = guards;
    }
    Ok(FlushOnDrop)
}
