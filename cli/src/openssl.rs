//! Host TLS adapter. Rust owns fixed buffers and pipes; OpenSSL owns TLS/DNS.
use core::{
    ffi::{c_char, c_int},
    fmt::Write as _,
};
use rustix::{
    fd::{AsRawFd, BorrowedFd, OwnedFd},
    io,
    pipe::{PipeFlags, pipe_with},
    process::{Pid, Signal, WaitOptions},
};

unsafe extern "C" {
    fn fork() -> c_int;
    fn dup2(old: c_int, new: c_int) -> c_int;
    fn execve(path: *const c_char, argv: *const *const c_char, envp: *const *const c_char)
    -> c_int;
    fn _exit(status: c_int) -> !;
}

pub struct Tls {
    child: Pid,
    rx: OwnedFd,
    tx: OwnedFd,
    timeout: core::time::Duration,
    read_timeout: core::time::Duration,
}
fn c_string<const N: usize>(text: &str) -> Result<heapless::Vec<u8, N>, &'static str> {
    if text.as_bytes().contains(&0) {
        return Err("TLS argument contains NUL");
    }
    let mut bytes = heapless::Vec::new();
    bytes
        .extend_from_slice(text.as_bytes())
        .map_err(|_| "TLS argument is too long")?;
    bytes.push(0).map_err(|_| "TLS argument is too long")?;
    Ok(bytes)
}
impl Tls {
    pub fn connect(
        host: &str,
        address: &str,
        port: u16,
        ca_file: Option<&str>,
        timeout: core::time::Duration,
        insecure: bool,
    ) -> Result<Self, &'static str> {
        let mut target = heapless::String::<512>::new();
        if address.contains(':') {
            write!(target, "[{address}]:{port}").map_err(|_| "TLS address is too long")?;
        } else {
            write!(target, "{address}:{port}").map_err(|_| "TLS address is too long")?;
        }
        let target = c_string::<513>(&target)?;
        let literal_ip = host.parse::<core::net::IpAddr>().is_ok();
        let host = c_string::<256>(host)?;
        let ca = ca_file.map(c_string::<4096>).transpose()?;
        let mut argv = heapless::Vec::<*const c_char, 20>::new();
        for value in [c"openssl", c"s_client", c"-quiet"] {
            let _ = argv.push(value.as_ptr());
        }
        if !insecure {
            let _ = argv.push(c"-verify_return_error".as_ptr());
            let _ = argv.push(
                if literal_ip {
                    c"-verify_ip"
                } else {
                    c"-verify_hostname"
                }
                .as_ptr(),
            );
            let _ = argv.push(host.as_ptr().cast());
        }
        let _ = argv.push(c"-servername".as_ptr());
        let _ = argv.push(host.as_ptr().cast());
        let _ = argv.push(c"-connect".as_ptr());
        let _ = argv.push(target.as_ptr().cast());
        if let Some(ca) = &ca {
            let _ = argv.push(c"-CAfile".as_ptr());
            let _ = argv.push(ca.as_ptr().cast());
        }
        let _ = argv.push(core::ptr::null());
        let environment = [c"LANG=C".as_ptr(), core::ptr::null()];
        let (parent_rx, child_tx) = pipe_with(PipeFlags::CLOEXEC).map_err(|_| "TLS pipe failed")?;
        let (child_rx, parent_tx) = pipe_with(PipeFlags::CLOEXEC).map_err(|_| "TLS pipe failed")?;
        for fd in [&parent_rx, &parent_tx] {
            rustix::fs::fcntl_setfl(fd, rustix::fs::OFlags::NONBLOCK)
                .map_err(|_| "TLS nonblocking pipe setup failed")?;
        }
        let null = rustix::fs::open(
            c"/dev/null",
            rustix::fs::OFlags::WRONLY | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|_| "TLS stderr setup failed")?;
        // SAFETY: this binary is single-threaded. The child only duplicates descriptors,
        // clears close-on-exec on stdio, and execs a fixed absolute executable, or exits.
        let pid = unsafe { fork() };
        if pid < 0 {
            return Err("TLS process creation failed");
        }
        if pid == 0 {
            // SAFETY: all source descriptors are live; standard targets are numeric FDs.
            if unsafe { dup2(child_rx.as_raw_fd(), 0) } < 0
                || unsafe { dup2(child_tx.as_raw_fd(), 1) } < 0
                || unsafe { dup2(null.as_raw_fd(), 2) } < 0
            {
                unsafe { _exit(126) }
            }
            for fd in 0..3 {
                // SAFETY: dup2 above established these three live standard descriptors.
                if io::fcntl_setfd(unsafe { BorrowedFd::borrow_raw(fd) }, io::FdFlags::empty())
                    .is_err()
                {
                    unsafe { _exit(126) }
                }
            }
            // SAFETY: argv/envp are NUL-terminated arrays of live NUL-terminated strings.
            unsafe {
                execve(
                    c"/usr/bin/openssl".as_ptr(),
                    argv.as_ptr(),
                    environment.as_ptr(),
                );
                _exit(127)
            }
        }
        let child = Pid::from_raw(pid).ok_or("invalid TLS child process")?;
        Ok(Self {
            child,
            rx: parent_rx,
            tx: parent_tx,
            timeout,
            read_timeout: timeout,
        })
    }
    fn ready(
        &self,
        fd: &OwnedFd,
        flags: rustix::event::PollFlags,
        duration: core::time::Duration,
    ) -> Result<(), embedded_io::ErrorKind> {
        let mut fds = [rustix::event::PollFd::new(fd, flags)];
        let timeout = rustix::event::Timespec {
            tv_sec: duration.as_secs() as i64,
            tv_nsec: duration.subsec_nanos() as i64,
        };
        loop {
            if crate::terminal::cancelled() {
                return Err(embedded_io::ErrorKind::Interrupted);
            }
            match rustix::event::poll(&mut fds, Some(&timeout)) {
                Ok(0) => return Err(embedded_io::ErrorKind::TimedOut),
                Ok(_) => return Ok(()),
                Err(io::Errno::INTR | io::Errno::AGAIN) => {}
                Err(_) => return Err(embedded_io::ErrorKind::Other),
            }
        }
    }
    pub fn set_read_timeout(&mut self, timeout: core::time::Duration) {
        self.read_timeout = timeout;
    }
}
impl embedded_io::ErrorType for Tls {
    type Error = embedded_io::ErrorKind;
}
impl embedded_io_async::Read for Tls {
    async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        loop {
            self.ready(&self.rx, rustix::event::PollFlags::IN, self.read_timeout)?;
            match io::read(&self.rx, &mut *bytes) {
                Ok(n) => return Ok(n),
                Err(io::Errno::INTR | io::Errno::AGAIN) => {}
                Err(_) => return Err(embedded_io::ErrorKind::Other),
            }
        }
    }
}
impl embedded_io_async::Write for Tls {
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        loop {
            self.ready(&self.tx, rustix::event::PollFlags::OUT, self.timeout)?;
            match io::write(&self.tx, bytes) {
                Ok(n) => return Ok(n),
                Err(io::Errno::INTR | io::Errno::AGAIN) => {}
                Err(_) => return Err(embedded_io::ErrorKind::Other),
            }
        }
    }
    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
impl Drop for Tls {
    fn drop(&mut self) {
        let _ = rustix::process::kill_process(self.child, Signal::TERM);
        while let Err(io::Errno::INTR) =
            rustix::process::waitpid(Some(self.child), WaitOptions::empty())
        {}
    }
}
