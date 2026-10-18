//! Board LAN transport; every exchange authenticates the enrolled certificate.
use alloc::vec::Vec;
use embassy_net::{tcp::TcpSocket, Stack};
use embassy_time::{with_timeout, Duration};
use embedded_tls::{Aes128GcmSha256, TlsConfig, TlsConnection, TlsContext};
use unifi_display::provision::{Http, HttpRequest, Response, Result, Target};

pub struct Lan {
    pub stack: Stack<'static>,
    pub owner: u32,
    rx: Vec<u8>,
    tx: Vec<u8>,
    read: Vec<u8>,
    write: Vec<u8>,
}
impl Lan {
    pub fn new(stack: Stack<'static>, owner: u32) -> Self {
        Self {
            stack,
            owner,
            rx: alloc::vec![0;4096],
            tx: alloc::vec![0;2048],
            read: alloc::vec![0;16640],
            write: alloc::vec![0;4096],
        }
    }
    pub async fn inspect(&mut self, target: &Target, now: u64) -> Result<[u8; 32]> {
        let observed = core::cell::Cell::new(None);
        let mut socket = TcpSocket::new(self.stack, &mut self.rx, &mut self.tx);
        socket.set_timeout(Some(Duration::from_secs(8)));
        let address = embassy_net::Ipv4Address::from(target.address.octets());
        socket
            .connect((address, 443))
            .await
            .map_err(|_| "Cannot reach the console from this Wi-Fi")?;
        let mut tls =
            TlsConnection::<_, Aes128GcmSha256>::new(socket, &mut self.read, &mut self.write);
        let config = TlsConfig::new().with_server_name(&target.hostname);
        let provider = unifi_display::tls::BootstrapProvider::new(
            crate::BoardRng(esp_hal::rng::Rng::new()),
            &target.hostname,
            now,
            webpki_roots::TLS_SERVER_ROOTS,
            &observed,
        );
        with_timeout(Duration::from_secs(25),tls.open(TlsContext::new(&config,provider))).await
            .map_err(|_| "Console certificate verification timed out")?
            .map_err(|_| "Console certificate could not be verified; check the browser clock and console direct-connect certificate")?;
        observed
            .get()
            .ok_or("Console certificate was not authenticated")
    }
}
impl Http for Lan {
    async fn exchange(&mut self, request: HttpRequest<'_>) -> Result<Response> {
        if !crate::usb::accepts(self.owner) {
            return Err("USB session ended; reconnect before signing in");
        }
        let mut socket = TcpSocket::new(self.stack, &mut self.rx, &mut self.tx);
        socket.set_timeout(Some(Duration::from_secs(8)));
        let address = embassy_net::Ipv4Address::from(request.target.address.octets());
        socket
            .connect((address, 443))
            .await
            .map_err(|_| "Cannot reach the local console")?;
        let mut tls =
            TlsConnection::<_, Aes128GcmSha256>::new(socket, &mut self.read, &mut self.write);
        let config = TlsConfig::new().with_server_name(&request.target.hostname);
        let provider = unifi_display::tls::Provider::new(
            crate::BoardRng(esp_hal::rng::Rng::new()),
            request.pin,
        );
        with_timeout(Duration::from_secs(20), async {
            tls.open(TlsContext::new(&config, provider))
                .await
                .map_err(|_| "Console certificate changed; discover it again before signing in")?;
            if !crate::usb::accepts(self.owner) {
                return Err("USB session ended; no console credentials were sent");
            }
            unifi_display::console_http::exchange(&mut tls, request).await
        })
        .await
        .map_err(|_| "Local console request timed out")?
    }
}
