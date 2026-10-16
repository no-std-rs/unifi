//! Guided local/cloud profiles. Cloud validation is strictly GET-only.
use crate::{Options, Output, config, flows, keyring, pages, storage, terminal::Terminal};
use core::fmt::Write;
use heapless::String;
use unifi::{Json, json::ObjectWriter};

struct Credential {
    value: String<8192>,
    storage: &'static str,
    variable: &'static str,
    kind: &'static str,
}
impl Credential {
    fn collect(
        ui: &mut Option<Terminal>,
        existing: Option<&str>,
        available: bool,
        kind: &'static str,
        storage: Option<&str>,
    ) -> Result<Self, &'static str> {
        let (label, variable) = if kind == "api-key" {
            ("API key", "UNIFI_API_KEY")
        } else {
            ("Password", "UNIFI_PASSWORD")
        };
        let value = if let Some(value) = existing {
            String::try_from(value).map_err(|_| "credential exceeds capacity")?
        } else {
            ui.as_mut()
                .ok_or("export credentials or use an interactive terminal")?
                .password(label)?
        };
        if value.is_empty() || value.as_bytes().contains(&0) {
            return Err("credential cannot be empty or contain NUL");
        }
        let prompted;
        let choice = if let Some(storage) = storage {
            storage
        } else if let Some(ui) = ui {
            prompted = ui.line(
                if kind == "api-key" {
                    "API key storage (env/keyring/file)"
                } else {
                    "Password storage (env/keyring/file)"
                },
                if available { "env" } else { "keyring" },
            )?;
            &prompted
        } else if available {
            "env"
        } else {
            "keyring"
        };
        let storage = match choice {
            "env" if available => "env",
            "env" => {
                return Err("environment storage requires the corresponding UNIFI_* credential");
            }
            "keyring" => "keyring",
            "file" => "file",
            _ => return Err("credential storage must be env, keyring or file"),
        };
        Ok(Self {
            value,
            storage,
            variable,
            kind,
        })
    }
    fn write(
        &self,
        w: &mut ObjectWriter<'_>,
        profile: &str,
        environment: &str,
    ) -> Result<(), &'static str> {
        let field = if self.kind == "api-key" {
            "apiKey"
        } else {
            "password"
        };
        match self.storage {
            "env" => {
                w.field(
                    if self.kind == "api-key" {
                        "apiKeyEnv"
                    } else {
                        "passwordEnv"
                    },
                    &self.variable,
                )
                .map_err(|_| "profile exceeds capacity")?;
            }
            "keyring" => {
                let reference = keyring::store(profile, self.kind, &self.value, environment)?;
                w.field(field, &reference.as_str())
                    .map_err(|_| "profile exceeds capacity")?;
            }
            _ => {
                w.field(field, &self.value.as_str())
                    .map_err(|_| "profile exceeds capacity")?;
            }
        };
        Ok(())
    }
}
fn yes(value: &str) -> Result<bool, &'static str> {
    match value {
        "yes" | "y" | "true" => Ok(true),
        "no" | "n" | "false" => Ok(false),
        _ => Err("answer yes or no"),
    }
}
fn label(value: Json<'_>, host: bool) -> Option<unifi::json::JsonString<'_>> {
    let named = if host {
        value
            .get("reportedState")
            .ok()
            .flatten()
            .and_then(|v| v.get("name").ok().flatten())
    } else {
        value.get("name").ok().flatten()
    };
    named
        .and_then(|v| v.string().ok())
        .or_else(|| value.get("id").ok().flatten().and_then(|v| v.string().ok()))
}
fn eligible(value: Json<'_>, host: bool, explicit: bool) -> bool {
    value
        .get("id")
        .ok()
        .flatten()
        .is_some_and(|v| v.string().is_ok())
        && (!host
            || value
                .get("type")
                .ok()
                .flatten()
                .and_then(|v| v.string().ok())
                .is_some_and(|v| v.equals("console"))
                && (explicit
                    || value
                        .get("reportedState")
                        .ok()
                        .flatten()
                        .and_then(|v| v.get("state").ok().flatten())
                        .and_then(|v| v.string().ok())
                        .is_some_and(|v| v.equals("connected"))))
}
fn matches(row: Json<'_>, wanted: &str, host: bool, exact: bool) -> bool {
    fn same(value: unifi::json::JsonString<'_>, wanted: &str, exact: bool) -> bool {
        if exact {
            return value.equals(wanted);
        }
        value
            .chars()
            .map(|c| c.to_ascii_lowercase())
            .eq(wanted.chars().map(|c| c.to_ascii_lowercase()))
    }
    ["id", "name", "internalReference"].iter().any(|key| {
        row.get(key)
            .ok()
            .flatten()
            .and_then(|v| v.string().ok())
            .is_some_and(|value| same(value, wanted, exact))
    }) || label(row, host).is_some_and(|value| same(value, wanted, exact))
}
fn choose(
    rows: Json<'_>,
    selector: Option<&str>,
    host: bool,
    ui: &mut Option<Terminal>,
) -> Result<String<512>, &'static str> {
    let records = rows
        .items()
        .map_err(|_| "discovery did not return an array")?;
    let mut selected = None;
    let mut count = 0usize;
    let exact = selector.is_some_and(|wanted| {
        records
            .clone()
            .filter(|v| eligible(*v, host, true))
            .any(|row| matches(row, wanted, host, true))
    });
    for row in records
        .clone()
        .filter(|v| eligible(*v, host, selector.is_some()))
    {
        if selector.is_none_or(|wanted| matches(row, wanted, host, exact)) {
            count += 1;
            selected = Some(row);
        }
    }
    if count == 1 {
        return flows::owned(
            selected.unwrap(),
            if host { "id" } else { "internalReference" },
        );
    }
    if count == 0 {
        return Err("no matching console/site found during setup");
    }
    if selector.is_some() {
        return Err("setup selector is ambiguous; use an exact ID");
    }
    let tty = ui
        .as_mut()
        .ok_or("multiple choices; supply --console and --site or use an interactive terminal")?;
    if count > 100 {
        return Err("too many setup choices; supply --console and --site");
    }
    for (index, row) in records
        .clone()
        .filter(|v| eligible(*v, host, false))
        .enumerate()
    {
        write!(tty, "{}. ", index + 1).map_err(|_| "terminal output failed")?;
        if let Some(name) = label(row, host) {
            for c in name.chars().take(60) {
                tty.write_char(if c.is_control() { '?' } else { c })
                    .map_err(|_| "terminal output failed")?;
            }
        }
        tty.write_str("\r\n")
            .map_err(|_| "terminal output failed")?;
    }
    let choice = tty.line(
        if host {
            "Console number or ID"
        } else {
            "Site number or ID"
        },
        "1",
    )?;
    if let Ok(index) = choice.parse::<usize>() {
        let row = records
            .filter(|v| eligible(*v, host, false))
            .nth(index.checked_sub(1).ok_or("choice is out of range")?)
            .ok_or("choice is out of range")?;
        return flows::owned(row, if host { "id" } else { "internalReference" });
    }
    choose(rows, Some(&choice), host, &mut None)
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    file: &config::File,
    cloud: bool,
    options: &Options<'_>,
    environment: &str,
    key_env: Option<&str>,
    username_env: Option<&str>,
    password_env: Option<&str>,
    storage: &mut storage::Storage,
) -> Result<i32, &'static str> {
    if !file.can_persist() {
        return Err("setup requires --config or a configuration directory");
    }
    let mut ui = if crate::terminal::available() {
        Some(Terminal::open()?)
    } else {
        None
    };
    let name = if let Some(name) = options.profile {
        String::<8192>::try_from(name).map_err(|_| "profile name too long")?
    } else if let Some(ui) = &mut ui {
        ui.line("Profile name", if cloud { "cloud" } else { "default" })?
    } else {
        String::try_from(if cloud { "cloud" } else { "default" }).unwrap()
    };
    if name.is_empty() || name.len() > 128 {
        return Err("profile names must contain 1–128 bytes");
    }
    if file.has_profile(&name)? {
        return Err("profile already exists; choose another name");
    }
    let controller = if let Some(url) = options.url {
        String::<8192>::try_from(url).map_err(|_| "controller URL too long")?
    } else if cloud {
        String::try_from("https://api.ui.com").unwrap()
    } else {
        ui.as_mut()
            .ok_or("local setup needs --controller URL")?
            .line("Controller URL", "https://192.168.1.1")?
    };
    let url = crate::parse_url(&controller)?;
    let auth = if cloud {
        String::try_from("cloud").unwrap()
    } else if let Some(ui) = &mut ui {
        ui.line("Authentication (api-key/session/hybrid)", "api-key")?
    } else {
        String::try_from("api-key").unwrap()
    };
    let mode = match auth.as_str() {
        "api-key" => "integration",
        "session" => "session",
        "hybrid" => "hybrid",
        "cloud" if cloud => "cloud",
        _ => return Err("unknown authentication mode"),
    };
    let key = if mode != "session" {
        Some(Credential::collect(
            &mut ui,
            options.api_key.or(key_env),
            options.api_key.is_none() && key_env.is_some(),
            "api-key",
            options.credential_storage,
        )?)
    } else {
        None
    };
    let username = if mode == "session" || mode == "hybrid" {
        Some(if let Some(ui) = &mut ui {
            ui.line("Username", username_env.unwrap_or(""))?
        } else {
            String::try_from(username_env.ok_or("session setup needs UNIFI_USERNAME")?)
                .map_err(|_| "username exceeds capacity")?
        })
    } else {
        None
    };
    if username.as_ref().is_some_and(|v| v.is_empty()) {
        return Err("username cannot be empty");
    }
    let password = if username.is_some() {
        Some(Credential::collect(
            &mut ui,
            password_env,
            password_env.is_some(),
            "password",
            options.credential_storage,
        )?)
    } else {
        None
    };
    let insecure = if options.insecure_explicit {
        options.insecure
    } else if cloud {
        false
    } else if let Some(ui) = &mut ui {
        !yes(&ui.line("Verify TLS certificates (yes/no)", "yes")?)?
    } else {
        false
    };
    let outgoing = Options {
        insecure,
        cloud,
        ..*options
    };
    let (console, site) = if cloud {
        let key = &key.as_ref().unwrap().value;
        let hosts = Json::parse(pages::collect(
            &url,
            url.base,
            key,
            &unifi::site_manager::list_hosts().unwrap(),
            &outgoing,
            &mut storage.response,
            &mut storage.combined,
        )?)
        .unwrap();
        let console = choose(hosts, options.console, true, &mut ui)?;
        let mut base = String::<4096>::try_from(url.base).map_err(|_| "API base path too long")?;
        base.push_str(
            unifi::site_manager::connector_get(&console, "proxy/network/integration")
                .map_err(|_| "invalid console ID")?
                .path(),
        )
        .map_err(|_| "API base path too long")?;
        let sites = Json::parse(pages::collect(
            &url,
            &base,
            key,
            &unifi::network::get_site_overview_page().unwrap(),
            &outgoing,
            &mut storage.response,
            &mut storage.combined,
        )?)
        .unwrap();
        (Some(console), choose(sites, options.site, false, &mut ui)?)
    } else {
        (
            None,
            if let Some(site) = options.site {
                String::try_from(site).map_err(|_| "site selector too long")?
            } else if let Some(ui) = &mut ui {
                let value = ui.line("Site", "default")?;
                String::try_from(value.as_str()).map_err(|_| "site selector too long")?
            } else {
                String::try_from("default").unwrap()
            },
        )
    };
    if let Some(ui) = &mut ui {
        writeln!(ui,"\r\nProfile: {name}\r\nController: {controller}\r\nAuthentication: {mode}\r\nSite: {site}\r").map_err(|_|"terminal output failed")?;
        if !yes(&ui.line("Save profile (yes/no)", "yes")?)? {
            return Err("cancelled");
        }
    }
    let mut body = [0; 32768];
    let mut writer = ObjectWriter::new(&mut body).unwrap();
    for (field, value) in [
        ("controller", controller.as_str()),
        ("site", site.as_str()),
        ("authMode", mode),
    ] {
        writer
            .field(field, &value)
            .map_err(|_| "profile exceeds capacity")?;
    }
    writer
        .field("insecure", &insecure)
        .map_err(|_| "profile exceeds capacity")?;
    if let Some(console) = &console {
        writer
            .field("console", &console.as_str())
            .map_err(|_| "profile exceeds capacity")?;
    }
    if let Some(username) = &username {
        writer
            .field("username", &username.as_str())
            .map_err(|_| "profile exceeds capacity")?;
    }
    for (field, value) in [
        ("caFile", options.ca_file),
        ("caCert", options.ca_cert),
        ("tls", options.tls),
    ] {
        if let Some(value) = value {
            writer
                .field(field, &value)
                .map_err(|_| "profile exceeds capacity")?;
        }
    }
    if let Some(timeout) = options.timeout {
        writer
            .field("timeout", &timeout)
            .map_err(|_| "profile exceeds capacity")?;
    }
    for secret in [key.as_ref(), password.as_ref()].into_iter().flatten() {
        secret.write(&mut writer, &name, environment)?;
    }
    file.create_profile(
        &name,
        Json::parse(writer.finish().map_err(|_| "profile exceeds capacity")?).unwrap(),
    )?;
    if !options.quiet {
        writeln!(Output(false), "Profile {name} saved and selected.")
            .map_err(|_| "output failed")?;
    }
    Ok(0)
}
