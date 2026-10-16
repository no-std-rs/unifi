//! Bounded subprocess I/O. No shell, heap, inherited stdin, or unbounded waits.
use core::ffi::{c_char, c_int};
use rustix::{
    event::{PollFd, PollFlags},
    fd::{AsRawFd, BorrowedFd},
    io,
    pipe::{PipeFlags, pipe_with},
    process::{Pid, Signal, WaitOptions},
};
unsafe extern "C" {
    fn fork() -> c_int;
    fn dup2(old: c_int, new: c_int) -> c_int;
    fn execve(path: *const c_char, args: *const *const c_char, env: *const *const c_char) -> c_int;
    fn _exit(code: c_int) -> !;
}
struct Arena {
    bytes: [u8; 65536],
    used: usize,
}
impl Arena {
    fn push(&mut self, text: &str) -> Result<*const c_char, &'static str> {
        if text.as_bytes().contains(&0) {
            return Err("process argument contains NUL");
        }
        let start = self.used;
        let end = start
            .checked_add(text.len() + 1)
            .filter(|n| *n <= self.bytes.len())
            .ok_or("process arguments exceed capacity")?;
        self.bytes[start..end - 1].copy_from_slice(text.as_bytes());
        self.bytes[end - 1] = 0;
        self.used = end;
        Ok(self.bytes[start..].as_ptr().cast())
    }
}
struct Child(Option<Pid>);
impl Drop for Child {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let _ = rustix::process::kill_process(pid, Signal::KILL);
            while let Err(io::Errno::INTR) =
                rustix::process::waitpid(Some(pid), WaitOptions::empty())
            {}
        }
    }
}
fn now() -> i64 {
    rustix::time::clock_gettime(rustix::time::ClockId::Monotonic).tv_sec
}
/// Capture at most `output.len()` bytes. A nonzero child status is returned to the caller.
#[allow(clippy::too_many_arguments)]
pub fn run(
    path: &str,
    args: &[&str],
    environment: &str,
    input: &[u8],
    output: &mut [u8],
    timeout: u64,
    capture_errors: bool,
    cancelled: fn() -> bool,
) -> Result<(usize, i32), &'static str> {
    if !path.starts_with('/') {
        return Err("process executable must be an absolute path");
    }
    let mut arena = Arena {
        bytes: [0; 65536],
        used: 0,
    };
    let path = arena.push(path)?;
    let mut argv = heapless::Vec::<*const c_char, 130>::new();
    argv.push(path).unwrap();
    for arg in args {
        argv.push(arena.push(arg)?)
            .map_err(|_| "too many process arguments")?;
    }
    argv.push(core::ptr::null())
        .map_err(|_| "too many process arguments")?;
    let mut env = heapless::Vec::<*const c_char, 130>::new();
    env.push(arena.push("LANG=C.UTF-8")?).unwrap();
    for item in environment
        .split('\0')
        .filter(|s| !s.is_empty() && !s.starts_with("LANG="))
    {
        if !item.contains('=') {
            return Err("invalid process environment");
        }
        env.push(arena.push(item)?)
            .map_err(|_| "too many environment entries")?;
    }
    env.push(core::ptr::null())
        .map_err(|_| "too many environment entries")?;
    let (rx, child_tx) = pipe_with(PipeFlags::CLOEXEC).map_err(|_| "process pipe failed")?;
    let (child_rx, tx) = pipe_with(PipeFlags::CLOEXEC).map_err(|_| "process pipe failed")?;
    let null = rustix::fs::open(
        c"/dev/null",
        rustix::fs::OFlags::WRONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| "process stderr setup failed")?;
    // SAFETY: the single-threaded CLI has prepared all strings and descriptors. The
    // child only performs async-signal-safe descriptor operations and execve/_exit.
    let raw = unsafe { fork() };
    if raw < 0 {
        return Err("process creation failed");
    }
    if raw == 0 {
        let err = if capture_errors {
            child_tx.as_raw_fd()
        } else {
            null.as_raw_fd()
        };
        if unsafe { dup2(child_rx.as_raw_fd(), 0) } < 0
            || unsafe { dup2(child_tx.as_raw_fd(), 1) } < 0
            || unsafe { dup2(err, 2) } < 0
        {
            unsafe { _exit(126) }
        }
        for fd in 0..3 {
            // SAFETY: dup2 established the standard descriptors above.
            if io::fcntl_setfd(unsafe { BorrowedFd::borrow_raw(fd) }, io::FdFlags::empty()).is_err()
            {
                unsafe { _exit(126) }
            }
        }
        // SAFETY: the fixed arena and pointer arrays remain live and NUL-terminated.
        unsafe {
            execve(path, argv.as_ptr(), env.as_ptr());
            _exit(127)
        }
    }
    let pid = Pid::from_raw(raw).ok_or("invalid process ID")?;
    let mut child = Child(Some(pid));
    drop(child_rx);
    drop(child_tx);
    drop(null);
    rustix::fs::fcntl_setfl(
        &rx,
        rustix::fs::fcntl_getfl(&rx).map_err(|_| "process flags failed")?
            | rustix::fs::OFlags::NONBLOCK,
    )
    .map_err(|_| "process flags failed")?;
    rustix::fs::fcntl_setfl(
        &tx,
        rustix::fs::fcntl_getfl(&tx).map_err(|_| "process flags failed")?
            | rustix::fs::OFlags::NONBLOCK,
    )
    .map_err(|_| "process flags failed")?;
    let mut tx = Some(tx);
    if input.is_empty() {
        tx.take();
    }
    let (mut written, mut used) = (0, 0);
    let deadline = now().saturating_add(timeout.min(i64::MAX as u64) as i64);
    let mut eof = false;
    loop {
        if cancelled() {
            return Err("process interrupted");
        }
        if now() >= deadline {
            return Err("process timed out");
        }
        if !eof {
            let mut poll = heapless::Vec::<PollFd<'_>, 2>::new();
            poll.push(PollFd::new(&rx, PollFlags::IN)).unwrap();
            if let Some(tx) = &tx {
                poll.push(PollFd::new(tx, PollFlags::OUT)).unwrap();
            }
            match rustix::event::poll(
                &mut poll,
                Some(&rustix::event::Timespec {
                    tv_sec: 1,
                    tv_nsec: 0,
                }),
            ) {
                Ok(_) => {}
                Err(io::Errno::INTR) => continue,
                Err(_) => return Err("process poll failed"),
            }
            let readable = !poll[0].revents().is_empty();
            let writable = poll.get(1).is_some_and(|fd| !fd.revents().is_empty());
            drop(poll);
            if readable {
                let mut probe = [0];
                let full = used == output.len();
                let bytes = if full {
                    &mut probe[..]
                } else {
                    &mut output[used..]
                };
                match io::read(&rx, bytes) {
                    Ok(0) => eof = true,
                    Ok(_) if full => return Err("process output exceeds capacity"),
                    Ok(n) => used += n,
                    Err(io::Errno::INTR | io::Errno::AGAIN) => {}
                    Err(_) => return Err("process read failed"),
                }
            }
            if writable && let Some(fd) = &tx {
                match io::write(fd, &input[written..]) {
                    Ok(0) => return Err("process write made no progress"),
                    Ok(n) => {
                        written += n;
                        if written == input.len() {
                            tx.take();
                        }
                    }
                    Err(io::Errno::INTR | io::Errno::AGAIN) => {}
                    Err(io::Errno::PIPE) => {
                        tx.take();
                    }
                    Err(_) => return Err("process write failed"),
                }
            }
        }
        if eof {
            match rustix::process::waitpid(Some(pid), WaitOptions::NOHANG) {
                Ok(Some((_, status))) => {
                    child.0 = None;
                    return Ok((
                        used,
                        status
                            .exit_status()
                            .unwrap_or(128 + status.terminating_signal().unwrap_or(0)),
                    ));
                }
                Ok(None) => {
                    let _ = rustix::event::poll(
                        &mut [],
                        Some(&rustix::event::Timespec {
                            tv_sec: 0,
                            tv_nsec: 10_000_000,
                        }),
                    );
                }
                Err(io::Errno::INTR) => {}
                Err(_) => return Err("process wait failed"),
            }
        }
    }
}
