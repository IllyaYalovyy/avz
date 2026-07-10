//! The terminal end of `avz-core`'s [`Progress`] callback trait (`VISION.md` §8).
//!
//! `avz-core` never prints. It calls [`Progress`] as phases begin, advance, and
//! end; this module is the only thing that turns those calls into pixels on a
//! terminal. A GUI would implement the same trait against a widget.
//!
//! Three presentations, chosen once at startup:
//!
//! - **Bars** — stderr is a terminal. `indicatif` draws a spinner for the phases
//!   whose length nobody knows in advance and a bar with frame count, live render
//!   fps, and an ETA for the one that does.
//! - **Lines** — stderr is a pipe or a CI log, where a bar's carriage returns
//!   would accumulate into a wall of redraw garbage. The same information
//!   degrades to one line per phase and one per decile of progress.
//! - **Silent** — `--quiet`. Errors only (`VISION.md` §3).
//!
//! Everything progress-shaped goes to stderr; stdout carries only the lines that
//! *are* the answer, so `avz render song.mp3 > log` still shows a working bar.
//!
//! Log records interleave with the bars through [`LogWriter`], a `tracing`
//! writer that suspends the draw before letting a line through. Without it, a
//! `--verbose` render's debug output and the bar overwrite each other.

use std::io::{self, IsTerminal as _, Write as _};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use avz_core::render::AdapterKind;
use avz_core::{Phase, Progress};
use indicatif::{MultiProgress, ProgressBar, ProgressState, ProgressStyle};
use tracing_subscriber::fmt::MakeWriter;

/// The template key [`style`] binds to [`fps_text`].
///
/// `indicatif`'s own `{per_sec}` renders `39.1114/s`, which is four digits of
/// precision nobody asked for on a number that moves every frame.
const FPS_KEY: &str = "fps";

/// The bar for a phase whose total is known: the frame count, the live render
/// fps, and an ETA (`VISION.md` §3).
const BAR_TEMPLATE: &str = "{msg:<10} [{bar:30.cyan/blue}] {pos}/{len} frames  {fps}  eta {eta}";

/// The spinner for a phase whose length nobody knows until it ends.
const SPINNER_TEMPLATE: &str = "{spinner:.cyan} {msg:<10} {elapsed_precise}";

/// Which characters the bar is drawn from.
const BAR_CHARS: &str = "=> ";

/// How often a spinner redraws itself while the phase it describes blocks.
const TICK: Duration = Duration::from_millis(120);

/// How much of a phase must complete before the line-based fallback says so.
///
/// Ten lines over a render: enough to see it is alive in a CI log, few enough
/// that a five-minute render does not bury the error that follows it.
const LINE_STEP_PERCENT: u64 = 10;

/// How the terminal is written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Errors only.
    Silent,
    /// One line per phase and per decile of progress, for pipes and CI logs.
    Lines,
    /// `indicatif` bars, for a terminal.
    Bars,
}

impl Mode {
    /// `--quiet` beats everything; a bar is drawn only where it can be redrawn.
    pub fn choose(quiet: bool, stderr_is_a_terminal: bool) -> Self {
        match (quiet, stderr_is_a_terminal) {
            (true, _) => Mode::Silent,
            (false, true) => Mode::Bars,
            (false, false) => Mode::Lines,
        }
    }
}

/// The template a phase's progress is drawn from.
fn template(total: Option<u64>) -> &'static str {
    match total {
        Some(_) => BAR_TEMPLATE,
        None => SPINNER_TEMPLATE,
    }
}

/// The live render rate, to one decimal place.
///
/// A bar drawn before the first frame lands has divided nothing by no time, so
/// `per_sec` can be zero or infinite. `-- fps` is what a rate nobody can compute
/// yet looks like; `inf fps` is what a bug looks like.
fn fps_text(per_sec: f64) -> String {
    if per_sec.is_finite() && per_sec > 0.0 {
        format!("{per_sec:.1} fps")
    } else {
        "-- fps".to_owned()
    }
}

/// A style, or a panic naming the template that would not parse.
///
/// `indicatif` validates templates at build time, not at compile time, so a
/// typo'd placeholder is a runtime failure — and an *unregistered* key is worse,
/// because it renders as nothing at all and fails silently.
/// `every_template_is_one_indicatif_accepts` catches the first;
/// `the_rendering_bar_draws_its_frame_count_render_fps_and_eta` draws a real bar
/// and catches the second.
fn style(template: &'static str) -> ProgressStyle {
    ProgressStyle::with_template(template)
        .expect("the progress templates are constants and must parse")
        .with_key(
            FPS_KEY,
            |state: &ProgressState, out: &mut dyn std::fmt::Write| {
                let _ = out.write_str(&fps_text(state.per_sec()));
            },
        )
        .progress_chars(BAR_CHARS)
}

/// The line-based fallback's state for one phase.
#[derive(Debug, Default)]
struct LineState {
    /// Units the phase will take, when known.
    total: u64,
    /// Units completed so far.
    done: u64,
    /// The next percentage worth printing a line for.
    next_percent: u64,
}

impl LineState {
    /// Begin a phase of `total` units.
    fn start(total: Option<u64>) -> Self {
        Self {
            total: total.unwrap_or(0),
            done: 0,
            next_percent: LINE_STEP_PERCENT,
        }
    }

    /// Absorb `units` of progress, and return the line to print if this crossed
    /// a decile. A phase with no total prints nothing: there is no percentage
    /// to report, and the phase-start line already said it began.
    fn advance(&mut self, phase: Phase, units: u64) -> Option<String> {
        if self.total == 0 {
            return None;
        }

        self.done = self.done.saturating_add(units).min(self.total);
        let percent = self.done * 100 / self.total;
        if percent < self.next_percent {
            return None;
        }

        self.next_percent = (percent / LINE_STEP_PERCENT + 1) * LINE_STEP_PERCENT;
        Some(format!(
            "{} {percent}% ({}/{})",
            phase.label(),
            self.done,
            self.total,
        ))
    }
}

/// What a phase-start line says.
fn start_line(phase: Phase, total: Option<u64>) -> String {
    match total {
        Some(total) => format!("{} {total} frames", phase.label()),
        None => phase.label().to_owned(),
    }
}

/// The terminal, as `avz-core` sees it.
///
/// One per process. `Progress` is `Send + Sync` and its methods take `&self`,
/// because the pipeline may report from worker threads; the mutable state below
/// therefore sits behind mutexes rather than behind `&mut self`.
#[derive(Debug)]
pub struct Ui {
    mode: Mode,
    /// The bars, when `mode` is [`Mode::Bars`]. Also the handle [`LogWriter`]
    /// suspends before letting a log line through.
    bars: Option<MultiProgress>,
    /// The bar for the phase now running.
    active: Mutex<Option<ProgressBar>>,
    /// The line-based fallback's counter for the phase now running.
    lines: Mutex<LineState>,
    /// When the phase now running began, for the `--verbose` phase timings.
    started: Mutex<Option<Instant>>,
}

impl Ui {
    /// Build the presentation `--quiet` and the shape of stderr call for.
    pub fn new(quiet: bool) -> Self {
        Self::with_mode(Mode::choose(quiet, io::stderr().is_terminal()))
    }

    fn with_mode(mode: Mode) -> Self {
        Self {
            mode,
            bars: (mode == Mode::Bars).then(MultiProgress::new),
            active: Mutex::new(None),
            lines: Mutex::new(LineState::default()),
            started: Mutex::new(None),
        }
    }

    /// The `tracing` writer that cooperates with the bars.
    pub fn log_writer(&self) -> LogWriter {
        LogWriter {
            bars: self.bars.clone(),
        }
    }

    /// Print a line that *is* the answer — where the mp4 went, which adapter drew
    /// it — on stdout, above whatever is being drawn.
    pub fn report(&self, line: &str) {
        if self.mode == Mode::Silent {
            return;
        }
        self.suspend(|| println!("{line}"));
    }

    /// Run `draw` with the bars cleared off the screen, so its output lands on a
    /// line of its own and the bars come back beneath it.
    fn suspend<R>(&self, draw: impl FnOnce() -> R) -> R {
        match &self.bars {
            Some(bars) => bars.suspend(draw),
            None => draw(),
        }
    }

    /// A mutex whose holder panicked has left an integer counter behind, not a
    /// broken invariant. Progress reporting must not turn that into a second
    /// panic on top of the first.
    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Progress for Ui {
    fn phase_started(&self, phase: Phase, total: Option<u64>) {
        *Self::lock(&self.started) = Some(Instant::now());

        match self.mode {
            Mode::Silent => {}
            Mode::Lines => {
                *Self::lock(&self.lines) = LineState::start(total);
                eprintln!("{}", start_line(phase, total));
            }
            Mode::Bars => {
                let bar = match total {
                    Some(total) => ProgressBar::new(total),
                    None => ProgressBar::new_spinner(),
                };
                bar.set_style(style(template(total)));
                bar.set_message(phase.label());
                if total.is_none() {
                    bar.enable_steady_tick(TICK);
                }

                let bar = match &self.bars {
                    Some(bars) => bars.add(bar),
                    None => bar,
                };
                *Self::lock(&self.active) = Some(bar);
            }
        }
    }

    fn advance(&self, phase: Phase, units: u64) {
        match self.mode {
            Mode::Silent => {}
            Mode::Lines => {
                if let Some(line) = Self::lock(&self.lines).advance(phase, units) {
                    eprintln!("{line}");
                }
            }
            Mode::Bars => {
                if let Some(bar) = Self::lock(&self.active).as_ref() {
                    bar.inc(units);
                }
            }
        }
    }

    fn phase_finished(&self, phase: Phase) {
        if let Some(started) = Self::lock(&self.started).take() {
            tracing::debug!(
                phase = phase.label(),
                elapsed_ms = started.elapsed().as_millis(),
                "phase finished"
            );
        }

        if let Some(bar) = Self::lock(&self.active).take() {
            bar.finish();
        }
    }

    fn warn(&self, message: &str) {
        if self.mode == Mode::Silent {
            return;
        }
        self.suspend(|| eprintln!("warning: {message}"));
    }

    fn adapter_selected(&self, kind: AdapterKind, name: &str) {
        self.report(&announce(kind, name));
    }
}

/// The one line that says who is doing the drawing.
///
/// Printed before the first frame, because hardware-versus-software is the
/// difference between a render that takes minutes and one that takes the
/// evening (`VISION.md` §7) — the user should learn which they got at the
/// start, not from the clock.
fn announce(kind: AdapterKind, name: &str) -> String {
    match kind {
        AdapterKind::Hardware => format!("rendering on {name} — hardware GPU"),
        AdapterKind::Software => {
            format!("rendering on {name} — software rasterizer, no GPU")
        }
    }
}

/// A `tracing` writer that clears the progress bars before writing.
///
/// `tracing_subscriber` formats one event into one writer, then drops it, so the
/// whole record is buffered and emitted once — a bar suspended per `write` call
/// would flicker for every field of every event.
#[derive(Debug, Clone)]
pub struct LogWriter {
    bars: Option<MultiProgress>,
}

impl<'a> MakeWriter<'a> for LogWriter {
    type Writer = LogRecord;

    fn make_writer(&'a self) -> Self::Writer {
        LogRecord {
            bars: self.bars.clone(),
            buffer: Vec::new(),
        }
    }
}

/// One buffered log record, flushed to stderr when the formatter drops it.
#[derive(Debug)]
pub struct LogRecord {
    bars: Option<MultiProgress>,
    buffer: Vec<u8>,
}

impl LogRecord {
    fn emit(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        let record = std::mem::take(&mut self.buffer);
        let write = || {
            let mut stderr = io::stderr().lock();
            stderr.write_all(&record)?;
            stderr.flush()
        };

        match &self.bars {
            Some(bars) => bars.suspend(write),
            None => write(),
        }
    }
}

impl io::Write for LogRecord {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.emit()
    }
}

impl Drop for LogRecord {
    fn drop(&mut self) {
        let _ = self.emit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--quiet` is errors only, whatever stderr is; a bar needs a terminal to
    /// redraw itself on, and a pipe gets lines instead (`VISION.md` §3).
    #[test]
    fn a_bar_is_drawn_only_on_a_terminal_and_never_when_quiet() {
        assert_eq!(Mode::choose(false, true), Mode::Bars);
        assert_eq!(Mode::choose(false, false), Mode::Lines);
        assert_eq!(Mode::choose(true, true), Mode::Silent);
        assert_eq!(Mode::choose(true, false), Mode::Silent);
    }

    /// A template `indicatif` rejects panics on the first frame of the first
    /// render, long after every unit test has passed.
    #[test]
    fn every_template_is_one_indicatif_accepts() {
        for template in [BAR_TEMPLATE, SPINNER_TEMPLATE] {
            assert!(
                ProgressStyle::with_template(template).is_ok(),
                "indicatif rejects: {template}"
            );
        }
    }

    /// `VISION.md` §3: "a progress bar with phase / frame count / render fps /
    /// ETA". Each of those is a placeholder, and dropping one is silent.
    #[test]
    fn the_rendering_bar_shows_the_phase_frame_count_render_fps_and_eta() {
        for placeholder in ["{msg", "{pos}", "{len}", "{fps}", "{eta}"] {
            assert!(
                BAR_TEMPLATE.contains(placeholder),
                "the rendering bar drops {placeholder}: {BAR_TEMPLATE}",
            );
        }
        assert!(SPINNER_TEMPLATE.contains("{msg"), "{SPINNER_TEMPLATE}");
    }

    /// A terminal that keeps what was drawn on it, so the bar can be read back.
    ///
    /// Cloned rather than shared by reference: `ProgressDrawTarget::term_like`
    /// takes ownership of the terminal, and the test still has to read it.
    #[derive(Debug, Default, Clone)]
    struct Capture(std::sync::Arc<Mutex<String>>);

    impl Capture {
        fn drawn(&self) -> String {
            Ui::lock(&self.0).clone()
        }
    }

    impl indicatif::TermLike for Capture {
        fn width(&self) -> u16 {
            120
        }
        fn move_cursor_up(&self, _n: usize) -> io::Result<()> {
            Ok(())
        }
        fn move_cursor_down(&self, _n: usize) -> io::Result<()> {
            Ok(())
        }
        fn move_cursor_right(&self, _n: usize) -> io::Result<()> {
            Ok(())
        }
        fn move_cursor_left(&self, _n: usize) -> io::Result<()> {
            Ok(())
        }
        fn write_line(&self, line: &str) -> io::Result<()> {
            Ui::lock(&self.0).push_str(line);
            Ok(())
        }
        fn write_str(&self, text: &str) -> io::Result<()> {
            Ui::lock(&self.0).push_str(text);
            Ok(())
        }
        fn clear_line(&self) -> io::Result<()> {
            Ok(())
        }
        fn flush(&self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The template is the contract, and `indicatif` renders an *unregistered*
    /// key as the empty string rather than as an error. `{fps}` is ours, so a
    /// `style()` that forgot to bind it would drop the render rate out of every
    /// bar and no test of the template string alone would notice.
    #[test]
    fn the_rendering_bar_draws_its_frame_count_render_fps_and_eta() {
        let capture = Capture::default();
        let bar = ProgressBar::with_draw_target(
            Some(30),
            indicatif::ProgressDrawTarget::term_like(Box::new(capture.clone())),
        );
        bar.set_style(style(BAR_TEMPLATE));
        bar.set_message(Phase::Rendering.label());
        bar.inc(15);
        bar.tick();

        let drawn = capture.drawn();
        assert!(drawn.contains("rendering"), "no phase: {drawn:?}");
        assert!(drawn.contains("15/30 frames"), "no frame count: {drawn:?}");
        assert!(drawn.contains("fps"), "no render rate: {drawn:?}");
        assert!(drawn.contains("eta"), "no eta: {drawn:?}");
    }

    /// A rate nobody can compute yet reads as unknown, never as `inf` or `NaN`.
    #[test]
    fn a_render_rate_that_cannot_be_computed_yet_reads_as_unknown() {
        assert_eq!(fps_text(39.111_4), "39.1 fps");
        assert_eq!(fps_text(0.0), "-- fps");
        assert_eq!(fps_text(f64::INFINITY), "-- fps");
        assert_eq!(fps_text(f64::NAN), "-- fps");
    }

    /// A phase whose total is known gets a bar; one whose total is not gets a
    /// spinner, because a bar with no length draws an empty trough forever.
    #[test]
    fn a_phase_of_unknown_length_gets_a_spinner_rather_than_an_empty_bar() {
        assert_eq!(template(Some(60)), BAR_TEMPLATE);
        assert_eq!(template(None), SPINNER_TEMPLATE);
    }

    /// The one thing that could panic at runtime, exercised.
    #[test]
    fn both_styles_build() {
        style(BAR_TEMPLATE);
        style(SPINNER_TEMPLATE);
    }

    #[test]
    fn a_phase_start_line_names_the_phase_and_its_frame_count() {
        assert_eq!(
            start_line(Phase::Rendering, Some(60)),
            "rendering 60 frames"
        );
        assert_eq!(start_line(Phase::Analyzing, None), "analyzing");
    }

    /// One line per decile, and nothing between them: a 9000-frame render must
    /// not write 9000 lines into a CI log.
    #[test]
    fn the_line_fallback_reports_once_per_decile_of_progress() {
        let mut state = LineState::start(Some(60));

        let lines: Vec<String> = (0..60)
            .filter_map(|_| state.advance(Phase::Rendering, 1))
            .collect();

        assert_eq!(
            lines,
            [
                "rendering 10% (6/60)",
                "rendering 20% (12/60)",
                "rendering 30% (18/60)",
                "rendering 40% (24/60)",
                "rendering 50% (30/60)",
                "rendering 60% (36/60)",
                "rendering 70% (42/60)",
                "rendering 80% (48/60)",
                "rendering 90% (54/60)",
                "rendering 100% (60/60)",
            ],
        );
    }

    /// The last line of a finished phase reads 100%, whatever the frame count
    /// does to the arithmetic.
    #[test]
    fn the_line_fallback_always_ends_at_a_hundred_percent() {
        for total in [1_u64, 3, 7, 60, 9_001] {
            let mut state = LineState::start(Some(total));
            // Collected rather than `.last()`: `advance` mutates, so the units
            // must be fed to it in order.
            let lines: Vec<String> = (0..total)
                .filter_map(|_| state.advance(Phase::Rendering, 1))
                .collect();

            assert_eq!(
                lines.last().map(String::as_str),
                Some(format!("rendering 100% ({total}/{total})").as_str()),
                "a phase of {total} frames never reported finishing",
            );
        }
    }

    /// A phase with no total has no percentage. Reporting one would mean
    /// dividing by zero, and reporting `0%` forever would mean nothing.
    #[test]
    fn a_phase_of_unknown_length_reports_no_percentage() {
        let mut state = LineState::start(None);

        assert_eq!(state.advance(Phase::Analyzing, 1), None);
        assert_eq!(state.advance(Phase::Analyzing, 1_000), None);
    }

    /// A phase cannot report past its own end, however many units arrive.
    #[test]
    fn progress_beyond_the_total_still_reads_a_hundred_percent() {
        let mut state = LineState::start(Some(10));

        assert_eq!(
            state.advance(Phase::Rendering, 999),
            Some("rendering 100% (10/10)".to_owned()),
        );
    }

    /// The announcement names the adapter and says plainly which side of the
    /// hardware/software line it falls on — that distinction is minutes versus
    /// hours on a long render (`VISION.md` §7).
    #[test]
    fn a_hardware_adapter_is_announced_as_a_real_gpu() {
        assert_eq!(
            announce(AdapterKind::Hardware, "AMD Radeon 780M"),
            "rendering on AMD Radeon 780M — hardware GPU"
        );
    }

    #[test]
    fn a_software_adapter_is_announced_as_cpu_emulation() {
        assert_eq!(
            announce(AdapterKind::Software, "llvmpipe (LLVM 19.1.7, 256 bits)"),
            "rendering on llvmpipe (LLVM 19.1.7, 256 bits) — software rasterizer, no GPU"
        );
    }

    /// `--quiet` suppresses everything but errors (`VISION.md` §3), and neither
    /// a warning nor an adapter announcement is an error. The three phase
    /// callbacks must also survive being called with no bar to draw on.
    #[test]
    fn a_silent_ui_draws_nothing_and_still_accepts_every_callback() {
        let ui = Ui::with_mode(Mode::Silent);

        ui.phase_started(Phase::Rendering, Some(2));
        ui.advance(Phase::Rendering, 1);
        ui.warn("something the user could act on");
        ui.adapter_selected(AdapterKind::Software, "llvmpipe");
        ui.phase_finished(Phase::Rendering);

        assert!(ui.bars.is_none(), "a silent ui draws no bars");
    }

    /// The line fallback holds no `MultiProgress`, so its log writer has nothing
    /// to suspend and writes straight through.
    #[test]
    fn only_a_bar_drawing_ui_suspends_its_log_writer() {
        assert!(Ui::with_mode(Mode::Bars).log_writer().bars.is_some());
        assert!(Ui::with_mode(Mode::Lines).log_writer().bars.is_none());
        assert!(Ui::with_mode(Mode::Silent).log_writer().bars.is_none());
    }
}
