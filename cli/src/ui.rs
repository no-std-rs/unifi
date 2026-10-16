use crate::routes::{COMMAND_OPTIONS, ROUTES};
use core::fmt::{self, Write};

fn command_options(words: &[&str]) -> &'static [&'static str] {
    COMMAND_OPTIONS
        .iter()
        .filter(|(path, _)| words.starts_with(path))
        .max_by_key(|(path, _)| path.len())
        .map_or(&[], |(_, flags)| *flags)
}
pub fn validate_typed_options(words: &[&str], options: &[&str]) -> Result<(), &'static str> {
    let allowed = command_options(words);
    for option in options {
        let aliases: &[&str] = match *option {
            "minutes" | "time-limit" => &["minutes", "time-limit"],
            "data-limit" | "data-limit-mb" => &["data-limit", "data-limit-mb"],
            "rx-limit" | "rx-limit-kbps" => &["rx-limit", "rx-limit-kbps"],
            "tx-limit" | "tx-limit-kbps" => &["tx-limit", "tx-limit-kbps"],
            _ => core::slice::from_ref(option),
        };
        if !aliases.iter().any(|name| allowed.contains(name)) {
            return Err(
                "body option is not valid for this command; use --help or --field KEY=JSON",
            );
        }
    }
    Ok(())
}

pub fn help(out: &mut dyn Write, words: &[&str]) -> fmt::Result {
    if words == ["connection"] {
        return writeln!(
            out,
            "Connection, TLS and output options:\n  --controller URL, --url URL   Controller/API URL\n  --api-key KEY                 API credential (prefer UNIFI_API_KEY)\n  --profile NAME, --config FILE Saved profile and configuration\n  --site NAME_OR_ID, --console ID  Site and cloud console selectors\n  --tls auto|openssl|embedded   TLS backend\n  --ca-file FILE               PEM trust file for OpenSSL\n  --ca-cert FILE               DER trust file for embedded TLS\n  --connect IP                 Connection override; retains TLS hostname\n  --insecure                   Explicitly disable certificate checks\n  --timeout SEC                I/O timeout (default 30; maximum 3600)\n  --cookie-file FILE           Origin-bound session cache\n  --standalone                 Controller without UniFi OS\n  --output FORMAT              table, json, json-compact, yaml, plain\n  --limit N, --offset N, --all  Pagination\n  --filter EXPR, --query K=V   Filtering and documented query parameters\n  --color auto|always|never     Table color policy\n  --theme NAME                 plain, nord, dracula, silkcircuit-neon\n  --demo                       Mask identifying output\n  --save FILE                  Save response bytes\n  --quiet, --verbose           Output controls\n  --allow-write, --yes          Permit non-GET operations\n\nCommand options override UNIFI_* environment values, then .env, then profile defaults.\nWithout a URL, use https://api.ui.com and discover a unique connected console/site."
        );
    }
    let route = ROUTES
        .iter()
        .filter(|r| words.starts_with(r.words))
        .max_by_key(|r| r.words.len());
    let words = route.map_or(words, |r| r.words);
    if words.is_empty() {
        writeln!(
            out,
            "unifi {} — allocation-free UniFi CLI\n\nUsage: unifi <resource> <command> [OPTIONS]\n",
            env!("CARGO_PKG_VERSION")
        )?;
        writeln!(
            out,
            "  wifi list                 Wi-Fi broadcasts\n  devices list              Access points and switches\n  clients list              Connected clients\n  cloud hosts               Cloud consoles\n  tui                       Interactive dashboard\n"
        )?;
        writeln!(
            out,
            "Resources: acl, admin, alarms, clients, cloud, config, countries, devices,\n           dns, dpi, events, firewall, hotspot, nat, network, networks, protect, radius,\n           session, settings, sites, stats, switching, system, topology,\n           traffic-lists, vpn, wans, wifi\n"
        )?;
    } else {
        write!(out, "Usage: unifi")?;
        for word in words {
            write!(out, " {word}")?;
        }
        if let Some(usage) = local_usage(words) {
            writeln!(out, " {usage}\n")?;
        } else if let Some(route) = route {
            for param in route.params {
                write!(out, " <{}>", param.trim_start_matches(['$', '?', '!', '@']))?;
            }
            writeln!(out, " [OPTIONS]\n\n{}\n", route.operation.summary)?;
        } else {
            writeln!(out, " <command> [OPTIONS]\n")?;
            let mut seen = heapless::Vec::<&str, 128>::new();
            for path in ROUTES.iter().map(|r| r.words).chain(LOCAL.iter().copied()) {
                if path.starts_with(words)
                    && let Some(child) = path.get(words.len())
                    && !seen.contains(child)
                {
                    let _ = seen.push(child);
                    writeln!(out, "  {child}")?;
                }
            }
            if seen.is_empty() {
                writeln!(out, "No commands match this resource.")?;
            }
            writeln!(out)?;
        }
        let flags = command_options(words);
        let extra = additional_options(words);
        if !flags.is_empty() || !extra.is_empty() {
            writeln!(out, "Command options:")?;
            for (i, flag) in flags.iter().chain(extra.iter()).enumerate() {
                if i >= flags.len() && flags.contains(flag) {
                    continue;
                }
                let (value, description) = option_help(flag);
                let mut label = heapless::String::<64>::new();
                write!(label, "--{flag}{value}").map_err(|_| fmt::Error)?;
                if description.is_empty() {
                    writeln!(out, "  {label}")?;
                } else {
                    writeln!(out, "  {label:<30} {description}")?;
                }
            }
            writeln!(out)?;
        }
    }
    writeln!(
        out,
        "Connection and output:\n  -c, --controller URL   Controller/API URL (or UNIFI_URL)\n  -p, --profile NAME     Saved controller profile\n  -s, --site NAME_OR_ID  Site selector\n      --console ID       Cloud console selector\n  -o, --output FORMAT    table, json, json-compact, yaml, plain\n      --all              Fetch every page\n      --allow-write      Permit non-GET operations\n  -q, --quiet            Suppress successful output\n  -v, --verbose          Operation names on stderr\n  -h, --help             Command help\n"
    )?;
    if route.is_some_and(|r| r.operation.method != unifi::Method::GET)
        || words.first() == Some(&"call")
        || words.first() == Some(&"api")
    {
        writeln!(
            out,
            "Request body: --body FILE (or --from-file/-F), --data JSON, --field KEY=JSON.\n"
        )?;
    }
    writeln!(
        out,
        "Credentials: UNIFI_API_KEY, .env, or a profile. HTTPS verifies system trust.\nAdvanced: operations [SURFACE], call SURFACE OPERATION, api PATH, completions SHELL.\nUse unifi help connection for all connection, TLS, paging and output options."
    )
}
fn additional_options(words: &[&str]) -> &'static [&'static str] {
    match words {
        ["config", "set-password"] => &["secret-kind"],
        ["config", "init" | "cloud-setup"] => &["credential-storage"],
        ["session", "login"] => &["standalone", "cookie-file"],
        ["protect", "files", "list" | "upload"] => &["type"],
        ["tui"] => &["count"],
        ["completions" | "completion", ..] => &["no-descriptions"],
        _ => &[],
    }
}
fn option_help(name: &str) -> (&'static str, &'static str) {
    match name {
        "name" => (" NAME", "Resource name"),
        "enabled" | "on" => (" BOOL", "true or false"),
        "from-file" => (" FILE", "JSON request body"),
        "all" => ("", "Fetch every page"),
        "limit" | "page-limit" => (" N", "Items per page"),
        "offset" | "page-offset" => (" N", "Items to skip"),
        "filter" => (" EXPR", "Filter matching records"),
        "hide-page" => ("", "Omit pagination metadata"),
        "id-only" => ("", "One resource ID per line"),
        "with-clients" => ("", "Attach connected clients and devices"),
        "with-secrets" => ("", "Include credential fields in output"),
        "allow-secret-removal" => ("", "Allow explicit clearing of credentials"),
        "broadcast-type" => (" TYPE", "standard or iot-optimized"),
        "security" => (" TYPE", "open, wpa2-personal, wpa3-personal, ..."),
        "passphrase" => (" TEXT", "Wi-Fi passphrase"),
        "frequencies" => (" CSV", "2.4, 5, 6 GHz"),
        "network" => (" NAME_OR_ID", "Network selector"),
        "management" => (" TYPE", "gateway, switch, unmanaged"),
        "vlan" => (" ID", "802.1Q VLAN ID"),
        "ipv4-host" => (" CIDR", "Gateway IPv4 address and prefix"),
        "dhcp-start" | "dhcp-stop" => (" IPV4", "DHCP address range"),
        "dhcp-lease" => (" SEC", "DHCP lease duration"),
        "dns" => (" CSV", "DNS server addresses"),
        "zone" => (" NAME_OR_ID", "Firewall zone"),
        "domain" => (" NAME", "DNS record domain"),
        "record-type" => (" TYPE", "A, AAAA, CNAME, MX, SRV, TXT, FORWARD_DOMAIN"),
        "value" => (" VALUE", "Record value"),
        "ttl" => (" SEC", "DNS TTL"),
        "list-type" => (" TYPE", "ipv4-addresses, ipv6-addresses, ports"),
        "items" | "members" | "networks" => (" CSV", "Comma-separated resource values"),
        "get" => ("", "Read current ordering"),
        "set" => (" CSV", "Ordered resource IDs"),
        "after-system" => ("", "Place policy after system rules"),
        "source-zone" | "dest-zone" => (" NAME_OR_ID", "Firewall zone selector"),
        "source-port" | "destination-port" | "src-port" | "dst-port" => {
            (" CSV", "Ports and ranges")
        }
        "src-ip" | "dst-ip" => (" CSV", "Addresses, subnets or ranges"),
        "src-network" | "dst-network" => (" CSV", "Network names or IDs"),
        "src-address-group" | "dst-address-group" | "src-port-group" | "dst-port-group" => {
            (" NAME_OR_ID", "Traffic matching list")
        }
        "states" => (" CSV", "new, invalid, established, related"),
        "ip-version" => (" VERSION", "ipv4, ipv6, ipv4-and-ipv6"),
        "path" => (" FILE", "Download destination"),
        "sites" => (" CSV_OR_JSON", "Site names, HOST/SITE pairs, or objects"),
        "type" => (" TYPE", "Resource/query type"),
        "group-by" => (" TYPE", "app or category"),
        "types" => (" CSV", "Subscribed event types"),
        "liveview" => (" ID", "Liveview displayed by a viewer"),
        "count" => (" N", "Number of results or refreshes"),
        "secret-kind" => (" KIND", "api-key, password, both"),
        "credential-storage" => (" KIND", "env, keyring, file"),
        "no-descriptions" => ("", "Emit completion names only"),
        _ if crate::compat::boolean(name) => (" [BOOL]", ""),
        "reset" | "low" | "medium" | "high" | "package" | "low-quality" | "ignore-limit"
        | "standalone" => ("", ""),
        _ => (" VALUE", ""),
    }
}
fn local_usage(words: &[&str]) -> Option<&'static str> {
    Some(match words {
        ["api"] => "PATH [--method METHOD] [--data JSON]",
        ["call"] => "SURFACE OPERATION [--param NAME=VALUE] [--query NAME=VALUE]",
        ["operations"] => "[network|site-manager|protect|session-api]",
        ["protect", "files", "list"] => "[TYPE] [OPTIONS]",
        ["protect", "files", "upload"] => "FILE [--type TYPE] --allow-write",
        ["vpn", "peers", "list"] => "[SERVER_ID] [OPTIONS]",
        ["completions" | "completion"] => "bash|zsh|fish|powershell|elvish",
        ["devices", "ports"] => "DEVICE [--with-clients] [OPTIONS]",
        ["devices", "ports-export"] => "DEVICE [--all] [--with-clients] [OPTIONS]",
        ["devices", "port-set"] => {
            "DEVICE [PORT --name NAME --mode access|trunk|mirror --native-vlan NETWORK --tagged-vlans CSV --poe MODE --speed SPEED | --reset | --from-file FILE] --allow-write"
        }
        ["clients", "set-ip"] => "MAC --ip IPV4 [--network NAME] --allow-write",
        ["clients", "remove-ip"] => "MAC [--network NAME] --allow-write",
        ["topology"] => "[--output json|table] [OPTIONS]",
        ["tui"] => {
            "[--profile NAME] [--log-file FILE] [OPTIONS]\n\nKeys: 1–0 views, r refresh, j/k scroll, / search, : command, q quit"
        }
        ["cloud", "switch"] => "SITE [--profile NAME]",
        ["config", "set-password"] => "[PROFILE] [--secret-kind api-key|password|both]",
        ["config", "set"] => "KEY VALUE",
        ["config", "use"] => "PROFILE",
        ["config", "init" | "cloud-setup"] => "[--profile NAME] [--controller URL]",
        ["config", "theme"] => "[NAME]",
        ["config", "profiles" | "show"] => "[OPTIONS]",
        _ => return None,
    })
}
const LOCAL: &[&[&str]] = &[
    &["api"],
    &["call"],
    &["operations"],
    &["completions"],
    &["completion"],
    &["cloud", "switch"],
    &["config", "init"],
    &["config", "cloud-setup"],
    &["config", "show"],
    &["config", "set"],
    &["config", "profiles"],
    &["config", "use"],
    &["config", "theme"],
    &["config", "set-password"],
    &["clients", "set-ip"],
    &["clients", "remove-ip"],
    &["devices", "ports"],
    &["devices", "ports-export"],
    &["devices", "port-set"],
    &["topology"],
    &["tui"],
];
const FLAGS: &[&str] = &[
    "--help",
    "--version",
    "--controller",
    "--site",
    "--console",
    "--profile",
    "--output",
    "--limit",
    "--offset",
    "--filter",
    "--query",
    "--allow-write",
    "--config",
    "--cookie-file",
    "--standalone",
    "--tls",
    "--ca-file",
    "--ca-cert",
    "--connect",
    "--timeout",
    "--insecure",
    "--quiet",
    "--verbose",
    "--save",
    "--all",
    "--demo",
    "--color",
    "--theme",
];

/// Offline completion candidates. Never authenticates or contacts a controller.
pub fn complete(out: &mut dyn Write, words: &[&str]) -> fmt::Result {
    if let Some(last) = words.last() {
        let values: Option<&[&str]> = match *last {
            "--output" | "-o" => Some(&["table", "json", "json-compact", "yaml", "plain"]),
            "--tls" => Some(&["auto", "openssl", "embedded"]),
            "--method" => Some(&["get", "post", "put", "patch", "delete", "head", "options"]),
            "--interval" => Some(&["5m", "hourly", "daily", "monthly"]),
            "--type" => Some(&["5m", "1h"]),
            "--color" => Some(&["auto", "always", "never"]),
            "--theme" => Some(&["plain", "nord", "dracula", "silkcircuit-neon"]),
            "--mode" => Some(&["access", "trunk", "mirror"]),
            "--poe" => Some(&["auto", "on", "off", "pasv24", "passthrough"]),
            "--speed" => Some(&["auto", "10", "100", "1000", "2500", "5000", "10000"]),
            "--secret-kind" => Some(&["api-key", "password", "both"]),
            "--broadcast-type" => Some(&["standard", "iot-optimized"]),
            "--security" => Some(&[
                "open",
                "wpa2-personal",
                "wpa3-personal",
                "wpa2-wpa3-personal",
                "wpa2-enterprise",
                "wpa3-enterprise",
                "wpa2-wpa3-enterprise",
            ]),
            "--management" => Some(&["gateway", "switch", "unmanaged"]),
            "--record-type" => Some(&["A", "AAAA", "CNAME", "MX", "SRV", "TXT", "FORWARD_DOMAIN"]),
            "--list-type" => Some(&["ipv4-addresses", "ipv6-addresses", "ports"]),
            "--ip-version" => Some(&["ipv4", "ipv6", "ipv4-and-ipv6"]),
            "--enabled" | "--on" => Some(&["true", "false"]),
            "completions" | "completion" => Some(&["bash", "zsh", "fish", "powershell", "elvish"]),
            "operations" | "call" => Some(&["network", "site-manager", "protect", "session-api"]),
            _ => None,
        };
        if let Some(values) = values {
            for value in values {
                writeln!(out, "{value}")?;
            }
            return Ok(());
        }
    }
    let mut prefix = heapless::Vec::<&str, 8>::new();
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        i += 1;
        if word.starts_with('-') {
            if crate::compat::boolean(word.trim_start_matches('-')) {
                if words
                    .get(i)
                    .is_some_and(|value| matches!(*value, "true" | "false"))
                {
                    i += 1;
                }
                continue;
            }
            if !word.contains('=')
                && ![
                    "--help",
                    "--version",
                    "--quiet",
                    "--verbose",
                    "--allow-write",
                    "--yes",
                    "--insecure",
                    "--standalone",
                    "--all",
                    "--demo",
                    "--hide-page",
                    "--id-only",
                    "--with-clients",
                    "--with-secrets",
                    "--allow-secret-removal",
                    "--reset",
                    "--low",
                    "--medium",
                    "--high",
                    "--package",
                    "--low-quality",
                    "--ignore-limit",
                    "--no-cache",
                    "--no-effects",
                    "-h",
                    "-V",
                    "-q",
                    "-v",
                    "-y",
                    "-k",
                ]
                .contains(&word)
            {
                i += 1;
            }
            continue;
        }
        if prefix.push(word).is_err() {
            break;
        }
        if !ROUTES.iter().any(|r| r.words.starts_with(&prefix))
            && !LOCAL.iter().any(|r| r.starts_with(&prefix))
        {
            prefix.pop();
            break;
        }
    }
    let mut candidates = heapless::Vec::<&str, 128>::new();
    for route in ROUTES.iter().map(|r| r.words).chain(LOCAL.iter().copied()) {
        if route.starts_with(&prefix)
            && let Some(word) = route.get(prefix.len())
            && !candidates.contains(word)
        {
            let _ = candidates.push(word);
        }
    }
    for flag in FLAGS {
        if !candidates.contains(flag) {
            let _ = candidates.push(flag);
        }
    }
    for word in candidates {
        writeln!(out, "{word}")?;
    }
    let options = command_options(&prefix);
    let extra = additional_options(&prefix);
    for (i, flag) in options.iter().chain(extra.iter()).enumerate() {
        if FLAGS
            .iter()
            .any(|known| known.trim_start_matches('-') == *flag)
            || i >= options.len() && options.contains(flag)
        {
            continue;
        }
        writeln!(out, "--{flag}")?;
    }
    if ROUTES
        .iter()
        .any(|r| r.words == prefix && r.operation.method != unifi::Method::GET)
    {
        for flag in ["--body", "--data", "--field"] {
            writeln!(out, "{flag}")?;
        }
    }
    Ok(())
}
pub fn completions(out: &mut dyn Write, shell: &str) -> Result<(), &'static str> {
    let script = match shell {
        "bash" => {
            r#"_unifi_complete() {
  local candidates
  candidates=$(unifi __complete "${COMP_WORDS[@]:1:$((COMP_CWORD-1))}")
  mapfile -t COMPREPLY < <(compgen -W "$candidates" -- "${COMP_WORDS[COMP_CWORD]}")
}
complete -F _unifi_complete unifi
"#
        }
        "zsh" => {
            r#"#compdef unifi
_unifi_complete() {
  local -a candidates
  candidates=("${(@f)$(unifi __complete "${words[@]:1:$((CURRENT-2))}")}")
  compadd -- "${candidates[@]}"
}
compdef _unifi_complete unifi
"#
        }
        "fish" => "complete -c unifi -f -a '(unifi __complete (commandline -opc)[2..-1])'\n",
        "powershell" => {
            r#"Register-ArgumentCompleter -Native -CommandName unifi -ScriptBlock {
  param($wordToComplete, $commandAst, $cursorPosition)
  $previous = @($commandAst.CommandElements | Select-Object -Skip 1 | Where-Object { $_.Extent.EndOffset -le ($cursorPosition - $wordToComplete.Length) } | ForEach-Object { $_.Value })
  & unifi __complete @previous | Where-Object { $_.StartsWith($wordToComplete) } | ForEach-Object {
    [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
  }
}
"#
        }
        "elvish" => {
            r#"set edit:completion:arg-completer[unifi] = {|@words|
  unifi __complete $@words[1..-1]
}
"#
        }
        _ => return Err("completion shell must be bash, zsh, fish, powershell or elvish"),
    };
    out.write_str(script).map_err(|_| "output failed")
}
