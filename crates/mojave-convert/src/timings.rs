//! Wall-clock timings of a run: what each stage and step took, for
//! `--timings` and `<out>/timings.txt`.
//!
//! The record is process-wide: [`span`] returns a guard that fills its
//! line when dropped, so a step nested in another shows indented under it,
//! in the order the steps began. [`note`] adds a line of counts (cache hits,
//! say) under the step that is open. Nothing here ever reaches a converted
//! file: the seconds differ run to run, so they go only to the console and
//! to `timings.txt`.

use std::cell::Cell;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

/// The file written beside `map-report.md`.
pub const FILE_NAME: &str = "timings.txt";

#[derive(Debug, Clone)]
struct Line {
    depth: usize,
    label: String,
    /// `None` while the step runs, and for a note.
    seconds: Option<f64>,
    note: bool,
}

static LINES: Mutex<Vec<Line>> = Mutex::new(Vec::new());
thread_local! {
    /// Steps open on this thread: a worker's steps start at the left.
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

fn lines() -> std::sync::MutexGuard<'static, Vec<Line>> {
    LINES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// An open step; its time is recorded when it is dropped.
#[must_use = "a span records its time when dropped"]
pub struct Span {
    index: usize,
    started: Instant,
}

impl Span {
    /// Seconds since the step began.
    pub fn elapsed(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        let seconds = self.started.elapsed().as_secs_f64();
        DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        if let Some(line) = lines().get_mut(self.index) {
            line.seconds = Some(seconds);
        }
    }
}

/// Opens a step called `label`.
pub fn span(label: impl Into<String>) -> Span {
    let depth = DEPTH.with(|d| {
        let depth = d.get();
        d.set(depth + 1);
        depth
    });
    let mut all = lines();
    all.push(Line {
        depth,
        label: label.into(),
        seconds: None,
        note: false,
    });
    Span {
        index: all.len() - 1,
        started: Instant::now(),
    }
}

/// Adds a line of text under the open step.
pub fn note(text: impl Into<String>) {
    let depth = DEPTH.with(Cell::get);
    lines().push(Line {
        depth,
        label: text.into(),
        seconds: None,
        note: true,
    });
}

/// Forgets everything recorded (tests, and a process that runs twice).
pub fn reset() {
    lines().clear();
}

/// The record as a table: seconds, then the step, indented by nesting.
pub fn table() -> String {
    let mut text = String::new();
    for line in lines().iter() {
        let indent = "  ".repeat(line.depth);
        if line.note {
            let _ = writeln!(text, "{:>9}  {indent}- {}", "", line.label);
        } else {
            match line.seconds {
                Some(s) => {
                    let _ = writeln!(text, "{s:>8.2}s  {indent}{}", line.label);
                }
                None => {
                    let _ = writeln!(text, "{:>9}  {indent}{} (unfinished)", "?", line.label);
                }
            }
        }
    }
    text
}

/// Writes the table to `<out>/timings.txt` (best effort: a run never fails
/// for its timings).
pub fn write(out: &Path) {
    if std::fs::create_dir_all(out).is_ok() {
        let _ = std::fs::write(out.join(FILE_NAME), table());
    }
}

/// A renderer that counts its calls and the time they take.
pub struct TimedRenderer<'a> {
    inner: &'a mut dyn foundry_render::Renderer,
    /// Calls, and seconds spent in them.
    pub calls: usize,
    pub seconds: f64,
    /// Pixels returned.
    pub pixels: u64,
}

impl<'a> TimedRenderer<'a> {
    /// Wraps `inner`.
    pub fn new(inner: &'a mut dyn foundry_render::Renderer) -> Self {
        Self {
            inner,
            calls: 0,
            seconds: 0.0,
            pixels: 0,
        }
    }

    /// Adds a note of what it counted.
    pub fn note(&self, what: &str) {
        note(format!(
            "{what}: {} renders, {:.2} s, {:.1} Mpixels",
            self.calls,
            self.seconds,
            self.pixels as f64 / 1.0e6
        ));
    }
}

impl foundry_render::Renderer for TimedRenderer<'_> {
    fn render(
        &mut self,
        scene: &foundry_render::Scene,
        window: foundry_render::Window,
    ) -> Result<foundry_render::RenderOutput, foundry_render::RenderError> {
        let started = Instant::now();
        let out = self.inner.render(scene, window);
        self.seconds += started.elapsed().as_secs_f64();
        self.calls += 1;
        if let Ok(o) = &out {
            self.pixels += u64::from(o.width) * u64::from(o.height);
        }
        out
    }

    fn backend(&self) -> foundry_render::Backend {
        self.inner.backend()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_steps_keep_their_order_and_depth() {
        {
            let _outer = span("timings-test outer");
            {
                let _inner = span("timings-test inner");
                note("timings-test 3 cached");
            }
        }
        let text = table();
        let line = |label: &str| {
            text.lines()
                .find(|l| l.ends_with(label))
                .unwrap_or_else(|| panic!("{label} missing: {text}"))
                .to_owned()
        };
        let indent = |l: &str| l[11..].len() - l[11..].trim_start().len();
        let (outer, inner) = (line("timings-test outer"), line("timings-test inner"));
        assert!(text.find(&outer) < text.find(&inner), "{text}");
        assert!(indent(&inner) > indent(&outer), "{text}");
        assert!(line("timings-test 3 cached").contains("- "), "{text}");
        assert!(!outer.contains("unfinished"), "{text}");
    }
}
