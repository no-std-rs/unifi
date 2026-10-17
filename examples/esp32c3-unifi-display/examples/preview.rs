//! Render the actual tiled dashboard to a PPM contact sheet on the host.
use core::convert::Infallible;
use embedded_graphics::{pixelcolor::Rgb565, prelude::*};
use std::{
    fs::File,
    io::{BufWriter, Write},
};
use unifi_display::{
    panel::{Tile, TILE_ROWS, WIDTH},
    render,
    stats::{Dashboard, Device, Radio, Rates, System},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let locked = std::env::args().any(|arg| arg == "--locked");
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "preview.ppm".into());
    let mut snapshot = Dashboard {
        device: Device {
            name: Some("Living room AP".into()),
            model: Some("U7 Pro".into()),
            firmware: Some("8.1.3".into()),
            ip: Some("192.0.2.42".into()),
            state: Some("ONLINE".into()),
        },
        system: System {
            cpu: Some(12.4),
            memory: Some(47.8),
            load: [Some(0.25), Some(0.17), Some(0.12)],
            uptime: Some(245820),
        },
        radios: vec![
            Radio {
                frequency: Some(2.4),
                retries: Some(0.8),
            },
            Radio {
                frequency: Some(5.0),
                retries: Some(1.2),
            },
            Radio {
                frequency: Some(6.0),
                retries: Some(0.4),
            },
        ],
        devices: Some(8),
        clients: Some(42),
        last_sample_ms: Some(1000),
        ..Default::default()
    };
    for i in 0u64..60 {
        let rx = 12000000 + (i * 7919 % 29) * 810000;
        snapshot.push_rate(Some(Rates { rx, tx: rx / 7 }));
    }

    let sheet_width = 256 * render::PAGE_COUNT;
    let mut image = vec![[7u8, 9, 14]; sheet_width * 256];
    let mut pixels = [Rgb565::BLACK; WIDTH * TILE_ROWS];
    for page in 0..render::PAGE_COUNT {
        for y in (0..240u16).step_by(TILE_ROWS) {
            let result: Result<(), Infallible> = render::draw_with_lock(
                &mut Tile {
                    y,
                    pixels: &mut pixels,
                },
                &snapshot,
                page,
                1000,
                locked,
            );
            result.unwrap();
            for (i, pixel) in pixels.iter().enumerate() {
                let (x, row) = (i % WIDTH, y as usize + i / WIDTH);
                if (x as i32 - 120).pow(2) + (row as i32 - 120).pow(2) >= 120 * 120 {
                    continue;
                }
                image[(row + 8) * sheet_width + page * 256 + x + 8] = [
                    (u16::from(pixel.r()) * 255 / 31) as u8,
                    (u16::from(pixel.g()) * 255 / 63) as u8,
                    (u16::from(pixel.b()) * 255 / 31) as u8,
                ];
            }
        }
    }
    let mut file = BufWriter::new(File::create(&output)?);
    write!(file, "P6\n{sheet_width} 256\n255\n")?;
    for pixel in image {
        file.write_all(&pixel)?;
    }
    file.flush()?;
    println!("Wrote {output} (illustrative data)");
    Ok(())
}
