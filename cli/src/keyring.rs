//! Secret Service interoperability with unifly's service/username attributes.
use crate::{Options, Output, config, process, setting, terminal};
use core::fmt::Write;
use heapless::String;
pub fn resolve(reference: &str, environment: &str) -> Result<String<8192>, &'static str> {
    let name = reference
        .strip_prefix("keyring:")
        .ok_or("invalid keyring reference")?;
    if name.is_empty() {
        return Err("empty keyring reference");
    }
    let mut output = [0; 8192];
    let (n, status) = process::run(
        setting(environment, "UNIFI_SECRET_TOOL").unwrap_or("/usr/bin/secret-tool"),
        &["lookup", "service", "unifly", "username", name],
        environment,
        &[],
        &mut output,
        30,
        false,
        terminal::cancelled,
    )?;
    if status != 0 {
        return Err(
            "keyring lookup failed; unlock Secret Service and check secret-tool is installed",
        );
    }
    // libsecret 0.21.4+ emits exact bytes for piped stdout; preserve stored trailing newlines.
    String::try_from(
        core::str::from_utf8(&output[..n]).map_err(|_| "keyring secret must be UTF-8")?,
    )
    .map_err(|_| "keyring secret exceeds capacity")
}
pub fn store(
    profile: &str,
    kind: &str,
    secret: &str,
    environment: &str,
) -> Result<String<520>, &'static str> {
    if secret.is_empty() || secret.as_bytes().contains(&0) {
        return Err("secret cannot be empty or contain NUL");
    }
    let mut name = String::<512>::new();
    write!(name, "{profile}/{kind}").map_err(|_| "keyring name too long")?;
    let mut output = [0; 1024];
    let (_, status) = process::run(
        setting(environment, "UNIFI_SECRET_TOOL").unwrap_or("/usr/bin/secret-tool"),
        &[
            "store",
            "--label",
            "UniFi profile credential",
            "service",
            "unifly",
            "username",
            &name,
        ],
        environment,
        secret.as_bytes(),
        &mut output,
        60,
        false,
        terminal::cancelled,
    )?;
    if status != 0 {
        return Err(
            "keyring storage failed; unlock Secret Service and check secret-tool is installed",
        );
    }
    let mut reference = String::<520>::new();
    write!(reference, "keyring:{name}").unwrap();
    Ok(reference)
}
pub fn configure(
    file: &config::File,
    args: &[&str],
    options: &Options<'_>,
    environment: &str,
) -> Result<i32, &'static str> {
    if !file.can_persist() {
        return Err("keyring profile setup requires --config or a configuration directory");
    }
    if args.len() > 3 {
        return Err("usage: config set-password [PROFILE] [--secret-kind api-key|password|both]");
    }
    let active = file.active()?;
    let profile = args
        .get(2)
        .copied()
        .or(options.profile)
        .or_else(|| setting(environment, "UNIFI_PROFILE"))
        .unwrap_or(&active);
    if !file.has_profile(profile)? {
        return Err("profile does not exist; run config init first");
    }
    let mode = file.value::<64>(profile, "authMode")?;
    let user = file.value::<512>(profile, "username")?;
    let kind = options.secret_kind.unwrap_or(match mode.as_deref() {
        Some("hybrid") => "both",
        Some("session" | "legacy") => "password",
        _ if user.is_some() => "password",
        _ => "api-key",
    });
    if !["api-key", "password", "both"].contains(&kind) {
        return Err("secret kind must be api-key, password or both");
    }
    for (secret_kind, env, field) in [
        ("api-key", "UNIFI_API_KEY", "apiKey"),
        ("password", "UNIFI_PASSWORD", "password"),
    ] {
        if kind != "both" && kind != secret_kind {
            continue;
        }
        let prompted;
        let secret = if let Some(value) = setting(environment, env) {
            value
        } else {
            prompted = terminal::Terminal::open()?.password(if secret_kind == "api-key" {
                "API key"
            } else {
                "Password"
            })?;
            &prompted
        };
        let reference = store(profile, secret_kind, secret, environment)?;
        file.secret_reference(profile, field, &reference)?;
    }
    if !options.quiet {
        writeln!(Output(false), "Credentials stored in the system keyring.")
            .map_err(|_| "output failed")?;
    }
    Ok(0)
}
