//! Restored-on-drop raw terminal with bounded input and handled termination signals.
use core::{
    ffi::c_int,
    fmt,
    sync::atomic::{AtomicI32, Ordering},
};
use rustix::{
    fd::OwnedFd,
    fs::{self, Mode, OFlags},
    io,
    termios::{self, OptionalActions, Termios},
};
static INTERRUPTED: AtomicI32 = AtomicI32::new(0);
pub fn available() -> bool {
    // SAFETY: the single-threaded CLI keeps stdin open while checking its terminal status.
    termios::isatty(unsafe { crate::stdio::stdin() })
}
extern "C" fn interrupt(signal: c_int) {
    INTERRUPTED.store(signal, Ordering::Relaxed);
}
pub fn interrupted_code() -> i32 {
    128 + INTERRUPTED.load(Ordering::Relaxed).max(2)
}
pub fn cancelled() -> bool {
    INTERRUPTED.load(Ordering::Relaxed) != 0
}
pub struct Terminal {
    fd: OwnedFd,
    original: Termios,
    handlers: [usize; 4],
    active: bool,
    screen: bool,
}
impl Terminal {
    pub fn open() -> Result<Self, &'static str> {
        let fd = fs::open(c"/dev/tty", OFlags::RDWR | OFlags::CLOEXEC, Mode::empty())
            .map_err(|_| "this command needs an interactive terminal")?;
        let original = termios::tcgetattr(&fd).map_err(|_| "cannot inspect terminal")?;
        INTERRUPTED.store(0, Ordering::Relaxed);
        let mut terminal = Self {
            fd,
            original,
            handlers: [0; 4],
            active: false,
            screen: false,
        };
        for (i, signal) in [1, 2, 3, 15].into_iter().enumerate() {
            // SAFETY: the C signal handler only stores a lock-free atomic integer.
            terminal.handlers[i] =
                unsafe { crate::signal(signal, interrupt as *const () as usize) };
        }
        terminal.active = true;
        let mut raw = terminal.original.clone();
        raw.make_raw();
        raw.special_codes[termios::SpecialCodeIndex::VMIN] = 0;
        raw.special_codes[termios::SpecialCodeIndex::VTIME] = 1;
        termios::tcsetattr(&terminal.fd, OptionalActions::Now, &raw)
            .map_err(|_| "cannot configure terminal")?;
        Ok(terminal)
    }
    pub fn enter_screen(&mut self) -> Result<(), &'static str> {
        crate::write_fd(&self.fd, b"\x1b[?1049h\x1b[?25l")?;
        self.screen = true;
        Ok(())
    }
    pub fn dimensions(&self) -> (usize, usize) {
        let size = termios::tcgetwinsize(&self.fd).ok();
        (
            size.as_ref()
                .filter(|s| s.ws_row != 0)
                .map_or(24, |s| usize::from(s.ws_row))
                .clamp(8, 200),
            size.as_ref()
                .filter(|s| s.ws_col != 0)
                .map_or(100, |s| usize::from(s.ws_col))
                .clamp(20, 500),
        )
    }
    pub fn read(&self) -> Result<Option<u8>, &'static str> {
        if cancelled() {
            return Err("interrupted");
        }
        let mut byte = [0];
        match io::read(&self.fd, &mut byte) {
            Ok(0) => Ok(None),
            Ok(_) => Ok(Some(byte[0])),
            Err(io::Errno::INTR) => Ok(None),
            Err(_) => Err("terminal read failed"),
        }
    }
    pub fn line(
        &mut self,
        label: &str,
        default: &str,
    ) -> Result<heapless::String<8192>, &'static str> {
        use core::fmt::Write;
        write!(self, "{label} [{default}]: ").map_err(|_| "terminal output failed")?;
        let mut bytes = heapless::Vec::<u8, 8192>::new();
        loop {
            match self.read()? {
                None => {}
                Some(3) => {
                    interrupt(2);
                    return Err("interrupted");
                }
                Some(4 | 27) => return Err("cancelled"),
                Some(b'\r' | b'\n') => break,
                Some(127 | 8) => {
                    while let Some(b) = bytes.pop() {
                        if b & 0xc0 != 0x80 {
                            break;
                        }
                    }
                    crate::write_fd(&self.fd, b"\r\x1b[2K")?;
                    write!(self, "{label} [{default}]: ").map_err(|_| "terminal output failed")?;
                    crate::write_fd(&self.fd, &bytes)?;
                }
                Some(b) if b >= 32 => {
                    bytes.push(b).map_err(|_| "input exceeds capacity")?;
                    crate::write_fd(&self.fd, &[b])?;
                }
                Some(_) => {}
            }
        }
        self.write_str("\r\n")
            .map_err(|_| "terminal output failed")?;
        let text = if bytes.is_empty() {
            default
        } else {
            core::str::from_utf8(&bytes).map_err(|_| "input must be UTF-8")?
        };
        heapless::String::try_from(text).map_err(|_| "input exceeds capacity")
    }
    pub fn password(&mut self, label: &str) -> Result<heapless::String<8192>, &'static str> {
        use core::fmt::Write;
        write!(self, "{label}: ").map_err(|_| "terminal output failed")?;
        let mut bytes = heapless::Vec::<u8, 8192>::new();
        loop {
            match self.read()? {
                None => {}
                Some(3) => {
                    interrupt(2);
                    return Err("interrupted");
                }
                Some(4 | 27) => return Err("cancelled"),
                Some(b'\r' | b'\n') => break,
                Some(127 | 8) => {
                    while let Some(b) = bytes.pop() {
                        if b & 0xc0 != 0x80 {
                            break;
                        }
                    }
                }
                Some(0) => return Err("secret cannot contain NUL"),
                Some(b) => bytes.push(b).map_err(|_| "secret exceeds capacity")?,
            }
        }
        self.write_str("\r\n")
            .map_err(|_| "terminal output failed")?;
        let text = core::str::from_utf8(&bytes).map_err(|_| "secret must be UTF-8")?;
        heapless::String::try_from(text).map_err(|_| "secret exceeds capacity")
    }
}
impl fmt::Write for Terminal {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        crate::write_fd(&self.fd, text.as_bytes()).map_err(|_| fmt::Error)
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        if self.screen {
            let _ = crate::write_fd(&self.fd, b"\x1b[0m\x1b[?25h\x1b[?1049l");
        }
        if self.active {
            let _ = termios::tcsetattr(&self.fd, OptionalActions::Now, &self.original);
            for (i, signal) in [1, 2, 3, 15].into_iter().enumerate() {
                // SAFETY: restore exactly the handler returned for this signal at open().
                unsafe {
                    crate::signal(signal, self.handlers[i]);
                }
            }
        }
    }
}
