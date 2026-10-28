/// Runs `restore` when dropped, so the terminal returns to its normal screen and mode
/// on every exit path. `ratatui::init` also installs a panic hook that restores it.
pub struct RestoreGuard<F: FnMut()> {
    restore: F,
}

impl<F: FnMut()> RestoreGuard<F> {
    pub fn new(restore: F) -> Self {
        Self { restore }
    }
}

impl<F: FnMut()> Drop for RestoreGuard<F> {
    fn drop(&mut self) {
        (self.restore)();
    }
}

/// Escape sequence asking the terminal to put `text` on the system clipboard (OSC 52).
/// Works over SSH; inside tmux it is wrapped so tmux forwards it to the outer terminal.
pub fn osc52(text: &str, in_tmux: bool) -> String {
    use base64::Engine as _;
    let payload = base64::engine::general_purpose::STANDARD.encode(text);
    let seq = format!("\x1b]52;c;{payload}\x07");
    if in_tmux {
        format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b"))
    } else {
        seq
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn osc52_encodes_text_as_base64() {
        assert_eq!(osc52("hi", false), "\x1b]52;c;aGk=\x07");
        assert_eq!(osc52("hi", true), "\x1bPtmux;\x1b\x1b]52;c;aGk=\x07\x1b\\");
    }

    #[test]
    fn restores_on_drop_and_on_unwind() {
        let calls = Cell::new(0);
        drop(RestoreGuard::new(|| calls.set(calls.get() + 1)));
        assert_eq!(calls.get(), 1);

        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = RestoreGuard::new(|| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            });
            panic!("boom");
        }));
        assert!(result.is_err());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
