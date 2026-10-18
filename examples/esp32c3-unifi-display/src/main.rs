//! Bare-metal Wi-Fi / UniFi dashboard for the round razem-tag gadget.
#![no_std]
#![no_main]

extern crate alloc;
mod console_net;
mod usb;

use embassy_executor::Spawner;
use embassy_net::{Runner, Stack, StackResources};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{with_timeout, Duration, Instant, Timer};
use embedded_graphics::{pixelcolor::Rgb565, prelude::RgbColor};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    i2c::master::{Config as I2cConfig, I2c},
    interrupt::software::SoftwareInterruptControl,
    rng::Rng,
    spi::master::{Config as SpiConfig, Spi},
    time::Rate,
    timer::timg::TimerGroup,
};
use esp_radio::wifi::Interface;
use log::warn;
use static_cell::StaticCell;
use unifi_display::{
    api::{self, Client, Error},
    config::{self, DeviceConfig},
    navigation::Navigation,
    panel, render,
    stats::Dashboard,
};

type Shared = Mutex<CriticalSectionRawMutex, Dashboard>;
static STATE: StaticCell<Shared> = StaticCell::new();
static NET: StaticCell<StackResources<4>> = StaticCell::new();
static PIXELS: StaticCell<[Rgb565; panel::WIDTH * panel::TILE_ROWS]> = StaticCell::new();
static WIFI: StaticCell<usb::Wifi> = StaticCell::new();
static USB_TX: StaticCell<usb::Tx> = StaticCell::new();
static CONFIG: StaticCell<DeviceConfig> = StaticCell::new();

esp_bootloader_esp_idf::esp_app_desc!();

static HTTP_HEADERS: StaticCell<[u8; 4096]> = StaticCell::new();
struct Connector {
    stack: Stack<'static>,
    host: &'static str,
    port: u16,
    pin: [u8; 32],
    rx: alloc::vec::Vec<u8>,
    tx: alloc::vec::Vec<u8>,
    tls_read: alloc::vec::Vec<u8>,
    tls_write: alloc::vec::Vec<u8>,
}
struct BoardRng(Rng);
impl rand_core::RngCore for BoardRng {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0; 4];
        self.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, bytes: &mut [u8]) {
        self.0.read(bytes)
    }
    fn try_fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(bytes);
        Ok(())
    }
}
impl rand_core::CryptoRng for BoardRng {}
impl unifi::embedded_async::Connector for Connector {
    type Error = Error;
    type Connection<'a> = embedded_tls::TlsConnection<
        'a,
        embassy_net::tcp::TcpSocket<'a>,
        embedded_tls::Aes128GcmSha256,
    >;
    async fn connect(&mut self) -> Result<Self::Connection<'_>, Error> {
        use embassy_net::{dns::DnsQueryType, tcp::TcpSocket};
        use embedded_tls::{Aes128GcmSha256, TlsConfig, TlsConnection, TlsContext};
        let address = self
            .stack
            .dns_query(self.host, DnsQueryType::A)
            .await
            .map_err(|_| Error::Transport)?
            .first()
            .copied()
            .ok_or(Error::Transport)?;
        // Enforce LAN-only API traffic even when a configured DNS name changes.
        let embassy_net::IpAddress::Ipv4(ip) = address;
        if !config::local_ipv4(ip) {
            return Err(Error::LocalAddress);
        }
        let mut socket = TcpSocket::new(self.stack, &mut self.rx, &mut self.tx);
        socket.set_timeout(Some(Duration::from_secs(8)));
        socket
            .connect((address, self.port))
            .await
            .map_err(|_| Error::Transport)?;
        let mut tls = TlsConnection::<_, Aes128GcmSha256>::new(
            socket,
            &mut self.tls_read,
            &mut self.tls_write,
        );
        let tls_config = TlsConfig::new().with_server_name(self.host);
        let provider = unifi_display::tls::Provider::new(BoardRng(Rng::new()), self.pin);
        tls.open(TlsContext::new(&tls_config, provider))
            .await
            .map_err(|_| Error::Tls)?;
        Ok(tls)
    }
}
struct Http {
    client: unifi::embedded_async::Client<'static, Connector>,
}
impl Http {
    fn new(stack: Stack<'static>, config: &'static DeviceConfig) -> Self {
        let endpoint = config::Endpoint::parse(&config.controller_url).unwrap();
        let connector = Connector {
            stack,
            host: endpoint.host,
            port: endpoint.port,
            pin: config.fingerprint(),
            rx: alloc::vec![0;4096],
            tx: alloc::vec![0;2048],
            tls_read: alloc::vec![0;16640],
            tls_write: alloc::vec![0;4096],
        };
        let core = unifi::Client::new(endpoint.authority, endpoint.base, &config.api_key).unwrap();
        Self {
            client: unifi::embedded_async::Client::new(
                core,
                connector,
                HTTP_HEADERS.init([0; 4096]),
            ),
        }
    }
    async fn exchange(&mut self, call: &unifi::Call, output: &mut [u8]) -> Result<usize, Error> {
        let response =
            self.client
                .execute(call, None, output)
                .await
                .map_err(|error| match error {
                    unifi::embedded_async::Error::Connect(error) => error,
                    unifi::embedded_async::Error::Core(unifi::Error::Transport(
                        reqwless::Error::BufferTooSmall,
                    )) => Error::TooLarge,
                    _ => Error::Transport,
                })?;
        if response.status == 429 {
            return Err(Error::RateLimited(
                response
                    .retry_after
                    .as_ref()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(60)
                    .clamp(5, 3600),
            ));
        }
        if !(200..300).contains(&response.status) {
            return Err(Error::Http(response.status));
        }
        Ok(response.body.len())
    }
}
impl api::Http for Http {
    async fn get(&mut self, call: &unifi::Call, response: &mut [u8]) -> Result<usize, Error> {
        with_timeout(Duration::from_secs(8), self.exchange(call, response))
            .await
            .map_err(|_| Error::Timeout)?
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // USB carries framed setup commands; keep ordinary logging off this transport.
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    esp_alloc::heap_allocator!(size: 160 * 1024);
    let timers = TimerGroup::new(p.TIMG0);
    esp_rtos::start(
        timers.timer0,
        SoftwareInterruptControl::new(p.SW_INTERRUPT).software_interrupt0,
    );

    // Exact razem-tag board pins. GPIO10 is also touch INT; poll touch instead.
    let _backlight = Output::new(p.GPIO3, Level::High, OutputConfig::default());
    let _cs = Output::new(p.GPIO10, Level::Low, OutputConfig::default());
    let dc = Output::new(p.GPIO2, Level::Low, OutputConfig::default());
    let spi = Spi::new(
        p.SPI2,
        SpiConfig::default().with_frequency(Rate::from_mhz(40)),
    )
    .unwrap()
    .with_sck(p.GPIO6)
    .with_mosi(p.GPIO7);
    let mut panel = panel::Panel::new(spi, dc);
    let mut delay = Delay::new();
    panel.init(&mut delay).unwrap();

    let mut touch = I2c::new(
        p.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .unwrap()
    .with_sda(p.GPIO4)
    .with_scl(p.GPIO5);
    let mut touch_reset = Output::new(p.GPIO1, Level::Low, OutputConfig::default());
    delay.delay_millis(20);
    touch_reset.set_high();
    delay.delay_millis(400);
    let _ = touch.write(0x15, &[0xFE, 1]); // Keep scanning while idle.

    let pixels = PIXELS.init([Rgb565::BLACK; panel::WIDTH * panel::TILE_ROWS]);
    let mut store = usb::Store::new(p.FLASH);
    let config = unifi_display::settings::load(&mut store).map(|c| &*CONFIG.init(c));
    let (usb_rx, usb_tx) = esp_hal::usb_serial_jtag::UsbSerialJtag::new(p.USB_DEVICE)
        .into_async()
        .split();
    let usb_tx = USB_TX.init(Mutex::new(usb_tx));
    let state = STATE.init(Mutex::new(Dashboard::default()));
    let (controller, interfaces) = esp_radio::wifi::new(p.WIFI, Default::default()).unwrap();
    let mut seed = [0u8; 8];
    Rng::new().read(&mut seed);
    let (stack, runner) = embassy_net::new(
        interfaces.station,
        embassy_net::Config::dhcpv4(Default::default()),
        NET.init(StackResources::new()),
        u64::from_le_bytes(seed),
    );
    let mut controller = controller;
    let baseline = match config {
        Some(c) => usb::wifi_config(&c.wifi_ssid, &c.wifi_password),
        None => usb::wifi_config("", ""),
    };
    controller.set_config(&baseline).unwrap();
    let wifi = WIFI.init(Mutex::new(controller));
    spawner.spawn(wifi_connection(wifi, config).unwrap());
    spawner.spawn(usb::receive(usb_rx, usb_tx).unwrap());
    spawner.spawn(usb::work(usb_tx, wifi, stack, config, store).unwrap());
    spawner.spawn(network(runner).unwrap());
    if let Some(config) = config {
        spawner.spawn(poll_router(stack, state, config).unwrap());
    }

    let mut navigation = Navigation::new(Instant::now().as_millis());
    let mut frame_at = Instant::now();
    let mut was_app = false;
    loop {
        let now = Instant::now();
        let mut fingers = [0];
        let touching = touch
            .write_read(0x15, &[0x02], &mut fingers)
            .ok()
            .map(|_| fingers[0] & 0x0F != 0);
        let app_mode = usb::active();
        if was_app && !app_mode {
            navigation.resume(now.as_millis());
        }
        was_app = app_mode;
        let changed = !app_mode && navigation.update(now.as_millis(), touching);
        if changed || now >= frame_at {
            let snapshot = state.lock().await.clone();
            for y in (0..240).step_by(panel::TILE_ROWS) {
                let mut tile = panel::Tile { y, pixels };
                if app_mode {
                    render::app_mode(&mut tile, usb::label()).unwrap();
                } else if config.is_none() {
                    render::setup(&mut tile).unwrap();
                } else {
                    render::draw_with_lock(
                        &mut tile,
                        &snapshot,
                        navigation.page(),
                        now.as_millis(),
                        navigation.locked(),
                    )
                    .unwrap();
                }
                panel.write_tile(y, pixels).unwrap();
                embassy_futures::yield_now().await;
            }
            frame_at = Instant::now() + Duration::from_millis(250);
        }
        Timer::after(Duration::from_millis(20)).await;
    }
}

#[embassy_executor::task]
async fn wifi_connection(wifi: &'static usb::Wifi, config: Option<&'static DeviceConfig>) -> ! {
    loop {
        if config.is_some() && !usb::active() {
            let mut controller = wifi.lock().await;
            if !usb::active() && !controller.is_connected() {
                let _ = with_timeout(Duration::from_secs(20), controller.connect_async()).await;
            }
        }
        Timer::after(Duration::from_secs(1)).await;
    }
}

#[embassy_executor::task]
async fn network(mut runner: Runner<'static, Interface<'static>>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn poll_router(
    stack: Stack<'static>,
    shared: &'static Shared,
    config: &'static DeviceConfig,
) -> ! {
    let mut client = Client::new(Http::new(stack, config), &config.site_id, &config.device_id);
    let mut state = Dashboard::default();
    loop {
        if usb::active() {
            Timer::after_millis(250).await;
            continue;
        }
        if !stack.is_config_up() && state.last_sample_ms.is_some() {
            state.error = Some(Error::Transport);
            state.push_rate(None);
            *shared.lock().await = state.clone();
        }
        stack.wait_config_up().await;
        if usb::active() {
            continue;
        }
        let started = Instant::now();
        let result = client
            .sample(&mut state, || Instant::now().as_millis())
            .await;
        let delay = match result {
            Ok(()) => {
                state.error = None;
                Duration::from_secs(5)
            }
            Err(error) => {
                warn!("Controller sample failed: {error:?}");
                state.error = Some(error);
                state.push_rate(None);
                if let Error::RateLimited(seconds) = error {
                    Duration::from_secs(seconds)
                } else {
                    Duration::from_secs(5)
                }
            }
        };
        *shared.lock().await = state.clone();
        if matches!(result, Err(Error::RateLimited(_))) {
            Timer::after(delay).await;
        } else {
            Timer::at(started + delay).await;
        }
    }
}
