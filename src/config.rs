use crate::{
    envelope::address,
    error::{Error, Result},
};
use rustls::{ClientConfig, RootCertStore};
use std::{collections::HashMap, fs, sync::Arc};

pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub tls: Arc<ClientConfig>,
}
pub struct Config {
    pub imap: Endpoint,
    pub smtp: Endpoint,
    pub source: String,
    pub destination: String,
    pub recipient: String,
    pub max_bytes: u64,
    pub log_level: String,
}
const KEYS: &[&str] = &[
    "IMAP_HOST",
    "IMAP_PORT",
    "IMAP_CA_FILE",
    "IMAP_MAILBOX",
    "IMAP_USERNAME",
    "IMAP_USERNAME_FILE",
    "IMAP_PASSWORD",
    "IMAP_PASSWORD_FILE",
    "SMTP_HOST",
    "SMTP_PORT",
    "SMTP_CA_FILE",
    "SMTP_USERNAME",
    "SMTP_USERNAME_FILE",
    "SMTP_PASSWORD",
    "SMTP_PASSWORD_FILE",
    "SMTP_RECIPIENT",
    "DESTINATION_FOLDER",
    "MAX_MESSAGE_BYTES",
    "LOG_LEVEL",
];
fn get<'a>(env: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    env.get(&format!("INGESTOR_{name}")).map(String::as_str)
}
fn required(env: &HashMap<String, String>, name: &str) -> Result<String> {
    get(env, name)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| Error::fatal("missing or empty required configuration"))
}
fn text(s: &str) -> Result<()> {
    if s.bytes().any(|b| b == 0 || b == b'\r' || b == b'\n') {
        return Err(Error::fatal(
            "configuration contains unsupported control characters",
        ));
    }
    Ok(())
}
fn credential(env: &HashMap<String, String>, name: &str) -> Result<String> {
    let direct = get(env, name);
    let path = get(env, &format!("{name}_FILE"));
    let value = match (direct, path) {
        (Some(v), None) => v.to_owned(),
        (None, Some(p)) if !p.is_empty() => {
            let bytes = fs::read(p).map_err(|_| Error::fatal("cannot read credential file"))?;
            let mut value = String::from_utf8(bytes)
                .map_err(|_| Error::fatal("credential file is not UTF-8"))?;
            if value.ends_with('\n') {
                value.pop();
                if value.ends_with('\r') {
                    value.pop();
                }
            }
            value
        }
        _ => {
            return Err(Error::fatal(
                "credential requires exactly one direct or file value",
            ));
        }
    };
    if value.is_empty() {
        return Err(Error::fatal("empty credential"));
    }
    text(&value)?;
    Ok(value)
}
fn tls(path: Option<&str>) -> Result<Arc<ClientConfig>> {
    let mut roots = RootCertStore::empty();
    if let Some(path) = path {
        if path.is_empty() {
            return Err(Error::fatal("empty CA file path"));
        }
        let data = fs::read(path).map_err(|_| Error::fatal("cannot read CA file"))?;
        let certs = rustls_pemfile::certs(&mut &data[..])
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| Error::fatal("invalid CA PEM"))?;
        if certs.is_empty() {
            return Err(Error::fatal("CA file contains no certificates"));
        }
        for cert in certs {
            roots
                .add(cert)
                .map_err(|_| Error::fatal("invalid CA certificate"))?;
        }
    } else {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    Ok(Arc::new(
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ))
}
fn endpoint(env: &HashMap<String, String>, prefix: &str, default: u16) -> Result<Endpoint> {
    let host = required(env, &format!("{prefix}_HOST"))?;
    text(&host)?;
    rustls::pki_types::ServerName::try_from(host.clone())
        .map_err(|_| Error::fatal("invalid TLS hostname"))?;
    let port = match get(env, &format!("{prefix}_PORT")) {
        None => default,
        Some(v) => v
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .ok_or_else(|| Error::fatal("invalid port"))?,
    };
    Ok(Endpoint {
        host,
        port,
        username: credential(env, &format!("{prefix}_USERNAME"))?,
        password: credential(env, &format!("{prefix}_PASSWORD"))?,
        tls: tls(get(env, &format!("{prefix}_CA_FILE")))?,
    })
}
impl Config {
    pub fn from_env() -> Result<Self> {
        let mut values = HashMap::new();
        for (k, v) in std::env::vars_os() {
            if k.to_string_lossy().starts_with("INGESTOR_") {
                values.insert(
                    k.into_string()
                        .map_err(|_| Error::fatal("non-UTF8 configuration name"))?,
                    v.into_string()
                        .map_err(|_| Error::fatal("non-UTF8 configuration value"))?,
                );
            }
        }
        Self::parse(values)
    }
    pub fn parse(env: HashMap<String, String>) -> Result<Self> {
        for key in env.keys().filter_map(|k| k.strip_prefix("INGESTOR_")) {
            if !KEYS.contains(&key) {
                return Err(Error::fatal("unknown INGESTOR configuration"));
            }
        }
        let source = get(&env, "IMAP_MAILBOX").unwrap_or("INBOX").to_owned();
        let destination = required(&env, "DESTINATION_FOLDER")?;
        text(&source)?;
        text(&destination)?;
        if source.is_empty()
            || source == destination
            || (source.eq_ignore_ascii_case("INBOX") && destination.eq_ignore_ascii_case("INBOX"))
        {
            return Err(Error::fatal("invalid or identical source and destination"));
        }
        let recipient = required(&env, "SMTP_RECIPIENT")?;
        if !address(&recipient) {
            return Err(Error::fatal("invalid single SMTP recipient"));
        }
        let max_bytes = get(&env, "MAX_MESSAGE_BYTES")
            .unwrap_or("262144000")
            .parse::<u64>()
            .ok()
            .filter(|v| *v != 0)
            .ok_or_else(|| Error::fatal("invalid message size limit"))?;
        let log_level = get(&env, "LOG_LEVEL").unwrap_or("info").to_owned();
        if !["error", "warn", "info", "debug", "trace"].contains(&log_level.as_str()) {
            return Err(Error::fatal("invalid log level"));
        }
        Ok(Self {
            imap: endpoint(&env, "IMAP", 993)?,
            smtp: endpoint(&env, "SMTP", 465)?,
            source,
            destination,
            recipient,
            max_bytes,
            log_level,
        })
    }
}
