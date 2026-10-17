//! 240×240 round-screen layout. Can draw into a small tile or a host preview.

use crate::stats::Dashboard;
use alloc::{format, string::String};
use embedded_graphics::{
    mono_font::{
        ascii::{FONT_6X10, FONT_9X18_BOLD},
        MonoTextStyle,
    },
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle, Rectangle},
    text::{Alignment, Text},
};

pub const PAGE_COUNT: usize = 5;
pub const BACKGROUND: Rgb565 = Rgb565::new(1, 3, 6);
const WHITE: Rgb565 = Rgb565::new(29, 59, 30);
const MUTED: Rgb565 = Rgb565::new(14, 31, 18);
const DOWN: Rgb565 = Rgb565::new(6, 57, 23);
const UP: Rgb565 = Rgb565::new(31, 35, 9);
const ACCENT: Rgb565 = Rgb565::new(29, 14, 23);

fn short(s: &str, max: usize) -> String {
    let mut out: String = s
        .chars()
        .take(max)
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '?'
            }
        })
        .collect();
    if s.chars().count() > max && max >= 3 {
        out.truncate(max - 3);
        out.push_str("...");
    }
    out
}

fn text<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    s: &str,
    x: i32,
    y: i32,
    color: Rgb565,
    align: Alignment,
) -> Result<(), D::Error> {
    Text::with_alignment(
        s,
        Point::new(x, y),
        MonoTextStyle::new(&FONT_6X10, color),
        align,
    )
    .draw(d)?;
    Ok(())
}

fn center<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    s: &str,
    y: i32,
    color: Rgb565,
) -> Result<(), D::Error> {
    text(d, &short(s, 30), 120, y, color, Alignment::Center)
}

fn row<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    label: &str,
    value: &str,
    y: i32,
) -> Result<(), D::Error> {
    text(d, label, 32, y, MUTED, Alignment::Left)?;
    text(d, &short(value, 21), 208, y, WHITE, Alignment::Right)
}

fn large<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    s: &str,
    y: i32,
    color: Rgb565,
) -> Result<(), D::Error> {
    Text::with_alignment(
        s,
        Point::new(120, y),
        MonoTextStyle::new(&FONT_9X18_BOLD, color),
        Alignment::Center,
    )
    .draw(d)?;
    Ok(())
}

pub fn megabits(bps: u64) -> String {
    let hundredths = u128::from(bps) / 10000;
    format!("{}.{:02} Mb/s", hundredths / 100, hundredths % 100)
}
fn optional(n: Option<f64>, suffix: &str) -> String {
    n.map(|n| format!("{n:.1}{suffix}"))
        .unwrap_or_else(|| "--".into())
}
fn count(n: Option<u64>) -> String {
    n.map(|n| format!("{n}")).unwrap_or_else(|| "--".into())
}
fn uptime(n: Option<u64>) -> String {
    n.map(|n| format!("{}d {:02}h {:02}m", n / 86400, n / 3600 % 24, n / 60 % 60))
        .unwrap_or_else(|| "--".into())
}

pub fn setup<D: DrawTarget<Color = Rgb565>>(d: &mut D) -> Result<(), D::Error> {
    d.clear(BACKGROUND)?;
    center(d, "UNIFI", 55, ACCENT)?;
    center(d, "SETUP REQUIRED", 92, WHITE)?;
    center(d, "Open the USB flasher", 119, MUTED)?;
    center(d, "on your computer.", 136, MUTED)?;
    center(d, "Enter Wi-Fi and UniFi", 162, WHITE)?;
    center(d, "settings, then install.", 179, WHITE)
}

pub fn draw<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    s: &Dashboard,
    page: usize,
    now_ms: u64,
) -> Result<(), D::Error> {
    draw_with_lock(d, s, page, now_ms, false)
}

pub fn draw_with_lock<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    s: &Dashboard,
    page: usize,
    now_ms: u64,
    locked: bool,
) -> Result<(), D::Error> {
    d.clear(BACKGROUND)?;
    let page = page % PAGE_COUNT;
    center(
        d,
        [
            "DEVICE UPLINK",
            "DEVICE HEALTH",
            "RADIOS",
            "SITE OVERVIEW",
            "UNIFI DEVICE",
        ][page],
        28,
        ACCENT,
    )?;
    Line::new(Point::new(62, 36), Point::new(178, 36))
        .into_styled(PrimitiveStyle::with_stroke(MUTED, 1))
        .draw(d)?;
    match page {
        0 => {
            center(d, "RECEIVE (RX)", 52, MUTED)?;
            large(
                d,
                &s.rates
                    .map(|r| megabits(r.rx))
                    .unwrap_or_else(|| "-- Mb/s".into()),
                73,
                DOWN,
            )?;
            center(d, "TRANSMIT (TX)", 92, MUTED)?;
            large(
                d,
                &s.rates
                    .map(|r| megabits(r.tx))
                    .unwrap_or_else(|| "-- Mb/s".into()),
                113,
                UP,
            )?;
            let scale = s
                .history
                .iter()
                .flatten()
                .map(|r| r.rx.max(r.tx))
                .max()
                .unwrap_or(0)
                .max(1000000);
            let height = |n: u64| (u128::from(n) * 42 / u128::from(scale)) as i32;
            for i in 1..60 {
                if let (Some(a), Some(b)) = (s.history[i - 1], s.history[i]) {
                    let x = 32 + i as i32 * 3;
                    for (a, b, color) in [(a.rx, b.rx, DOWN), (a.tx, b.tx, UP)] {
                        Line::new(
                            Point::new(x - 3, 170 - height(a)),
                            Point::new(x, 170 - height(b)),
                        )
                        .into_styled(PrimitiveStyle::with_stroke(color, 1))
                        .draw(d)?;
                    }
                }
            }
            Line::new(Point::new(32, 171), Point::new(209, 171))
                .into_styled(PrimitiveStyle::with_stroke(MUTED, 1))
                .draw(d)?;
            center(d, "LAST 60 POLLS", 185, MUTED)?;
        }
        1 => {
            center(d, "CPU UTILIZATION", 58, MUTED)?;
            large(d, &optional(s.system.cpu, "%"), 81, DOWN)?;
            row(d, "MEMORY", &optional(s.system.memory, "%"), 110)?;
            if let Some(pct) = s.system.memory {
                Rectangle::new(Point::new(36, 119), Size::new(168, 5))
                    .into_styled(PrimitiveStyle::with_fill(MUTED))
                    .draw(d)?;
                Rectangle::new(
                    Point::new(36, 119),
                    Size::new((pct * 168.0 / 100.0) as u32, 5),
                )
                .into_styled(PrimitiveStyle::with_fill(DOWN))
                .draw(d)?;
            }
            center(d, "LOAD  1 / 5 / 15 MIN", 151, MUTED)?;
            center(
                d,
                &format!(
                    "{} / {} / {}",
                    optional(s.system.load[0], ""),
                    optional(s.system.load[1], ""),
                    optional(s.system.load[2], "")
                ),
                170,
                WHITE,
            )?;
            center(d, &uptime(s.system.uptime), 187, MUTED)?;
        }
        2 => {
            if s.radios.is_empty() {
                center(d, "No radio metrics", 94, WHITE)?;
                center(d, "This device may not be an AP", 120, MUTED)?;
            } else {
                for (i, r) in s.radios.iter().take(3).enumerate() {
                    let y = 64 + i as i32 * 43;
                    row(d, "BAND", &optional(r.frequency, " GHz"), y)?;
                    row(d, "TX RETRIES", &optional(r.retries, "%"), y + 18)?;
                }
            }
            center(d, "CONTROLLER RADIO SNAPSHOT", 185, MUTED)?;
        }
        3 => {
            center(d, "CONNECTED CLIENTS", 59, MUTED)?;
            large(d, &count(s.clients), 83, DOWN)?;
            row(d, "ADOPTED DEVICES", &count(s.devices), 113)?;
            row(
                d,
                "SELECTED DEVICE",
                s.device.state.as_deref().unwrap_or("--"),
                141,
            )?;
            center(d, "LATEST API SNAPSHOT", 177, MUTED)?;
        }
        _ => {
            center(
                d,
                s.device.name.as_deref().unwrap_or("UniFi device"),
                56,
                WHITE,
            )?;
            center(
                d,
                s.device.model.as_deref().unwrap_or("Unknown model"),
                77,
                MUTED,
            )?;
            row(
                d,
                "FIRMWARE",
                s.device.firmware.as_deref().unwrap_or("--"),
                108,
            )?;
            row(d, "STATE", s.device.state.as_deref().unwrap_or("--"), 133)?;
            center(
                d,
                s.device.ip.as_deref().unwrap_or("No device address"),
                158,
                WHITE,
            )?;
            center(d, &uptime(s.system.uptime), 183, MUTED)?;
        }
    }
    let status = if let Some(error) = s.error {
        error.label()
    } else if s.last_sample_ms.is_none() {
        "CONNECTING..."
    } else if s.stale(now_ms) {
        "STALE CONTROLLER DATA"
    } else if s
        .device
        .state
        .as_deref()
        .is_some_and(|state| state != "ONLINE")
    {
        "DEVICE NOT ONLINE"
    } else {
        "LIVE"
    };
    let color = if status == "LIVE" { DOWN } else { UP };
    center(d, &short(status, 28), 202, color)?;
    for i in 0..PAGE_COUNT {
        Circle::new(Point::new(91 + i as i32 * 13, 215), 5)
            .into_styled(PrimitiveStyle::with_fill(if i == page {
                ACCENT
            } else {
                MUTED
            }))
            .draw(d)?;
    }
    let color = if locked { UP } else { MUTED };
    let style = PrimitiveStyle::with_stroke(color, 1);
    // A closed gold shackle indicates the page is locked; grey/open is unlocked.
    let shackle_x = if locked { 167 } else { 173 };
    for (a, b) in [
        (Point::new(shackle_x, 214), Point::new(shackle_x, 210)),
        (Point::new(shackle_x, 210), Point::new(shackle_x + 6, 210)),
        (
            Point::new(shackle_x + 6, 210),
            Point::new(shackle_x + 6, 214),
        ),
    ] {
        Line::new(a, b).into_styled(style).draw(d)?;
    }
    Rectangle::new(Point::new(165, 214), Size::new(11, 8))
        .into_styled(PrimitiveStyle::with_stroke(color, 1))
        .draw(d)?;
    Line::new(Point::new(170, 217), Point::new(170, 219))
        .into_styled(style)
        .draw(d)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::mock_display::MockDisplay;
    #[test]
    fn preserves_api_bit_rate_units_and_handles_all_pages_without_data() {
        assert_eq!(megabits(1000000), "1.00 Mb/s");
        assert_eq!(megabits(24860000), "24.86 Mb/s");
        assert!(megabits(u64::MAX).ends_with("Mb/s"));
        let mut d = MockDisplay::new();
        d.set_allow_out_of_bounds_drawing(true);
        d.set_allow_overdraw(true);
        let mut s = Dashboard::default();
        s.device.name = Some("AP\n日本語".into());
        for page in 0..PAGE_COUNT {
            draw(&mut d, &s, page, 0).unwrap();
        }
        setup(&mut d).unwrap();
    }
}
