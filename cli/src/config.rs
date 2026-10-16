//! Bounded JSON profiles. Reads are side-effect free; updates use mode 0600 and atomic rename.
use crate::{Options, Output, fill_random, read_file, setting, write_fd};
use core::fmt::{self, Write};
use heapless::{String, Vec};
use rustix::fs::{self, Mode, OFlags};
use unifi::Json;

pub struct File {
    path: String<4096>,
    bytes: [u8; 65536],
    used: usize,
}
impl File {
    pub fn load(environment: &str, explicit: Option<&str>) -> Result<Self, &'static str> {
        let mut path = String::new();
        if let Some(value) = explicit.or_else(|| setting(environment, "UNIFI_CONFIG")) {
            path.push_str(value).map_err(|_| "config path too long")?;
        } else if let Some(value) =
            setting(environment, "XDG_CONFIG_HOME").filter(|s| !s.is_empty())
        {
            write!(path, "{value}/unifi/config.json").map_err(|_| "config path too long")?;
        } else if let Some(home) = setting(environment, "HOME").filter(|s| !s.is_empty()) {
            write!(path, "{home}/.config/unifi/config.json").map_err(|_| "config path too long")?;
        }
        let mut result = Self {
            path,
            bytes: [0; 65536],
            used: 0,
        };
        match fs::stat(result.path.as_str()) {
            Err(rustix::io::Errno::NOENT) => {}
            Err(_) => return Err("cannot inspect config file"),
            Ok(_) => {
                result.used = read_file(&result.path, &mut result.bytes)?.len();
                result
                    .json()?
                    .object()
                    .map_err(|_| "config must be a JSON object")?;
            }
        }
        Ok(result)
    }
    fn json(&self) -> Result<Json<'_>, &'static str> {
        Json::parse(if self.used == 0 {
            b"{}"
        } else {
            &self.bytes[..self.used]
        })
        .map_err(|_| "invalid JSON config")
    }
    pub fn can_persist(&self) -> bool {
        !self.path.is_empty()
    }
    pub fn child_environment(&self, environment: &str) -> Result<String<8192>, &'static str> {
        let mut out = String::try_from(environment).map_err(|_| "environment exceeds capacity")?;
        if let Some(profiles) = self.json()?.get("profiles").ok().flatten()
            && let Ok(profiles) = profiles.fields()
        {
            for (_, profile) in profiles {
                for field in ["apiKeyEnv", "passwordEnv", "totpEnv"] {
                    let Some(name) = profile
                        .get(field)
                        .ok()
                        .flatten()
                        .and_then(|v| v.string().ok())
                        .and_then(|v| v.decode::<128>().ok())
                    else {
                        continue;
                    };
                    if setting(&out, &name).is_some() {
                        continue;
                    }
                    if let Some(value) = crate::environment_value(&name) {
                        write!(out, "{name}={value}\0")
                            .map_err(|_| "credential environment exceeds capacity")?;
                    }
                }
            }
        }
        Ok(out)
    }
    pub fn theme(&self) -> Result<Option<String<128>>, &'static str> {
        self.json()?
            .optional("theme")
            .map_err(|_| "invalid theme")?
            .map(|v| {
                v.string()
                    .and_then(|s| s.decode())
                    .map_err(|_| "invalid theme")
            })
            .transpose()
    }
    pub fn active(&self) -> Result<String<128>, &'static str> {
        self.json()?
            .optional("activeProfile")
            .map_err(|_| "invalid active profile")?
            .map(|v| {
                v.string()
                    .and_then(|s| s.decode())
                    .map_err(|_| "invalid active profile")
            })
            .transpose()
            .map(|v| v.unwrap_or_else(|| String::try_from("default").unwrap()))
    }
    fn profile(&self, profile: &str) -> Result<Option<Json<'_>>, &'static str> {
        let Some(profiles) = self
            .json()?
            .get("profiles")
            .map_err(|_| "invalid profiles")?
        else {
            return Ok(None);
        };
        profiles
            .object()
            .map_err(|_| "profiles must be an object")?;
        let Some(selected) = profiles
            .get(profile)
            .map_err(|_| "invalid selected profile")?
        else {
            return Ok(None);
        };
        selected
            .object()
            .map_err(|_| "selected profile must be an object")?;
        Ok(Some(selected))
    }
    pub fn has_profile(&self, profile: &str) -> Result<bool, &'static str> {
        Ok(self.profile(profile)?.is_some())
    }
    pub fn has_active_profile(&self) -> Result<bool, &'static str> {
        Ok(self
            .json()?
            .optional("activeProfile")
            .map_err(|_| "invalid active profile")?
            .is_some())
    }
    pub fn value<const N: usize>(
        &self,
        profile: &str,
        key: &str,
    ) -> Result<Option<String<N>>, &'static str> {
        let Some(selected) = self.profile(profile)? else {
            return Ok(None);
        };
        let value = selected
            .optional(key)
            .map_err(|_| "invalid profile field")?;
        value
            .map(|v| {
                v.string()
                    .and_then(|s| s.decode())
                    .map_err(|_| "profile field is not a string or exceeds capacity")
            })
            .transpose()
    }
    pub fn field(&self, profile: &str, key: &str) -> Result<Option<Json<'_>>, &'static str> {
        self.profile(profile)?
            .map(|v| v.optional(key).map_err(|_| "invalid profile field"))
            .transpose()
            .map(Option::flatten)
    }
    pub fn create_profile(&self, name: &str, value: Json<'_>) -> Result<(), &'static str> {
        if !self.can_persist() {
            return Err(
                "saving configuration requires --config, UNIFI_CONFIG, XDG_CONFIG_HOME or HOME",
            );
        }
        if name.is_empty() || name.len() > 128 {
            return Err("profile names must contain 1–128 bytes");
        }
        let fresh = Self::load("", Some(&self.path))?;
        if fresh.has_profile(name)? {
            return Err("profile already exists; use config set to update it");
        }
        let mut profile = String::<65536>::new();
        patch(
            &mut profile,
            Some(fresh.json()?),
            &["profiles", name],
            value,
        )
        .map_err(|_| "config exceeds capacity")?;
        let mut encoded = [0; 1024];
        let n = unifi::json::encode_to_slice(name, &mut encoded)
            .map_err(|_| "profile name exceeds capacity")?;
        let mut active = String::<65536>::new();
        patch(
            &mut active,
            Some(Json::parse(profile.as_bytes()).unwrap()),
            &["activeProfile"],
            Json::parse(&encoded[..n]).unwrap(),
        )
        .map_err(|_| "config exceeds capacity")?;
        store(&self.path, active.as_bytes())
    }
    fn update(&self, keys: &[&str], value: Json<'_>) -> Result<(), &'static str> {
        if !self.can_persist() {
            return Err(
                "saving configuration requires --config, UNIFI_CONFIG, XDG_CONFIG_HOME or HOME",
            );
        }
        let mut output = String::<65536>::new();
        patch(&mut output, Some(self.json()?), keys, value)
            .map_err(|_| "config exceeds capacity or key is too long")?;
        store(&self.path, output.as_bytes())
    }
    pub fn session_path(&self, profile: &str) -> Result<String<4096>, &'static str> {
        if !self.can_persist() {
            return Err("session persistence requires --cookie-file or a configuration directory");
        }
        if profile.is_empty() {
            return Err("profile name is empty");
        }
        let mut path = String::new();
        write!(
            path,
            "{}/sessions/",
            self.path.rsplit_once('/').map(|(p, _)| p).unwrap_or(".")
        )
        .map_err(|_| "session path too long")?;
        for byte in profile.bytes() {
            if byte.is_ascii_alphanumeric() || b"_-".contains(&byte) {
                path.push(byte as char)
                    .map_err(|_| "session path too long")?;
            } else {
                write!(path, "%{byte:02X}").map_err(|_| "session path too long")?;
            }
        }
        path.push_str(".json")
            .map_err(|_| "session path too long")?;
        Ok(path)
    }
    pub fn select_site(&self, profile: &str, site: &str) -> Result<(), &'static str> {
        let mut buffer = [0; 4096];
        let n = unifi::json::encode_to_slice(site, &mut buffer)
            .map_err(|_| "site selector too long")?;
        self.update(
            &["profiles", profile, "site"],
            Json::parse(&buffer[..n]).unwrap(),
        )
    }
    pub fn secret_reference(
        &self,
        profile: &str,
        field: &str,
        reference: &str,
    ) -> Result<(), &'static str> {
        let mut bytes = [0; 8192];
        let n = unifi::json::encode_to_slice(reference, &mut bytes)
            .map_err(|_| "credential reference too long")?;
        // Reload so storing two credentials preserves the first update.
        let environment = heapless::String::<1>::new();
        let fresh = Self::load(&environment, Some(&self.path))?;
        fresh.update(
            &["profiles", profile, field],
            Json::parse(&bytes[..n]).unwrap(),
        )
    }
    pub fn command(&self, args: &[&str], options: &Options<'_>) -> Result<i32, &'static str> {
        let command = args
            .get(1)
            .copied()
            .ok_or("usage: unifi config init|cloud-setup|show|set|profiles|use|theme")?;
        match command {
            "show" => {
                let mut text = String::<65536>::new();
                redact(&mut text, self.json()?).map_err(|_| "output too large")?;
                crate::output::render(
                    &mut Output(false),
                    Json::parse(text.as_bytes()).unwrap(),
                    options.format.unwrap_or(crate::output::Format::Json),
                )
                .map_err(|_| "output failed")?;
            }
            "profiles" => {
                let active = self.active()?;
                if let Some(profiles) = self
                    .json()?
                    .get("profiles")
                    .map_err(|_| "invalid profiles")?
                {
                    for (name, _) in profiles
                        .fields()
                        .map_err(|_| "profiles must be an object")?
                    {
                        let name: String<128> =
                            name.decode().map_err(|_| "profile name too long")?;
                        writeln!(
                            Output(false),
                            "{} {}",
                            if name == active { '*' } else { ' ' },
                            name
                        )
                        .map_err(|_| "output failed")?;
                    }
                }
            }
            "set" => {
                if args.len() != 4 {
                    return Err("usage: unifi config set KEY VALUE");
                }
                let mut keys = Vec::<&str, 8>::new();
                for key in args[2].split('.') {
                    keys.push(key).map_err(|_| "config key is too deep")?;
                }
                if keys.is_empty() || keys.iter().any(|s| s.is_empty()) {
                    return Err("invalid config key");
                }
                let mut encoded = [0; 8192];
                let value = if let Ok(json) = Json::parse(args[3].as_bytes()) {
                    json
                } else {
                    let n = unifi::json::encode_to_slice(args[3], &mut encoded)
                        .map_err(|_| "config value too long")?;
                    Json::parse(&encoded[..n]).unwrap()
                };
                self.update(&keys, value)?;
            }
            "use" => {
                let name = *args
                    .get(2)
                    .filter(|_| args.len() == 3)
                    .ok_or("usage: unifi config use PROFILE")?;
                if self
                    .json()?
                    .required("profiles")
                    .and_then(|p| p.required(name))
                    .is_err()
                {
                    return Err("unknown profile");
                }
                let mut encoded = [0; 1024];
                let n = unifi::json::encode_to_slice(name, &mut encoded)
                    .map_err(|_| "profile name too long")?;
                self.update(&["activeProfile"], Json::parse(&encoded[..n]).unwrap())?;
            }
            "init" | "cloud-setup" => {
                let name = options.profile.unwrap_or(if command == "cloud-setup" {
                    "cloud"
                } else {
                    "default"
                });
                if self
                    .json()?
                    .get("profiles")
                    .ok()
                    .flatten()
                    .and_then(|p| p.get(name).ok().flatten())
                    .is_some()
                {
                    return Err("profile already exists; use config set to update it");
                }
                let mut buffer = [0; 8192];
                let mut body = unifi::json::ObjectWriter::new(&mut buffer).unwrap();
                body.field(
                    "controller",
                    &options.url.unwrap_or(if command == "cloud-setup" {
                        "https://api.ui.com"
                    } else {
                        "https://unifi"
                    }),
                )
                .map_err(|_| "profile too large")?;
                if let Some(site) = options.site {
                    body.field("site", &site).map_err(|_| "profile too large")?;
                }
                if let Some(console) = options.console {
                    body.field("console", &console)
                        .map_err(|_| "profile too large")?;
                }
                for (key, value) in [
                    ("caFile", options.ca_file),
                    ("caCert", options.ca_cert),
                    ("tls", options.tls),
                ] {
                    if let Some(value) = value {
                        body.field(key, &value).map_err(|_| "profile too large")?;
                    }
                }
                if options.insecure_explicit {
                    body.field("insecure", &options.insecure).unwrap();
                }
                if let Some(timeout) = options.timeout {
                    body.field("timeout", &timeout).unwrap();
                }
                let bytes = body.finish().map_err(|_| "profile too large")?;
                self.create_profile(name, Json::parse(bytes).unwrap())?;
                writeln!(Output(false), "Profile {name} saved to {}", self.path)
                    .map_err(|_| "output failed")?;
            }
            "theme" => {
                if let Some(name) = args.get(2) {
                    if args.len() != 3 {
                        return Err("usage: unifi config theme [NAME]");
                    }
                    let mut bytes = [0; 256];
                    let n = unifi::json::encode_to_slice(*name, &mut bytes)
                        .map_err(|_| "theme name too long")?;
                    self.update(&["theme"], Json::parse(&bytes[..n]).unwrap())?;
                } else {
                    writeln!(Output(false), "plain, nord, dracula, silkcircuit-neon")
                        .map_err(|_| "output failed")?;
                }
            }
            _ => return Err("unknown config command"),
        }
        Ok(0)
    }
}
fn quote(out: &mut dyn Write, text: &str) -> fmt::Result {
    let mut bytes = [0; 4096];
    let n = unifi::json::encode_to_slice(text, &mut bytes).map_err(|_| fmt::Error)?;
    out.write_str(core::str::from_utf8(&bytes[..n]).map_err(|_| fmt::Error)?)
}
fn patch(
    out: &mut dyn Write,
    current: Option<Json<'_>>,
    keys: &[&str],
    value: Json<'_>,
) -> fmt::Result {
    if keys.is_empty() {
        return out.write_str(value.raw());
    }
    out.write_char('{')?;
    let mut first = true;
    let mut found = false;
    if let Some(current) = current {
        for (key, child) in current.fields().map_err(|_| fmt::Error)? {
            if !first {
                out.write_char(',')?;
            }
            first = false;
            let key: String<512> = key.decode().map_err(|_| fmt::Error)?;
            quote(out, &key)?;
            out.write_char(':')?;
            if key == keys[0] {
                found = true;
                patch(out, Some(child), &keys[1..], value)?;
            } else {
                out.write_str(child.raw())?;
            }
        }
    }
    if !found {
        if !first {
            out.write_char(',')?;
        }
        quote(out, keys[0])?;
        out.write_char(':')?;
        patch(out, None, &keys[1..], value)?;
    }
    out.write_char('}')
}
fn redact(out: &mut dyn Write, value: Json<'_>) -> fmt::Result {
    if let Ok(fields) = value.fields() {
        out.write_char('{')?;
        let mut first = true;
        for (key, child) in fields {
            if !first {
                out.write_char(',')?;
            }
            first = false;
            let name: String<512> = key.decode().map_err(|_| fmt::Error)?;
            quote(out, &name)?;
            out.write_char(':')?;
            if ["key", "password", "cookie", "csrf", "secret", "token"]
                .iter()
                .any(|s| name.to_lowercase_fixed().contains(s))
                || name.starts_with("x_")
            {
                quote(out, "[REDACTED]")?;
            } else {
                redact(out, child)?;
            }
        }
        out.write_char('}')
    } else if let Ok(items) = value.items() {
        out.write_char('[')?;
        let mut first = true;
        for child in items {
            if !first {
                out.write_char(',')?;
            }
            first = false;
            redact(out, child)?;
        }
        out.write_char(']')
    } else {
        out.write_str(value.raw())
    }
}
trait Lowercase {
    fn to_lowercase_fixed(&self) -> String<512>;
}
impl Lowercase for str {
    fn to_lowercase_fixed(&self) -> String<512> {
        let mut out = String::new();
        for c in self.chars() {
            for c in c.to_lowercase() {
                let _ = out.push(c);
            }
        }
        out
    }
}
fn directories(path: &str) -> Result<(), &'static str> {
    for (i, _) in path
        .match_indices('/')
        .chain(core::iter::once((path.len(), "")))
    {
        if i == 0 {
            continue;
        }
        match fs::mkdir(&path[..i], Mode::RUSR | Mode::WUSR | Mode::XUSR) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
            Err(_) => return Err("cannot create config directory"),
        }
    }
    Ok(())
}

pub fn store(path: &str, bytes: &[u8]) -> Result<(), &'static str> {
    let parent = path
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or(".");
    directories(parent)?;
    let mut tmp = String::<4096>::try_from(path).map_err(|_| "config path too long")?;
    let mut random = [0; 8];
    fill_random(&mut random)?;
    tmp.push_str(".tmp-").map_err(|_| "config path too long")?;
    for byte in random {
        write!(tmp, "{byte:02x}").map_err(|_| "config path too long")?;
    }
    let result = (|| {
        let file = fs::open(
            tmp.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| "cannot create temporary config")?;
        write_fd(&file, bytes)?;
        fs::fsync(&file).map_err(|_| "cannot sync config")?;
        fs::rename(tmp.as_str(), path).map_err(|_| "cannot replace config")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::unlink(tmp.as_str());
    }
    result
}
