//! Terminal progress bars (drawn on stderr, hidden automatically when it isn't a terminal).
//! With `--progress-lines` they are printed instead as `@@progress <stage>|<pos>|<len>` lines
//! on stdout, for front ends such as the Calibre plugin.

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle, TermLike};
use std::io;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const BAR: &str = "{prefix:>10.cyan.bold} [{bar:30.green/dim}] {pos}/{len} {msg} ({elapsed}, eta {eta})";
const SPIN: &str = "{prefix:>10.cyan.bold} {spinner} {msg} ({elapsed})";
const LINE: &str = "@@progress {prefix}|{pos}|{len}";

static LINES: AtomicBool = AtomicBool::new(false);

pub fn enable_lines() {
    LINES.store(true, Ordering::Relaxed);
}

fn lines() -> bool {
    LINES.load(Ordering::Relaxed)
}

fn style(template: &str) -> ProgressStyle {
    ProgressStyle::with_template(if lines() { LINE } else { template }).unwrap().progress_chars("=> ")
}

fn target() -> ProgressDrawTarget {
    if lines() { ProgressDrawTarget::term_like_with_hz(Box::new(LineTerm::default()), 4) } else { ProgressDrawTarget::stderr() }
}

pub fn spinner(prefix: &'static str, msg: &str) -> ProgressBar {
    let pb = ProgressBar::with_draw_target(None, target())
        .with_style(style(SPIN))
        .with_prefix(prefix)
        .with_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

pub fn bar(prefix: &'static str, len: usize, msg: &str) -> ProgressBar {
    ProgressBar::with_draw_target(Some(len as u64), target())
        .with_style(style(BAR))
        .with_prefix(prefix)
        .with_message(msg.to_string())
}

/// Turn a spinner into a bar once the total is known.
pub fn to_bar(pb: &ProgressBar, len: usize) {
    pb.set_length(len as u64);
    pb.set_style(style(BAR));
}

/// A fake terminal that prints each finished, changed progress line to stdout.
#[derive(Debug, Default)]
struct LineTerm {
    buf: Mutex<String>,
    last: Mutex<String>,
}

impl LineTerm {
    fn emit(&self) {
        let line = std::mem::take(&mut *self.buf.lock().unwrap());
        let line = line.trim();
        let mut last = self.last.lock().unwrap();
        if line.starts_with("@@progress") && *last != line {
            println!("{line}");
            *last = line.to_string();
        }
    }
}

impl TermLike for LineTerm {
    fn width(&self) -> u16 {
        200
    }
    fn move_cursor_up(&self, _: usize) -> io::Result<()> {
        Ok(())
    }
    fn move_cursor_down(&self, _: usize) -> io::Result<()> {
        Ok(())
    }
    fn move_cursor_right(&self, _: usize) -> io::Result<()> {
        Ok(())
    }
    fn move_cursor_left(&self, _: usize) -> io::Result<()> {
        Ok(())
    }
    fn write_line(&self, s: &str) -> io::Result<()> {
        self.buf.lock().unwrap().push_str(s);
        self.emit();
        Ok(())
    }
    fn write_str(&self, s: &str) -> io::Result<()> {
        self.buf.lock().unwrap().push_str(s);
        Ok(())
    }
    fn clear_line(&self) -> io::Result<()> {
        Ok(())
    }
    fn flush(&self) -> io::Result<()> {
        self.emit();
        Ok(())
    }
}
