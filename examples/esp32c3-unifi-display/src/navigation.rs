//! Touch gestures and page rotation, independent of the board clock and I/O.
use crate::render::PAGE_COUNT;

const TAP_WINDOW_MS: u64 = 650;
const PAGE_MS: u64 = 8000;

pub struct Navigation {
    page: usize,
    locked: bool,
    page_since: u64,
    down_since: Option<u64>,
    first_tap: u64,
    taps: usize,
}

impl Navigation {
    pub fn new(now_ms: u64) -> Self {
        Self {
            page: 0,
            locked: false,
            page_since: now_ms,
            down_since: None,
            first_tap: 0,
            taps: 0,
        }
    }
    pub fn page(&self) -> usize {
        self.page
    }
    pub fn locked(&self) -> bool {
        self.locked
    }

    /// A failed touch read is `None`, preserving the previous finger state.
    /// Wait for the gesture window before applying one/two taps, so triple-tap
    /// locks the page that was visible when the gesture began.
    pub fn update(&mut self, now_ms: u64, touching: Option<bool>) -> bool {
        let previous = (self.page, self.locked);
        if self.taps > 0 && now_ms.saturating_sub(self.first_tap) > TAP_WINDOW_MS {
            if !self.locked {
                self.page = (self.page + self.taps) % PAGE_COUNT;
                self.page_since = now_ms;
            }
            self.taps = 0;
        }
        match touching {
            Some(true) if self.down_since.is_none() => self.down_since = Some(now_ms),
            Some(false)
                if self
                    .down_since
                    .take()
                    .is_some_and(|start| now_ms.saturating_sub(start) >= 20) =>
            {
                if self.taps == 0 {
                    self.first_tap = now_ms;
                }
                self.taps += 1;
                if self.taps == 3 {
                    self.locked = !self.locked;
                    self.taps = 0;
                    self.page_since = now_ms;
                }
            }
            _ => {}
        }
        if !self.locked
            && self.taps == 0
            && self.down_since.is_none()
            && now_ms.saturating_sub(self.page_since) >= PAGE_MS
        {
            self.page = (self.page + 1) % PAGE_COUNT;
            self.page_since = now_ms;
        }
        previous != (self.page, self.locked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tap(n: &mut Navigation, at: u64) {
        n.update(at, Some(true));
        n.update(at + 40, Some(false));
    }
    #[test]
    fn triple_tap_locks_current_page_and_unlocks_without_advancing_it() {
        let mut n = Navigation::new(0);
        n.update(8000, None);
        assert_eq!(n.page(), 1);
        for at in [8100, 8300, 8500] {
            tap(&mut n, at);
        }
        assert!(n.locked());
        assert_eq!(n.page(), 1);
        tap(&mut n, 9000);
        n.update(10000, Some(false));
        n.update(60000, None);
        assert_eq!(n.page(), 1);
        for at in [61000, 61200, 61400] {
            tap(&mut n, at);
        }
        assert!(!n.locked());
        assert_eq!(n.page(), 1);
        assert!(!n.update(69439, None));
        assert!(n.update(69440, None));
        assert_eq!(n.page(), 2);
    }
    #[test]
    fn single_and_double_tap_wait_for_the_gesture_window() {
        let mut n = Navigation::new(0);
        tap(&mut n, 100);
        assert_eq!(n.page(), 0);
        n.update(791, None);
        assert_eq!(n.page(), 1);
        tap(&mut n, 1000);
        tap(&mut n, 1200);
        n.update(1691, None);
        assert_eq!(n.page(), 3);
        assert!(!n.locked());
    }
    #[test]
    fn pending_touch_suppresses_rotation_and_failed_reads_do_not_create_taps() {
        let mut n = Navigation::new(0);
        n.update(7900, Some(true));
        n.update(8100, None);
        assert_eq!(n.page(), 0);
        n.update(8200, Some(false));
        assert_eq!(n.page(), 0);
        n.update(8851, None);
        assert_eq!(n.page(), 1);
        n.update(9000, Some(true));
        n.update(9001, Some(false));
        n.update(10000, None);
        assert_eq!(n.page(), 1);
    }
}
