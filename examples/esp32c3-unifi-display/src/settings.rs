//! Two-sector settings journal. Keep the previous verified sector until replacement succeeds.
use crate::config::{self, DeviceConfig, SECTOR_SIZE};

pub trait Flash {
    type Error;
    fn read(&mut self, slot: u32, bytes: &mut [u8; SECTOR_SIZE]) -> Result<(), Self::Error>;
    fn write(&mut self, slot: u32, bytes: &[u8; SECTOR_SIZE]) -> Result<(), Self::Error>;
}
pub fn load(flash: &mut impl Flash) -> Option<DeviceConfig> {
    let mut bytes = [0; SECTOR_SIZE];
    for slot in 0..2 {
        if flash.read(slot, &mut bytes).is_ok() {
            if let Ok(config) = config::decode_sector(&bytes) {
                return Some(config);
            }
        }
    }
    None
}
pub fn save(flash: &mut impl Flash, config: &DeviceConfig) -> Result<(), &'static str> {
    let next = config::encode_sector(config).map_err(|_| "Invalid settings")?;
    let mut previous = [0; SECTOR_SIZE];
    flash
        .read(0, &mut previous)
        .map_err(|_| "Cannot read settings")?;
    let mut check = [0; SECTOR_SIZE];
    if config::decode_sector(&previous).is_ok() {
        flash
            .write(1, &previous)
            .map_err(|_| "Cannot back up settings")?;
        flash
            .read(1, &mut check)
            .map_err(|_| "Cannot verify backup")?;
        if check != previous {
            return Err("Backup verification failed");
        }
    }
    flash.write(0, &next).map_err(|_| "Cannot save settings")?;
    flash
        .read(0, &mut check)
        .map_err(|_| "Cannot verify settings")?;
    if check != next {
        return Err("Settings verification failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Memory {
        slots: [[u8; SECTOR_SIZE]; 2],
        fail: Option<u32>,
    }
    impl Flash for Memory {
        type Error = ();
        fn read(&mut self, slot: u32, bytes: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
            *bytes = self.slots[slot as usize];
            Ok(())
        }
        fn write(&mut self, slot: u32, bytes: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
            if self.fail == Some(slot) {
                self.slots[slot as usize] = [0; SECTOR_SIZE];
                return Err(());
            }
            self.slots[slot as usize] = *bytes;
            Ok(())
        }
    }
    fn config(key: &str) -> DeviceConfig {
        DeviceConfig {
            wifi_ssid: "fixture".into(),
            wifi_password: "password".into(),
            controller_url: "https://192.168.1.1/integration".into(),
            api_key: key.into(),
            certificate_sha256: "ab".repeat(32),
            site_id: "".into(),
            device_id: "".into(),
        }
    }
    #[test]
    fn interrupted_save_keeps_previous_credentials() {
        let mut flash = Memory {
            slots: [[0xff; SECTOR_SIZE]; 2],
            fail: None,
        };
        assert!(load(&mut flash).is_none());
        save(&mut flash, &config("old")).unwrap();
        for fail in [1, 0] {
            flash.fail = Some(fail);
            assert!(save(&mut flash, &config("new")).is_err());
            assert_eq!(load(&mut flash).unwrap().api_key, "old");
        }
        flash.fail = None;
        save(&mut flash, &config("new")).unwrap();
        assert_eq!(load(&mut flash).unwrap().api_key, "new");
    }
}
