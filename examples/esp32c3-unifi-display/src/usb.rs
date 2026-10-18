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
pub(crate) fn accepts(id: u32) -> bool {
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
                    reply(tx,request.id,id,Ok(json!({"nonce":nonce,"board":"ESP32-2424S012","protocol":1,"capabilities":["scan_wifi","test_wifi","save_config","console_provision"]}))).await;
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

enum RadioError {
    Operation(&'static str),
    Restore,
}

async fn restore_radio(
    wifi: &'static Wifi,
    stack: Stack<'static>,
    saved: Option<&DeviceConfig>,
) -> Result<(), ()> {
    let mut controller = with_timeout(Duration::from_secs(20), wifi.lock())
        .await
        .map_err(|_| ())?;
    disconnect(&mut controller, stack).await.map_err(|_| ())?;
    let config = saved
        .map(|c| wifi_config(&c.wifi_ssid, &c.wifi_password))
        .unwrap_or_else(|| wifi_config("", ""));
    controller.set_config(&config).map_err(|_| ())
}

async fn join_console_wifi(
    wifi: &'static Wifi,
    stack: Stack<'static>,
    ssid: &str,
    password: &str,
) -> Result<(), &'static str> {
    config::validate_wifi(ssid, password).map_err(|_| "Invalid Wi-Fi name or password format")?;
    let mut controller = with_timeout(Duration::from_secs(20), wifi.lock())
        .await
        .map_err(|_| "Wi-Fi is busy; try again")?;
    disconnect(&mut controller, stack).await?;
    controller
        .set_config(&wifi_config(ssid, password))
        .map_err(|_| "Cannot configure Wi-Fi")?;
    with_timeout(Duration::from_secs(20), controller.connect_async())
        .await
        .map_err(|_| "Wi-Fi association timed out")?
        .map_err(|_| "Wi-Fi association failed; check the password")?;
    with_timeout(Duration::from_secs(15), stack.wait_config_up())
        .await
        .map_err(|_| "Wi-Fi connected but no DHCP address")?;
    Ok(())
}
impl From<&'static str> for RadioError {
    fn from(message: &'static str) -> Self {
        Self::Operation(message)
    }
}

async fn radio_command(
    command: Command,
    wifi: &'static Wifi,
    stack: Stack<'static>,
    saved: Option<&'static DeviceConfig>,
) -> Result<Value, RadioError> {
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
            if cleanup.is_err() || restored.is_err() {
                return Err(RadioError::Restore);
            }
            result.map_err(RadioError::Operation)
        }
        _ => Err(RadioError::Operation("Unsupported radio command")),
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
    let mut console: Option<unifi_display::provision::Session> = None;
    loop {
        // A browser close, heartbeat expiry or replacement session drops all
        // temporary sign-in state and restores the saved network without a write.
        if console.as_ref().is_some_and(|c| !accepts(c.owner)) {
            console = None;
            if restore_radio(wifi, stack, saved).await.is_err() {
                esp_hal::system::software_reset();
            }
        }
        let Ok(request) = with_timeout(Duration::from_millis(250), COMMANDS.receive()).await else {
            continue;
        };
        if !accepts(request.session) {
            continue;
        }
        if console.is_some()
            && matches!(
                &request.command,
                Command::ConsoleBegin { .. }
                    | Command::ScanWifi {}
                    | Command::TestWifi { .. }
                    | Command::ConsoleEnd {}
            )
        {
            console = None;
            if restore_radio(wifi, stack, saved).await.is_err() {
                esp_hal::system::software_reset();
            }
        }
        let mut restart = false;
        let value = match request.command {
            Command::ConsoleBegin {
                ssid,
                password,
                address,
                hostname,
                now,
            } => {
                set_label("Verifying local console...");
                let result = async {
                    let target = unifi_display::provision::Target::new(&address,&hostname)?;
                    if !(1_700_000_000..=4_102_444_800).contains(&now) { return Err("Check your browser's date and time"); }
                    join_console_wifi(wifi,stack,&ssid,&password).await?;
                    let mut lan = crate::console_net::Lan::new(stack,request.session);
                    let pin = lan.inspect(&target,now).await?;
                    if !accepts(request.session) { return Err("USB session ended while verifying console"); }
                    let session = unifi_display::provision::Session::new(request.session,target,pin);
                    let result = json!({"certificate_sha256":session.fingerprint(),"hostname":session.target.hostname});
                    console = Some(session);
                    Ok(result)
                }.await;
                if result.is_err() && restore_radio(wifi, stack, saved).await.is_err() {
                    restart = true;
                }
                result
            }
            Command::ConsoleLogin {
                username,
                password,
                token,
            } => {
                set_label("Signing in to console...");
                match console.as_mut().filter(|c| c.owner == request.session) {
                    Some(c) => {
                        c.login(
                            &mut crate::console_net::Lan::new(stack, request.session),
                            &username,
                            &password,
                            &token,
                        )
                        .await
                    }
                    None => Err("Verify the console certificate before signing in"),
                }
            }
            Command::ConsoleMfa { id } => {
                match console.as_mut().filter(|c| c.owner == request.session) {
                    Some(c) => {
                        c.mfa(
                            &mut crate::console_net::Lan::new(stack, request.session),
                            &id,
                        )
                        .await
                    }
                    None => Err("Sign in to the console first"),
                }
            }
            Command::ConsolePoll {} => {
                match console.as_mut().filter(|c| c.owner == request.session) {
                    Some(c) => {
                        c.poll(&mut crate::console_net::Lan::new(stack, request.session))
                            .await
                    }
                    None => Err("Sign in to the console first"),
                }
            }
            Command::ConsoleProvision {
                site_id,
                device_id,
                local_key,
            } => {
                set_label("Creating local display access...");
                match console.as_mut().filter(|c| c.owner == request.session) {
                    Some(c) => {
                        c.provision(
                            &mut crate::console_net::Lan::new(stack, request.session),
                            &site_id,
                            &device_id,
                            &local_key,
                        )
                        .await
                    }
                    None => Err("Verify the console certificate first"),
                }
            }
            Command::ConsoleEnd {} => Ok(Value::Null),
            Command::GetConfig {} => Ok(json!({"config":saved})),
            Command::Status {} => Ok(
                json!({"configured":saved.is_some(),"ip":stack.config_v4().map(|c|c.address.address().to_string()),"app_mode":true}),
            ),
            Command::SaveConfig { config } => {
                console = None;
                set_label("Saving settings...");
                let result = settings::save(&mut store, &config).map(|()| json!({"restart":true}));
                restart = result.is_ok();
                result
            }
            command => match radio_command(command, wifi, stack, saved).await {
                Ok(value) => Ok(value),
                Err(RadioError::Operation(message)) => Err(message),
                Err(RadioError::Restore) => {
                    // A timed-out driver may still be associating. Reboot reloads only saved settings.
                    restart = true;
                    Err("Could not restore saved Wi-Fi; restarting display")
                }
            },
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
