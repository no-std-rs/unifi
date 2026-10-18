//! Network progress retained while the USB installer temporarily owns the radio.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetworkState {
    #[default]
    Connecting,
    AuthenticationFailed,
    NetworkNotFound,
    SecurityMismatch,
    ConnectionFailed,
    ConnectionTimedOut,
    WaitingForAddress,
    AddressTimedOut,
    Ready,
}

impl NetworkState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Connecting => "CONNECTING TO WI-FI",
            Self::AuthenticationFailed => "WI-FI PASSWORD / AUTH FAILED",
            Self::NetworkNotFound => "WI-FI NETWORK NOT FOUND",
            Self::SecurityMismatch => "WI-FI SECURITY NOT SUPPORTED",
            Self::ConnectionFailed => "WI-FI FAILED - RETRYING",
            Self::ConnectionTimedOut => "WI-FI TIMED OUT - RETRYING",
            Self::WaitingForAddress => "WAITING FOR DHCP ADDRESS",
            Self::AddressTimedOut => "NO DHCP ADDRESS - RETRYING",
            Self::Ready => "WI-FI CONNECTED",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Connecting => "connecting_wifi",
            Self::AuthenticationFailed => "wifi_authentication_failed",
            Self::NetworkNotFound => "wifi_network_not_found",
            Self::SecurityMismatch => "wifi_security_mismatch",
            Self::ConnectionFailed => "wifi_connection_failed",
            Self::ConnectionTimedOut => "wifi_connection_timed_out",
            Self::WaitingForAddress => "waiting_for_dhcp",
            Self::AddressTimedOut => "dhcp_timed_out",
            Self::Ready => "wifi_connected",
        }
    }
}

/// Tracks DHCP without registering a second waiter on Embassy's state waker.
#[derive(Default)]
pub struct AddressWait {
    since_ms: Option<u64>,
}

impl AddressWait {
    pub fn observe(
        &mut self,
        associated: bool,
        configured: bool,
        now_ms: u64,
    ) -> Option<NetworkState> {
        if !associated {
            self.since_ms = None;
            return None;
        }
        if configured {
            self.since_ms = None;
            return Some(NetworkState::Ready);
        }
        let since = *self.since_ms.get_or_insert(now_ms);
        Some(if now_ms.saturating_sub(since) >= 15_000 {
            NetworkState::AddressTimedOut
        } else {
            NetworkState::WaitingForAddress
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dhcp_deadline_survives_polls_and_recovers_when_a_lease_arrives() {
        let mut wait = AddressWait::default();
        assert_eq!(
            wait.observe(true, false, 1000),
            Some(NetworkState::WaitingForAddress)
        );
        assert_eq!(
            wait.observe(true, false, 15999),
            Some(NetworkState::WaitingForAddress)
        );
        assert_eq!(
            wait.observe(true, false, 16000),
            Some(NetworkState::AddressTimedOut)
        );
        assert_eq!(
            wait.observe(true, false, 30000),
            Some(NetworkState::AddressTimedOut)
        );
        assert_eq!(wait.observe(true, true, 31000), Some(NetworkState::Ready));
        assert_eq!(
            wait.observe(true, false, 32000),
            Some(NetworkState::WaitingForAddress)
        );
    }

    #[test]
    fn disconnection_does_not_overwrite_radio_failure_and_restarts_dhcp_deadline() {
        let mut wait = AddressWait::default();
        wait.observe(true, false, 0);
        assert_eq!(wait.observe(false, false, 16000), None);
        assert_eq!(
            wait.observe(true, false, 20000),
            Some(NetworkState::WaitingForAddress)
        );
        assert_eq!(
            wait.observe(true, false, 34999),
            Some(NetworkState::WaitingForAddress)
        );
        assert_eq!(
            wait.observe(true, false, 35000),
            Some(NetworkState::AddressTimedOut)
        );
    }
}
