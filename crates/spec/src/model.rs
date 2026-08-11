//! The provisioning specification model.
//!
//! Every field is read through [`crate::reader::Reader`], which rejects
//! unknown keys, and validated eagerly so that later stages can assume a
//! well-formed specification.

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::net::{self, Ipv4Cidr, MacAddr};
use crate::reader::Reader;
use crate::secret::{read_secret, resolve, SecretProvider};

pub const SCHEMA_VERSION: i64 = 1;

/// Loading context: where to resolve secrets from.
pub struct Context<'a> {
    pub provider: &'a dyn SecretProvider,
    pub base_dir: PathBuf,
    pub warnings: Vec<String>,
}

impl<'a> Context<'a> {
    pub fn new(provider: &'a dyn SecretProvider, base_dir: impl AsRef<Path>) -> Self {
        Self { provider, base_dir: base_dir.as_ref().to_path_buf(), warnings: Vec::new() }
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    fn secret(&self, reader: &Reader<'_>, key: &str, source: &crate::secret::SecretSource) -> Result<String> {
        let what = format!("`{}.{}`", reader.path(), key);
        resolve(source, self.provider, &self.base_dir, &what)
    }
}

// ---------------------------------------------------------------------- root

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub meta: Meta,
    pub system: System,
    pub user: User,
    pub ssh: Ssh,
    pub network: Network,
    pub hardware: Hardware,
    pub provisioning: Provisioning,
}

impl Spec {
    pub fn from_reader(reader: &mut Reader<'_>, ctx: &mut Context<'_>) -> Result<Self> {
        let meta = Meta::read(&mut reader.table("meta")?)?;
        let system = System::read(&mut reader.table("system")?)?;
        let user = User::read(&mut reader.table("user")?, ctx)?;
        let ssh = Ssh::read(&mut reader.table("ssh")?)?;
        let network =
            Network::read_with_hostname(&mut reader.table("network")?, ctx, &system.hostname)?;
        let hardware = Hardware::read(&mut reader.table("hardware")?)?;
        let provisioning = Provisioning::read(&mut reader.table("provisioning")?)?;

        let spec = Spec { meta, system, user, ssh, network, hardware, provisioning };
        spec.cross_validate(ctx)?;
        Ok(spec)
    }

    fn cross_validate(&self, ctx: &mut Context<'_>) -> Result<()> {
        if self.user.password_hash.is_none() && self.user.authorized_keys.is_empty() {
            return Err(Error::new(
                "`user`: neither `password_hash` nor `authorized_keys` was supplied; there would be \
                 no way to log in.",
            ));
        }
        if self.ssh.enabled && !self.ssh.password_authentication && self.user.authorized_keys.is_empty() {
            return Err(Error::new(
                "`ssh`: password authentication is disabled but no authorized keys were supplied; \
                 the machine would be unreachable. Set `user.authorized_keys` or enable \
                 `ssh.password_authentication`.",
            ));
        }
        if self.network.usb_gadget.is_some() && self.hardware.uart.debug_connector {
            ctx.warn(
                "USB gadget mode and the debug UART are both enabled; on Raspberry Pi 5 the USB-C \
                 port supplies power, so provide power through the GPIO header when using the \
                 gadget link.",
            );
        }
        let mut interfaces: Vec<&str> = Vec::new();
        for connection in self.network.ethernet.iter() {
            interfaces.push(&connection.interface);
        }
        if let Some(gadget) = &self.network.usb_gadget {
            if interfaces.contains(&gadget.interface.as_str()) {
                return Err(Error::new(format!(
                    "`network.usb_gadget.interface`: `{}` is also used by an ethernet connection",
                    gadget.interface
                )));
            }
        }
        let mut ids: Vec<&str> = self.network.ethernet.iter().map(|c| c.id.as_str()).collect();
        ids.extend(self.network.wifi.iter().map(|c| c.id.as_str()));
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        if sorted.len() != ids.len() {
            return Err(Error::new(
                "`network`: connection `id` values must be unique across ethernet and wifi",
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------- meta

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Pi5,
}

impl Target {
    pub fn as_str(self) -> &'static str {
        match self {
            Target::Pi5 => "pi5",
        }
    }

    /// Device-tree blob that must be present on the boot partition.
    pub fn device_tree_blob(self) -> &'static str {
        match self {
            Target::Pi5 => "bcm2712-rpi-5-b.dtb",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub schema_version: i64,
    pub target: Target,
    pub description: Option<String>,
}

impl Meta {
    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let schema_version = reader.integer_or("schema_version", SCHEMA_VERSION)?;
        if schema_version != SCHEMA_VERSION {
            return Err(reader.key_error(
                "schema_version",
                format!("this build understands schema version {SCHEMA_VERSION}"),
            ));
        }
        let target = match reader.enumerated("target", "pi5", &["pi5"])?.as_str() {
            "pi5" => Target::Pi5,
            other => unreachable!("unexpected target `{other}`"),
        };
        let description = reader.opt_string("description")?;
        reader.finish()?;
        Ok(Self { schema_version, target, description })
    }
}

// -------------------------------------------------------------------- system

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct System {
    pub hostname: String,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    pub keymap: Option<String>,
}

impl System {
    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let hostname = reader.req_string("hostname")?;
        validate_hostname(&hostname).map_err(|err| reader.key_error("hostname", err.message))?;
        let timezone = reader.opt_string("timezone")?;
        if let Some(tz) = &timezone {
            if !tz.bytes().all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b)) {
                return Err(reader.key_error("timezone", "contains unexpected characters"));
            }
        }
        let locale = reader.opt_string("locale")?;
        let keymap = reader.opt_string("keymap")?;
        reader.finish()?;
        Ok(Self { hostname, timezone, locale, keymap })
    }
}

/// RFC 1123 host name label.
pub fn validate_hostname(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 63 {
        return Err(Error::new("must be between 1 and 63 characters"));
    }
    if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(Error::new("may only contain ASCII letters, digits and hyphens"));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(Error::new("must not start or end with a hyphen"));
    }
    Ok(())
}

// ---------------------------------------------------------------------- user

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SudoMode {
    /// Member of `sudo`, password required.
    Password,
    /// Member of `sudo` with a NOPASSWD rule.
    NoPassword,
    /// No sudo access.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub name: String,
    pub password_hash: Option<String>,
    pub authorized_keys: Vec<String>,
    pub groups: Vec<String>,
    pub shell: String,
    pub sudo: SudoMode,
}

impl User {
    fn read(reader: &mut Reader<'_>, ctx: &mut Context<'_>) -> Result<Self> {
        let name = reader.req_string("name")?;
        validate_username(&name).map_err(|err| reader.key_error("name", err.message))?;

        let password_hash = match read_secret(reader, "password_hash")? {
            Some(source) => {
                let value = ctx.secret(reader, "password_hash", &source)?;
                validate_password_hash(&value)
                    .map_err(|err| reader.key_error("password_hash", err.message))?;
                Some(value)
            }
            None => None,
        };

        let mut authorized_keys = reader.string_list("authorized_keys")?;
        for path in reader.string_list("authorized_keys_files")? {
            let full = ctx.base_dir.join(&path);
            let contents = std::fs::read_to_string(&full).map_err(|err| {
                reader.key_error("authorized_keys_files", format!("cannot read `{}`: {err}", full.display()))
            })?;
            authorized_keys.extend(
                contents
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty() && !line.starts_with('#'))
                    .map(str::to_string),
            );
        }
        for key in &authorized_keys {
            validate_authorized_key(key).map_err(|err| reader.key_error("authorized_keys", err.message))?;
        }
        authorized_keys.dedup();

        let groups = reader.string_list("groups")?;
        for group in &groups {
            validate_username(group).map_err(|err| reader.key_error("groups", err.message))?;
        }

        let shell = reader.string_or("shell", "/bin/bash")?;
        if !shell.starts_with('/') {
            return Err(reader.key_error("shell", "must be an absolute path"));
        }

        let sudo = match reader.enumerated("sudo", "nopasswd", &["nopasswd", "password", "none"])?.as_str() {
            "nopasswd" => SudoMode::NoPassword,
            "password" => SudoMode::Password,
            _ => SudoMode::None,
        };
        if sudo == SudoMode::Password && password_hash.is_none() {
            ctx.warn(
                "`user.sudo` is `password` but no `password_hash` was supplied; sudo will be unusable.",
            );
        }

        reader.finish()?;
        Ok(Self { name, password_hash, authorized_keys, groups, shell, sudo })
    }
}

pub fn validate_username(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 32 {
        return Err(Error::new("must be between 1 and 32 characters"));
    }
    let mut bytes = name.bytes();
    let first = bytes.next().expect("non-empty");
    if !(first.is_ascii_lowercase() || first == b'_') {
        return Err(Error::new("must start with a lowercase letter or underscore"));
    }
    if !bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-') {
        return Err(Error::new("may only contain lowercase letters, digits, `_` and `-`"));
    }
    Ok(())
}

/// Reject anything that is obviously not a crypt(3) hash, most importantly a
/// plain-text password pasted into the field by mistake.
pub fn validate_password_hash(hash: &str) -> Result<()> {
    let known = ["$y$", "$6$", "$5$", "$2b$", "$2y$", "$7$", "$gy$"];
    if !known.iter().any(|prefix| hash.starts_with(prefix)) {
        return Err(Error::new(
            "does not look like a crypt(3) hash; generate one with `openssl passwd -6` \
             or `mkpasswd --method=yescrypt`",
        ));
    }
    if hash.contains(':') {
        return Err(Error::new("must not contain `:`"));
    }
    Ok(())
}

pub fn validate_authorized_key(key: &str) -> Result<()> {
    let mut fields = key.split_whitespace();
    let algorithm = fields.next().unwrap_or_default();
    let blob = fields.next().unwrap_or_default();
    let known = [
        "ssh-ed25519",
        "ssh-rsa",
        "ecdsa-sha2-nistp256",
        "ecdsa-sha2-nistp384",
        "ecdsa-sha2-nistp521",
        "sk-ssh-ed25519@openssh.com",
        "sk-ecdsa-sha2-nistp256@openssh.com",
    ];
    if !known.contains(&algorithm) {
        return Err(Error::new(format!(
            "`{algorithm}` is not a recognised SSH key type; expected one of {}",
            known.join(", ")
        )));
    }
    if blob.len() < 16 || !blob.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=') {
        return Err(Error::new("the key body is not valid base64"));
    }
    Ok(())
}

// ----------------------------------------------------------------------- ssh

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ssh {
    pub enabled: bool,
    pub port: u16,
    pub password_authentication: bool,
    pub permit_root_login: String,
    pub extra_config: Vec<String>,
}

impl Ssh {
    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let enabled = reader.bool_or("enabled", true)?;
        let port = reader.integer_in_range("port", 22, 1, 65535)? as u16;
        let password_authentication = reader.bool_or("password_authentication", false)?;
        let permit_root_login = reader.enumerated(
            "permit_root_login",
            "no",
            &["no", "yes", "prohibit-password", "forced-commands-only"],
        )?;
        let extra_config = reader.string_list("extra_config")?;
        for line in &extra_config {
            if line.contains('\n') {
                return Err(reader.key_error("extra_config", "entries must be single lines"));
            }
        }
        reader.finish()?;
        Ok(Self { enabled, port, password_authentication, permit_root_login, extra_config })
    }
}

// ------------------------------------------------------------------- network

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpMethod {
    Auto,
    Manual,
    Disabled,
}

impl IpMethod {
    pub fn as_nm(self) -> &'static str {
        match self {
            IpMethod::Auto => "auto",
            IpMethod::Manual => "manual",
            IpMethod::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpConfig {
    pub method: IpMethod,
    pub address: Option<Ipv4Cidr>,
    pub gateway: Option<Ipv4Addr>,
    pub dns: Vec<Ipv4Addr>,
    pub ignore_auto_dns: bool,
    pub ipv6: IpMethod,
}

impl IpConfig {
    fn read(reader: &mut Reader<'_>, ctx: &mut Context<'_>, default_method: &str) -> Result<Self> {
        let method = match reader.enumerated("method", default_method, &["auto", "manual", "disabled"])?.as_str() {
            "auto" => IpMethod::Auto,
            "manual" => IpMethod::Manual,
            _ => IpMethod::Disabled,
        };
        let address = match reader.opt_string("address")? {
            Some(text) => Some(Ipv4Cidr::parse(&text).map_err(|err| reader.key_error("address", err.message))?),
            None => None,
        };
        let gateway = match reader.opt_string("gateway")? {
            Some(text) => Some(net::parse_ipv4(&text).map_err(|err| reader.key_error("gateway", err.message))?),
            None => None,
        };
        let mut dns = Vec::new();
        for entry in reader.string_list("dns")? {
            dns.push(net::parse_ipv4(&entry).map_err(|err| reader.key_error("dns", err.message))?);
        }
        let ignore_auto_dns = reader.bool_or("ignore_auto_dns", !dns.is_empty() && method == IpMethod::Auto)?;
        let ipv6 = match reader.enumerated("ipv6", "auto", &["auto", "disabled"])?.as_str() {
            "auto" => IpMethod::Auto,
            _ => IpMethod::Disabled,
        };

        match method {
            IpMethod::Manual if address.is_none() => {
                return Err(reader.table_error("`method` is `manual` but no `address` was given"));
            }
            IpMethod::Auto | IpMethod::Disabled if address.is_some() => {
                return Err(reader.table_error(
                    "`address` is only meaningful when `method` is `manual`",
                ));
            }
            _ => {}
        }
        if let (Some(cidr), Some(gw)) = (address, gateway) {
            if !cidr.contains(gw) {
                ctx.warn(format!(
                    "`{}`: gateway {gw} is outside {cidr}; NetworkManager will need an on-link route",
                    reader.path()
                ));
            }
        }
        if gateway.is_some() && method != IpMethod::Manual {
            return Err(reader.table_error("`gateway` is only meaningful when `method` is `manual`"));
        }

        Ok(Self { method, address, gateway, dns, ignore_auto_dns, ipv6 })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EthernetConnection {
    pub id: String,
    pub interface: String,
    pub ip: IpConfig,
    pub autoconnect: bool,
    pub autoconnect_priority: i64,
    pub mac: Option<MacAddr>,
}

impl EthernetConnection {
    fn read(reader: &mut Reader<'_>, ctx: &mut Context<'_>) -> Result<Self> {
        let id = reader.req_string("id")?;
        validate_connection_id(&id).map_err(|err| reader.key_error("id", err.message))?;
        let interface = reader.string_or("interface", "eth0")?;
        net::validate_interface(&interface).map_err(|err| reader.key_error("interface", err.message))?;
        let autoconnect = reader.bool_or("autoconnect", true)?;
        let autoconnect_priority = reader.integer_in_range("autoconnect_priority", 0, -999, 999)?;
        let mac = match reader.opt_string("mac")? {
            Some(text) => Some(MacAddr::parse(&text).map_err(|err| reader.key_error("mac", err.message))?),
            None => None,
        };
        let ip = IpConfig::read(reader, ctx, "auto")?;
        reader.finish()?;
        Ok(Self { id, interface, ip, autoconnect, autoconnect_priority, mac })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiSecurity {
    /// WPA2 personal.
    WpaPsk,
    /// WPA3 personal.
    Sae,
    /// Open network.
    Open,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiConnection {
    pub id: String,
    pub ssid: String,
    pub interface: String,
    pub security: WifiSecurity,
    pub psk: Option<String>,
    pub hidden: bool,
    pub ip: IpConfig,
    pub autoconnect: bool,
    pub autoconnect_priority: i64,
}

impl WifiConnection {
    fn read(reader: &mut Reader<'_>, ctx: &mut Context<'_>) -> Result<Self> {
        let id = reader.req_string("id")?;
        validate_connection_id(&id).map_err(|err| reader.key_error("id", err.message))?;
        let ssid = reader.req_string("ssid")?;
        validate_ssid(&ssid).map_err(|err| reader.key_error("ssid", err.message))?;
        let interface = reader.string_or("interface", "wlan0")?;
        net::validate_interface(&interface).map_err(|err| reader.key_error("interface", err.message))?;
        let security = match reader.enumerated("security", "wpa-psk", &["wpa-psk", "sae", "open"])?.as_str() {
            "wpa-psk" => WifiSecurity::WpaPsk,
            "sae" => WifiSecurity::Sae,
            _ => WifiSecurity::Open,
        };
        let psk = match read_secret(reader, "psk")? {
            Some(source) => Some(ctx.secret(reader, "psk", &source)?),
            None => None,
        };
        match (security, &psk) {
            (WifiSecurity::Open, Some(_)) => {
                return Err(reader.key_error("psk", "must not be set for an open network"))
            }
            (WifiSecurity::WpaPsk | WifiSecurity::Sae, None) => {
                return Err(reader.key_error("psk", "is required for a protected network"))
            }
            _ => {}
        }
        if let Some(value) = &psk {
            validate_psk(value).map_err(|err| reader.key_error("psk", err.message))?;
        }
        let hidden = reader.bool_or("hidden", false)?;
        let autoconnect = reader.bool_or("autoconnect", true)?;
        let autoconnect_priority = reader.integer_in_range("autoconnect_priority", 0, -999, 999)?;
        let ip = IpConfig::read(reader, ctx, "auto")?;
        reader.finish()?;
        Ok(Self { id, ssid, interface, security, psk, hidden, ip, autoconnect, autoconnect_priority })
    }
}

pub fn validate_connection_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 64 {
        return Err(Error::new("must be between 1 and 64 characters"));
    }
    if !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.') {
        return Err(Error::new(
            "may only contain ASCII letters, digits, `-`, `_` and `.` (it becomes a file name)",
        ));
    }
    Ok(())
}

pub fn validate_ssid(ssid: &str) -> Result<()> {
    if ssid.is_empty() || ssid.len() > 32 {
        return Err(Error::new("must be between 1 and 32 bytes"));
    }
    if ssid.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err(Error::new("must not contain control characters"));
    }
    Ok(())
}

pub fn validate_psk(psk: &str) -> Result<()> {
    // Either an 8-63 character passphrase or a 64 hex digit pre-shared key.
    if psk.len() == 64 && psk.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(());
    }
    if psk.len() < 8 || psk.len() > 63 {
        return Err(Error::new(
            "must be an 8-63 character passphrase or a 64 hex digit pre-shared key",
        ));
    }
    if psk.bytes().any(|b| !(0x20..=0x7e).contains(&b)) {
        return Err(Error::new("must contain only printable ASCII characters"));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GadgetFunction {
    /// CDC ECM: works with Linux and macOS hosts.
    Ecm,
    /// CDC NCM: higher throughput, Linux, macOS and recent Windows hosts.
    Ncm,
    /// RNDIS: needed by older Windows hosts.
    Rndis,
}

impl GadgetFunction {
    /// configfs function directory name.
    pub fn configfs_name(self) -> &'static str {
        match self {
            GadgetFunction::Ecm => "ecm",
            GadgetFunction::Ncm => "ncm",
            GadgetFunction::Rndis => "rndis",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            GadgetFunction::Ecm => "ecm",
            GadgetFunction::Ncm => "ncm",
            GadgetFunction::Rndis => "rndis",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbGadget {
    pub function: GadgetFunction,
    pub interface: String,
    pub address: Ipv4Cidr,
    pub peer_address: Option<Ipv4Addr>,
    pub device_mac: MacAddr,
    pub host_mac: MacAddr,
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer: String,
    pub product: String,
    pub serial: String,
}

impl UsbGadget {
    fn read(reader: &mut Reader<'_>, ctx: &mut Context<'_>, hostname: &str) -> Result<Option<Self>> {
        if !reader.bool_or("enabled", false)? {
            reader.finish()?;
            return Ok(None);
        }
        let function = match reader.enumerated("function", "ecm", &["ecm", "ncm", "rndis"])?.as_str() {
            "ecm" => GadgetFunction::Ecm,
            "ncm" => GadgetFunction::Ncm,
            _ => GadgetFunction::Rndis,
        };
        let interface = reader.string_or("interface", "usb0")?;
        net::validate_interface(&interface).map_err(|err| reader.key_error("interface", err.message))?;

        let address_text = reader.string_or("address", "10.55.0.1/24")?;
        let address = Ipv4Cidr::parse(&address_text).map_err(|err| reader.key_error("address", err.message))?;
        let peer_address = match reader.opt_string("peer_address")? {
            Some(text) => {
                let peer = net::parse_ipv4(&text).map_err(|err| reader.key_error("peer_address", err.message))?;
                if !address.contains(peer) {
                    return Err(reader.key_error("peer_address", format!("is outside {address}")));
                }
                if peer == address.address {
                    return Err(reader.key_error("peer_address", "must differ from `address`"));
                }
                Some(peer)
            }
            None => None,
        };

        let seed_base = format!("{hostname}/{interface}");
        let device_mac = match reader.opt_string("device_mac")? {
            Some(text) => MacAddr::parse(&text).map_err(|err| reader.key_error("device_mac", err.message))?,
            None => MacAddr::derive(format!("{seed_base}/device").as_bytes()),
        };
        let host_mac = match reader.opt_string("host_mac")? {
            Some(text) => MacAddr::parse(&text).map_err(|err| reader.key_error("host_mac", err.message))?,
            None => MacAddr::derive(format!("{seed_base}/host").as_bytes()),
        };
        if device_mac == host_mac {
            return Err(reader.table_error("`device_mac` and `host_mac` must differ"));
        }
        for (key, mac) in [("device_mac", device_mac), ("host_mac", host_mac)] {
            if mac.is_multicast() {
                return Err(reader.key_error(key, "must not be a multicast address"));
            }
            if !mac.is_locally_administered() {
                ctx.warn(format!(
                    "`{}.{key}`: {mac} is not locally administered; prefer an address with the \
                     second least significant bit of the first octet set",
                    reader.path()
                ));
            }
        }

        // Defaults are the Linux Foundation identifiers used by the kernel's
        // own multifunction composite gadget.
        let vendor_id = reader.integer_in_range("vendor_id", 0x1d6b, 0, 0xffff)? as u16;
        let product_id = reader.integer_in_range("product_id", 0x0104, 0, 0xffff)? as u16;
        let manufacturer = reader.string_or("manufacturer", "Raspberry Pi")?;
        let product = reader.string_or("product", "rpi-provision USB gadget")?;
        let serial = reader.string_or("serial", hostname)?;
        for (key, value) in [("manufacturer", &manufacturer), ("product", &product), ("serial", &serial)] {
            if value.is_empty() || value.bytes().any(|b| !(0x20..=0x7e).contains(&b)) {
                return Err(reader.key_error(key, "must be non-empty printable ASCII"));
            }
        }

        reader.finish()?;
        Ok(Some(Self {
            function,
            interface,
            address,
            peer_address,
            device_mac,
            host_mac,
            vendor_id,
            product_id,
            manufacturer,
            product,
            serial,
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub wifi_country: Option<String>,
    pub ethernet: Vec<EthernetConnection>,
    pub wifi: Vec<WifiConnection>,
    pub usb_gadget: Option<UsbGadget>,
}

impl Network {
    fn read_with_hostname(
        reader: &mut Reader<'_>,
        ctx: &mut Context<'_>,
        hostname: &str,
    ) -> Result<Self> {
        let wifi_country = reader.opt_string("wifi_country")?;
        if let Some(country) = &wifi_country {
            if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_uppercase()) {
                return Err(reader.key_error(
                    "wifi_country",
                    "must be a two letter uppercase ISO 3166-1 alpha-2 code, e.g. `JP`",
                ));
            }
        }

        let mut ethernet = Vec::new();
        for mut entry in reader.table_list("ethernet")? {
            ethernet.push(EthernetConnection::read(&mut entry, ctx)?);
        }
        let mut wifi = Vec::new();
        for mut entry in reader.table_list("wifi")? {
            wifi.push(WifiConnection::read(&mut entry, ctx)?);
        }
        if !wifi.is_empty() && wifi_country.is_none() {
            return Err(reader.table_error(
                "`wifi_country` is required when Wi-Fi connections are configured; the radio stays \
                 blocked until a regulatory domain is set",
            ));
        }

        let usb_gadget = UsbGadget::read(&mut reader.table("usb_gadget")?, ctx, hostname)?;
        reader.finish()?;
        Ok(Self { wifi_country, ethernet, wifi, usb_gadget })
    }
}

// ------------------------------------------------------------------ hardware

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uart {
    /// `dtparam=uart0=on`: PL011 UART0 on GPIO 14/15, exposed as `/dev/ttyAMA0`.
    pub enabled: bool,
    /// Attach a kernel console to `/dev/ttyAMA0`.
    pub console: bool,
    pub baudrate: u32,
    /// `enable_uart=1`: the dedicated debug connector, `/dev/ttyAMA10`.
    pub debug_connector: bool,
}

impl Uart {
    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let enabled = reader.bool_or("enabled", false)?;
        let console = reader.bool_or("console", false)?;
        let baudrate = reader.integer_in_range("baudrate", 115_200, 300, 4_000_000)? as u32;
        let debug_connector = reader.bool_or("debug_connector", false)?;
        if console && !enabled {
            return Err(reader.table_error(
                "`console` requires `enabled`; a console on GPIO 14/15 needs UART0 turned on",
            ));
        }
        reader.finish()?;
        Ok(Self { enabled, console, baudrate, debug_connector })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2c {
    pub enabled: bool,
    pub baudrate: u32,
}

impl I2c {
    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let enabled = reader.bool_or("enabled", false)?;
        let baudrate = reader.integer_in_range("baudrate", 100_000, 10_000, 1_000_000)? as u32;
        reader.finish()?;
        Ok(Self { enabled, baudrate })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spi {
    pub enabled: bool,
}

impl Spi {
    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let enabled = reader.bool_or("enabled", false)?;
        reader.finish()?;
        Ok(Self { enabled })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OneWire {
    pub enabled: bool,
    pub gpio: u8,
}

impl OneWire {
    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let enabled = reader.bool_or("enabled", false)?;
        let gpio = reader.integer_in_range("gpio", 4, 0, 27)? as u8;
        reader.finish()?;
        Ok(Self { enabled, gpio })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hardware {
    pub uart: Uart,
    pub i2c: I2c,
    pub spi: Spi,
    pub one_wire: OneWire,
    pub pcie_gen: Option<u8>,
    pub overlays: Vec<String>,
    pub dtparams: Vec<String>,
    pub config_extra: Vec<String>,
    pub cmdline_append: Vec<String>,
    pub cmdline_remove: Vec<String>,
}

impl Hardware {
    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let uart = Uart::read(&mut reader.table("uart")?)?;
        let i2c = I2c::read(&mut reader.table("i2c")?)?;
        let spi = Spi::read(&mut reader.table("spi")?)?;
        let one_wire = OneWire::read(&mut reader.table("one_wire")?)?;
        let pcie_gen = match reader.opt_integer("pcie_gen")? {
            Some(gen) if (1..=3).contains(&gen) => Some(gen as u8),
            Some(_) => return Err(reader.key_error("pcie_gen", "must be 1, 2 or 3")),
            None => None,
        };
        let overlays = reader.string_list("overlays")?;
        let dtparams = reader.string_list("dtparams")?;
        let config_extra = reader.string_list("config_extra")?;
        let cmdline_append = reader.string_list("cmdline_append")?;
        let cmdline_remove = reader.string_list("cmdline_remove")?;

        for (key, values) in [
            ("overlays", &overlays),
            ("dtparams", &dtparams),
            ("config_extra", &config_extra),
        ] {
            for value in values {
                if value.contains('\n') || value.contains('\r') {
                    return Err(reader.key_error(key, "entries must be single lines"));
                }
            }
        }
        for (key, values) in [("cmdline_append", &cmdline_append), ("cmdline_remove", &cmdline_remove)] {
            for value in values {
                if value.is_empty() || value.bytes().any(|b| b.is_ascii_whitespace()) {
                    return Err(reader.key_error(key, "entries must be single whitespace-free tokens"));
                }
            }
        }

        reader.finish()?;
        Ok(Self {
            uart,
            i2c,
            spi,
            one_wire,
            pcie_gen,
            overlays,
            dtparams,
            config_extra,
            cmdline_append,
            cmdline_remove,
        })
    }
}

// -------------------------------------------------------------- provisioning

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provisioning {
    /// Mount point of the FAT boot partition as seen by the running system.
    pub boot_mount: String,
    /// Directory below the boot partition holding the generated payload.
    pub runner_dir: String,
    /// Delete the payload once the first boot has completed.
    pub wipe_payload: bool,
    /// Reboot after a successful run.
    pub reboot_after: bool,
    pub log_path: String,
}

impl Provisioning {
    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let boot_mount = reader.string_or("boot_mount", "/boot/firmware")?;
        if !boot_mount.starts_with('/') || boot_mount.ends_with('/') {
            return Err(reader.key_error("boot_mount", "must be an absolute path without a trailing slash"));
        }
        let runner_dir = reader.string_or("runner_dir", "rpi-provision")?;
        if runner_dir.is_empty()
            || !runner_dir.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(reader.key_error(
                "runner_dir",
                "must be a single path segment of letters, digits, `-` or `_`",
            ));
        }
        let wipe_payload = reader.bool_or("wipe_payload", true)?;
        let reboot_after = reader.bool_or("reboot_after", true)?;
        let log_path = reader.string_or("log_path", "/var/log/rpi-provision.log")?;
        if !log_path.starts_with('/') {
            return Err(reader.key_error("log_path", "must be an absolute path"));
        }
        reader.finish()?;
        Ok(Self { boot_mount, runner_dir, wipe_payload, reboot_after, log_path })
    }
}
