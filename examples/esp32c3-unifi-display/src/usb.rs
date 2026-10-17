//! Native USB Serial/JTAG setup service. All device code remains no_std Rust.
use alloc::{format, string::ToString};
use core::cell::RefCell;
use embassy_net::Stack;
use embassy_sync::{
    blocking_mutex::{raw::CriticalSectionRawMutex, Mutex as BlockingMutex},
    channel::Channel,
    mutex::Mutex,
};
use embassy_time::{with_timeout, Duration, Instant, Timer};
use embedded_io_async::{Read, Write};
use esp_hal::{
    rng::Rng,
    usb_serial_jtag::{UsbSerialJtagRx, UsbSerialJtagTx},
    Async,
};
use esp_radio::wifi::{
    scan::ScanConfig, sta::StationConfig, AuthenticationMethod, Config, WifiController,
};
use serde_json::{json, Value};
use unifi_display::{
    app::{Command, Decoder, Reply, Request, Session},
    config::{self, DeviceConfig, SECTOR_SIZE},
    settings,
};

pub type Wifi = Mutex<CriticalSectionRawMutex, WifiController<'static>>;
pub type Tx = Mutex<CriticalSectionRawMutex, UsbSerialJtagTx<'static, Async>>;
static SESSION: BlockingMutex<CriticalSectionRawMutex, RefCell<Session>> =
    BlockingMutex::new(RefCell::new(Session::default_const()));
static LABEL: BlockingMutex<CriticalSectionRawMutex, RefCell<&'static str>> =
    BlockingMutex::new(RefCell::new("Connected to website"));
static COMMANDS: Channel<CriticalSectionRawMutex, Request, 1> = Channel::new();
fn now() -> u64 {
    Instant::now().as_millis()
}
pub fn active() -> bool {
    SESSION.lock(|s| s.borrow().active(now()))
}
fn accepts(id: u32) -> bool {
    SESSION.lock(|s| s.borrow().accepts(id, now()))
}
pub fn label() -> &'static str {
    LABEL.lock(|s| *s.borrow())
}
fn set_label(value: &'static str) {
    LABEL.lock(|s| *s.borrow_mut() = value);
}
fn close(id: u32) {
    SESSION.lock(|s| s.borrow_mut().close(id));
}

pub fn wifi_config(ssid: &str, password: &str) -> Config {
    Config::Station(
        StationConfig::default()
            .with_ssid(ssid)
            .with_auth_method(if password.is_empty() {
                AuthenticationMethod::None
            } else {
                AuthenticationMethod::Wpa2Personal
            })
            .with_password(password.into()),
    )
}

pub struct Store {
    flash: esp_storage::FlashStorage<'static>,
    valid: bool,
}
impl Store {
    pub fn new(peripheral: esp_hal::peripherals::FLASH<'static>) -> Self {
        use esp_bootloader_esp_idf::partitions::{read_partition_table, PARTITION_TABLE_MAX_LEN};
        let mut flash = esp_storage::FlashStorage::new(peripheral);
        let mut bytes = [0; PARTITION_TABLE_MAX_LEN];
        let valid = read_partition_table(&mut flash, &mut bytes)
            .ok()
            .is_some_and(|t| {
                t.iter().any(|p| {
                    p.label().starts_with(b"dashboard\0")
                        && p.raw_type() == 1
                        && p.raw_subtype() == 0x06
                        && p.offset() == config::FLASH_OFFSET
                        && p.len() >= 2 * SECTOR_SIZE as u32
                })
            });
        Self { flash, valid }
    }
}
impl settings::Flash for Store {
    type Error = ();
    fn read(&mut self, slot: u32, bytes: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
        use embedded_storage::ReadStorage;
        if !self.valid || slot > 1 {
            return Err(());
        }
        self.flash
            .read(config::FLASH_OFFSET + slot * SECTOR_SIZE as u32, bytes)
            .map_err(|_| ())
    }
    fn write(&mut self, slot: u32, bytes: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
        use embedded_storage::Storage;
        if !self.valid || slot > 1 {
            return Err(());
        }
        self.flash
            .write(config::FLASH_OFFSET + slot * SECTOR_SIZE as u32, bytes)
            .map_err(|_| ())
    }
}

async fn reply(tx: &'static Tx, id: u32, session: u32, value: Result<Value, &'static str>) {
    let (result, error) = match value {
        Ok(value) => (value, None),
        Err(error) => (Value::Null, Some(error)),
    };
    let response = Reply {
        v: 1,
        id,
        session,
        ok: error.is_none(),
        result,
        error,
    };
    let Ok(json) = serde_json::to_string(&response) else {
        close(session);
        return;
    };
    let frame = format!("\nUNIFI_APP {json}\n");
    let result = with_timeout(Duration::from_secs(2), async {
        let mut writer = tx.lock().await;
        writer.write_all(frame.as_bytes()).await?;
        writer.flush().await
    })
    .await;
    if !matches!(result, Ok(Ok(()))) {
        close(session);
    }
}

#[embassy_executor::task]
pub async fn receive(mut rx: UsbSerialJtagRx<'static, Async>, tx: &'static Tx) -> ! {
    let mut decoder = Decoder::default();
    let mut bytes = [0; 64];
    loop {
        let count = rx.read(&mut bytes).await.unwrap();
        for byte in &bytes[..count] {
            let Some(mut request) = decoder.push(*byte) else {
                continue;
            };
            if request.v != 1 {
                reply(
                    tx,
                    request.id,
                    request.session,
                    Err("Unsupported protocol version"),
                )
                .await;
                continue;
            }
            match &request.command {
                Command::Hello { nonce } => {
                    if nonce.len() != 32 || !nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
                        continue;
                    }
                    let mut random = [0; 4];
                    Rng::new().read(&mut random);
                    let id = u32::from_le_bytes(random).max(1);
                    SESSION.lock(|s| s.borrow_mut().open(id, now()));
                    set_label("Connected to website");
                    reply(tx,request.id,id,Ok(json!({"nonce":nonce,"board":"ESP32-2424S012","protocol":1,"capabilities":["scan_wifi","test_wifi","save_config"]}))).await;
                }
                Command::Ping {} => {
                    let valid = SESSION.lock(|s| s.borrow_mut().touch(request.session, now()));
                    reply(
                        tx,
                        request.id,
                        request.session,
                        if valid {
                            Ok(Value::Null)
                        } else {
                            Err("USB session expired; reconnect")
                        },
                    )
                    .await;
                }
                Command::Bye {} => {
                    close(request.session);
                    reply(tx, request.id, request.session, Ok(Value::Null)).await;
                }
                _ => {
                    if !accepts(request.session) {
                        reply(
                            tx,
                            request.id,
                            request.session,
                            Err("USB session expired; reconnect"),
                        )
                        .await;
                        continue;
                    }
                    // Moving credentials into the single worker keeps radio/flash operations serialized.
                    let (id, session) = (request.id, request.session);
                    request.v = 1;
                    if COMMANDS.try_send(request).is_err() {
                        reply(tx, id, session, Err("Device is busy; try again")).await;
                    }
                }
            }
        }
    }
}

async fn disconnect(
    controller: &mut WifiController<'static>,
    stack: Stack<'static>,
) -> Result<(), &'static str> {
    if controller.is_connected() {
        with_timeout(Duration::from_secs(3), controller.disconnect_async())
            .await
            .map_err(|_| "Wi-Fi disconnect timed out")?
            .map_err(|_| "Cannot disconnect Wi-Fi")?;
    }
    with_timeout(Duration::from_secs(3), stack.wait_config_down())
        .await
        .map_err(|_| "Waiting for old IP address to clear")?;
    Ok(())
}

async fn radio_command(
    command: Command,
    wifi: &'static Wifi,
    stack: Stack<'static>,
    saved: Option<&'static DeviceConfig>,
) -> Result<Value, &'static str> {
    let mut controller = wifi.lock().await;
    match command {
        Command::ScanWifi {} => {
            set_label("Scanning Wi-Fi...");
            let scan = ScanConfig::default().with_max(24).with_show_hidden(false);
            let aps = with_timeout(Duration::from_secs(12), controller.scan_async(&scan))
                .await
                .map_err(|_| "Wi-Fi scan timed out")?
                .map_err(|_| "Wi-Fi scan failed; try again")?;
            Ok(
                json!({"networks":aps.iter().map(|ap|json!({"ssid":ap.ssid.as_str(),"rssi":ap.signal_strength,"channel":ap.channel,"secure":ap.auth_method!=Some(AuthenticationMethod::None)})).collect::<alloc::vec::Vec<_>>()}),
            )
        }
        Command::TestWifi { ssid, password } => {
            config::validate_wifi(&ssid, &password)
                .map_err(|_| "Invalid Wi-Fi name or password format")?;
            set_label("Testing Wi-Fi...");
            disconnect(&mut controller, stack).await?;
            let result = async {
                controller
                    .set_config(&wifi_config(&ssid, &password))
                    .map_err(|_| "Cannot configure Wi-Fi")?;
                with_timeout(Duration::from_secs(20), controller.connect_async())
                    .await
                    .map_err(|_| "Wi-Fi association timed out")?
                    .map_err(|_| "Wi-Fi association failed; check network and password")?;
                with_timeout(Duration::from_secs(15), stack.wait_config_up())
                    .await
                    .map_err(|_| "Wi-Fi connected but no DHCP address")?;
                let ip = stack
                    .config_v4()
                    .ok_or("No DHCP address")?
                    .address
                    .address()
                    .to_string();
                Ok(json!({"ssid":ssid,"ip":ip}))
            }
            .await;
            // Always restore the saved radio configuration; never persist a temporary test.
            let cleanup = disconnect(&mut controller, stack).await;
            let restore = match saved {
                Some(c) => wifi_config(&c.wifi_ssid, &c.wifi_password),
                None => wifi_config("", ""),
            };
            let restored = controller
                .set_config(&restore)
                .map_err(|_| "Cannot restore Wi-Fi; restart display");
            cleanup?;
            restored?;
            result
        }
        _ => Err("Unsupported radio command"),
    }
}

#[embassy_executor::task]
pub async fn work(
    tx: &'static Tx,
    wifi: &'static Wifi,
    stack: Stack<'static>,
    saved: Option<&'static DeviceConfig>,
    mut store: Store,
) -> ! {
    loop {
        let request = COMMANDS.receive().await;
        if !accepts(request.session) {
            continue;
        }
        let mut restart = false;
        let value = match request.command {
            Command::GetConfig {} => Ok(json!({"config":saved})),
            Command::Status {} => Ok(
                json!({"configured":saved.is_some(),"ip":stack.config_v4().map(|c|c.address.address().to_string()),"app_mode":true}),
            ),
            Command::SaveConfig { config } => {
                set_label("Saving settings...");
                let result = settings::save(&mut store, &config).map(|()| json!({"restart":true}));
                restart = result.is_ok();
                result
            }
            command => radio_command(command, wifi, stack, saved).await,
        };
        set_label("Connected to website");
        // A disconnected/replaced session must not receive another session's secrets.
        if accepts(request.session) {
            reply(tx, request.id, request.session, value).await;
        }
        if restart {
            Timer::after_millis(250).await;
            esp_hal::system::software_reset();
        }
    }
}
