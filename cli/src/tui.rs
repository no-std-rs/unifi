//! Keyboard dashboard; command palette invokes the same CLI without a shell.
use crate::{
    Options, color, process,
    terminal::{self, Terminal},
};
use core::fmt::Write;
use heapless::{String, Vec};
const VIEWS: [(&str, &str); 10] = [
    ("Devices", "devices list --all"),
    ("Clients", "clients list --all"),
    ("Wi-Fi", "wifi list --all"),
    ("Networks", "networks list --all"),
    ("Topology", "topology"),
    ("Events", "events list"),
    ("Alarms", "alarms list"),
    ("Settings", "settings list"),
    ("System", "system info"),
    ("Cloud", "cloud hosts --all"),
];
fn now() -> i64 {
    rustix::time::clock_gettime(rustix::time::ClockId::Monotonic).tv_sec
}
/// Shell-style quoting for argv only. No expansion, substitution, shell or pipelines.
fn tokens<'a>(text: &str, buffer: &'a mut [u8]) -> Result<Vec<&'a str, 128>, &'static str> {
    let mut spans = Vec::<(usize, usize), 128>::new();
    let mut start = None;
    let mut used = 0;
    let mut quote = None;
    let mut escape = false;
    for b in text.bytes() {
        if b == 0 || b.is_ascii_control() && b != b'\t' {
            return Err("command contains a control character");
        }
        if !escape && quote != Some(b'\'') && b == b'\\' {
            start.get_or_insert(used);
            escape = true;
            continue;
        }
        if !escape && (b == b'\'' || b == b'"') {
            if quote == Some(b) {
                quote = None;
                continue;
            }
            if quote.is_none() {
                start.get_or_insert(used);
                quote = Some(b);
                continue;
            }
        }
        if !escape && quote.is_none() && b.is_ascii_whitespace() {
            if let Some(start) = start.take() {
                spans
                    .push((start, used))
                    .map_err(|_| "too many command arguments")?;
            }
            continue;
        }
        start.get_or_insert(used);
        *buffer.get_mut(used).ok_or("command exceeds capacity")? = b;
        used += 1;
        escape = false;
    }
    if escape || quote.is_some() {
        return Err("unclosed quote or escape in command");
    }
    if let Some(start) = start {
        spans
            .push((start, used))
            .map_err(|_| "too many command arguments")?;
    }
    let mut args = Vec::new();
    for (start, end) in spans {
        args.push(core::str::from_utf8(&buffer[start..end]).map_err(|_| "command must be UTF-8")?)
            .map_err(|_| "too many command arguments")?;
    }
    if args.is_empty() {
        return Err("command is empty");
    }
    if args[0] == "tui" {
        return Err("the command palette cannot open another TUI");
    }
    Ok(args)
}
fn execute(
    command: &str,
    options: &Options<'_>,
    environment: &str,
    output: &mut [u8],
) -> Result<(usize, i32), &'static str> {
    let mut buffer = [0; 8192];
    let words = tokens(command, &mut buffer)?;
    let mut timeout = String::<20>::new();
    let mut args = Vec::<&str, 128>::new();
    // Stable parent context followed by the selected command, whose explicit flags win.
    if options.insecure_explicit && !options.insecure {
        args.extend_from_slice(&["--insecure", "false"])
            .map_err(|_| "too many arguments")?;
    }
    for (name, value) in [
        ("--controller", options.url),
        ("--config", options.config),
        ("--profile", options.profile),
        ("--site", options.site),
        ("--console", options.console),
        ("--api-key", options.api_key),
        ("--cookie-file", options.cookie_file),
        ("--tls", options.tls),
        ("--ca-file", options.ca_file),
        ("--ca-cert", options.ca_cert),
        ("--connect", options.connect_ip),
    ] {
        if let Some(value) = value {
            args.push(name)
                .map_err(|_| "too many TUI context arguments")?;
            args.push(value)
                .map_err(|_| "too many TUI context arguments")?;
        }
    }
    if let Some(value) = options.timeout {
        write!(timeout, "{value}").unwrap();
        args.push("--timeout")
            .map_err(|_| "too many TUI arguments")?;
        args.push(&timeout).map_err(|_| "too many TUI arguments")?;
    }
    for (flag, set) in [
        ("--allow-write", options.allow_write),
        ("--demo", options.demo),
        ("--insecure", options.insecure),
        ("--standalone", options.standalone),
    ] {
        if set {
            args.push(flag).map_err(|_| "too many TUI arguments")?;
        }
    }
    args.push("--color").map_err(|_| "too many TUI arguments")?;
    args.push("never").map_err(|_| "too many TUI arguments")?;
    for word in words {
        args.push(word).map_err(|_| "too many TUI arguments")?;
    }
    process::run(
        "/proc/self/exe",
        &args,
        environment,
        &[],
        output,
        options.timeout.unwrap_or(30).saturating_add(5),
        true,
        terminal::cancelled,
    )
}
fn contains(line: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut lower = String::<8192>::new();
    for c in line.chars() {
        if lower.push(c.to_ascii_lowercase()).is_err() {
            return false;
        }
    }
    let mut needle = String::<8192>::new();
    for c in query.chars() {
        if needle.push(c.to_ascii_lowercase()).is_err() {
            return false;
        }
    }
    lower.contains(needle.as_str())
}
struct Frame<'a> {
    title: &'a str,
    text: &'a str,
    status: i32,
    scroll: usize,
    filter: &'a str,
    mode: Option<u8>,
    input: &'a [u8],
}
fn draw(tty: &mut Terminal, frame: &Frame<'_>, options: &Options<'_>) -> Result<(), &'static str> {
    let (height, width) = tty.dimensions();
    let accent = if options.color == Some("never") {
        ""
    } else {
        color::accent(options.theme)
    };
    write!(
        tty,
        "\x1b[H\x1b[2J{accent}UniFi  ·  {}\x1b[0m\r\n",
        frame.title
    )
    .map_err(|_| "terminal output failed")?;
    let tabs = "1 Devices  2 Clients  3 Wi-Fi  4 Networks  5 Topology  6 Events  7 Alarms  8 Settings  9 System  0 Cloud";
    for c in tabs.chars().take(width) {
        tty.write_char(c).map_err(|_| "terminal output failed")?;
    }
    tty.write_str("\r\n\r\n")
        .map_err(|_| "terminal output failed")?;
    let mut lines = frame
        .text
        .lines()
        .filter(|s| contains(s, frame.filter))
        .skip(frame.scroll);
    for _ in 0..height.saturating_sub(6) {
        if let Some(line) = lines.next() {
            for c in line.chars().take(width) {
                tty.write_char(if c.is_control() { ' ' } else { c })
                    .map_err(|_| "terminal output failed")?;
            }
        }
        tty.write_str("\r\n")
            .map_err(|_| "terminal output failed")?;
    }
    let prompt = if frame.mode == Some(b':') {
        "Command: "
    } else if frame.mode == Some(b'/') {
        "Search: "
    } else {
        "r refresh · j/k scroll · / search · : command · Tab next · q quit"
    };
    tty.write_str(accent)
        .map_err(|_| "terminal output failed")?;
    for c in prompt.chars().take(width) {
        tty.write_char(c).map_err(|_| "terminal output failed")?;
    }
    tty.write_str("\x1b[0m")
        .map_err(|_| "terminal output failed")?;
    if frame.mode.is_some() {
        let text = core::str::from_utf8(frame.input).unwrap_or("");
        for c in text.chars().take(width.saturating_sub(prompt.len())) {
            tty.write_char(if c.is_control() { ' ' } else { c })
                .map_err(|_| "terminal output failed")?;
        }
    }
    write!(tty, "\r\nexit {} · refreshed every 5s\x1b[K", frame.status)
        .map_err(|_| "terminal output failed")?;
    Ok(())
}
fn log(options: &Options<'_>, status: i32) -> Result<(), &'static str> {
    if let Some(path) = options.log_file {
        let fd = rustix::fs::open(
            path,
            rustix::fs::OFlags::WRONLY
                | rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::APPEND
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .map_err(|_| "cannot open TUI log")?;
        let mut line = String::<128>::new();
        writeln!(line, "monotonic={} exit={status}", now()).unwrap();
        crate::write_fd(&fd, line.as_bytes())?;
    }
    Ok(())
}
pub fn run(
    options: &Options<'_>,
    environment: &str,
    output: &mut [u8],
) -> Result<i32, &'static str> {
    let mut tty = Terminal::open()?;
    tty.enter_screen()?;
    let mut selected = 0;
    let mut command = String::<8192>::try_from(VIEWS[0].1).unwrap();
    let mut custom = false;
    let mut input = Vec::<u8, 8192>::new();
    let mut mode = None;
    let mut filter = String::<8192>::new();
    let mut scroll = 0;
    let mut refresh = true;
    let mut redraw = true;
    let mut next = now();
    let mut status = 0;
    let mut used = 0;
    let mut snapshots = 0;
    loop {
        if terminal::cancelled() {
            return Ok(terminal::interrupted_code());
        }
        if refresh || mode.is_none() && now() >= next {
            match execute(&command, options, environment, output) {
                Ok((n, code)) => {
                    used = n;
                    status = code;
                }
                Err(error) => {
                    used = error.len().min(output.len());
                    output[..used].copy_from_slice(&error.as_bytes()[..used]);
                    status = 2;
                }
            }
            log(options, status)?;
            snapshots += 1;
            next = now() + 5;
            refresh = false;
            redraw = true;
        }
        if redraw {
            draw(
                &mut tty,
                &Frame {
                    title: if custom { "Command" } else { VIEWS[selected].0 },
                    text: core::str::from_utf8(&output[..used])
                        .unwrap_or("Binary output; use --save in the command palette."),
                    status,
                    scroll,
                    filter: &filter,
                    mode,
                    input: &input,
                },
                options,
            )?;
            redraw = false;
            if options.count.is_some_and(|count| snapshots >= count) {
                return Ok(status);
            }
        }
        let Some(key) = (match tty.read() {
            Ok(key) => key,
            Err(_) if terminal::cancelled() => return Ok(terminal::interrupted_code()),
            Err(error) => return Err(error),
        }) else {
            continue;
        };
        if let Some(active) = mode {
            match key {
                3 | 27 => {
                    mode = None;
                    input.clear();
                }
                b'\r' | b'\n' => {
                    if let Ok(value) = core::str::from_utf8(&input) {
                        if active == b':' {
                            command.clear();
                            command
                                .push_str(value)
                                .map_err(|_| "command exceeds capacity")?;
                            custom = true;
                            refresh = true;
                        } else {
                            filter.clear();
                            filter
                                .push_str(value)
                                .map_err(|_| "search exceeds capacity")?;
                        }
                    }
                    mode = None;
                    input.clear();
                    scroll = 0;
                }
                127 | 8 => {
                    while let Some(b) = input.pop() {
                        if b & 0xc0 != 0x80 {
                            break;
                        }
                    }
                }
                b if !b.is_ascii_control() => {
                    input.push(b).map_err(|_| "command exceeds capacity")?;
                }
                _ => {}
            }
        } else {
            match key {
                b'q' | 3 | 4 => return Ok(0),
                b'r' | b' ' => refresh = true,
                b'j' => scroll = scroll.saturating_add(1),
                b'k' => scroll = scroll.saturating_sub(1),
                b'g' => scroll = 0,
                b':' | b'/' => {
                    mode = Some(key);
                    input.clear();
                }
                b'\t' | b'0'..=b'9' => {
                    selected = if key == b'\t' {
                        (selected + 1) % VIEWS.len()
                    } else if key == b'0' {
                        9
                    } else {
                        (key - b'1') as usize
                    };
                    command.clear();
                    command.push_str(VIEWS[selected].1).unwrap();
                    custom = false;
                    refresh = true;
                    scroll = 0;
                    filter.clear();
                }
                _ => {}
            }
        }
        redraw = true;
    }
}
