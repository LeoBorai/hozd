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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

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
