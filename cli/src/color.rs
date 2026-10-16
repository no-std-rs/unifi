use crate::Options;
use core::fmt::{self, Write};
pub fn enabled(options: &Options<'_>) -> bool {
    if options.theme == Some("plain") {
        return false;
    }
    match options.color.unwrap_or("auto") {
        "always" => true,
        "never" => false,
        _ => {
            // SAFETY: stdout stays open throughout the CLI.
            rustix::termios::isatty(unsafe { rustix::stdio::stdout() })
        }
    }
}
pub fn accent(theme: Option<&str>) -> &'static str {
    match theme.unwrap_or("silkcircuit-neon") {
        "plain" => "",
        "nord" => "\x1b[38;2;136;192;208m",
        "dracula" => "\x1b[38;2;189;147;249m",
        _ => "\x1b[38;2;0;255;204m",
    }
}
pub struct Header<'a> {
    pub out: &'a mut dyn Write,
    pub accent: &'static str,
    pub first: bool,
}
impl Write for Header<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if !self.first {
            return self.out.write_str(text);
        }
        if let Some((line, rest)) = text.split_once('\n') {
            write!(self.out, "{}{line}\x1b[0m\n{rest}", self.accent)?;
            self.first = false;
            Ok(())
        } else {
            write!(self.out, "{}{text}", self.accent)
        }
    }
}
