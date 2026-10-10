use crate::{
    HEADER_LIMIT,
    error::{Error, Result},
};

/// SMTP mailbox validation, without display names, source routes or command delimiters.
pub fn address(value: &str) -> bool {
    if value.is_empty() || value.len() > 254 || value.chars().any(|c| c.is_control()) {
        return false;
    }
    let Some((local, domain)) = value.rsplit_once('@') else {
        return false;
    };
    if local.is_empty() || local.len() > 64 || domain.is_empty() {
        return false;
    }
    let valid_local = if local.starts_with('"') && local.ends_with('"') && local.len() >= 2 {
        let mut escaped = false;
        let mut valid = true;
        for c in local[1..local.len() - 1].chars() {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                valid = false;
            }
        }
        valid && !escaped
    } else {
        !local.starts_with('.')
            && !local.ends_with('.')
            && !local.contains("..")
            && local
                .chars()
                .all(|c| c.is_alphanumeric() || ".!#$%&'*+-/=?^_`{|}~".contains(c))
    };
    let valid_domain = if domain.starts_with('[') && domain.ends_with(']') {
        let inner = &domain[1..domain.len() - 1];
        inner.parse::<std::net::Ipv4Addr>().is_ok()
            || inner
                .strip_prefix("IPv6:")
                .and_then(|v| v.parse::<std::net::Ipv6Addr>().ok())
                .is_some()
    } else {
        domain.split('.').all(|part| {
            !part.is_empty()
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
    };
    valid_local && valid_domain
}
pub struct EnvelopeParser {
    line: Vec<u8>,
    field: Vec<u8>,
    paths: usize,
    return_path: bool,
    sender: Option<String>,
    bytes: usize,
    done: bool,
}
impl Default for EnvelopeParser {
    fn default() -> Self {
        Self {
            line: Vec::with_capacity(1024),
            field: Vec::with_capacity(HEADER_LIMIT - 2048),
            paths: 0,
            return_path: false,
            sender: None,
            bytes: 0,
            done: false,
        }
    }
}
impl EnvelopeParser {
    pub fn feed(&mut self, data: &[u8]) -> Result<()> {
        for &byte in data {
            if self.done {
                break;
            }
            self.bytes += 1;
            if self.bytes > HEADER_LIMIT {
                return Err(Error::fatal("unsupported excessive envelope headers"));
            }
            if self.line.len() >= 1000 {
                return Err(Error::fatal("unsupported header line length"));
            }
            self.line.push(byte);
            if byte == b'\n' {
                if !self.line.ends_with(b"\r\n") {
                    return Err(Error::fatal("unsupported header line endings"));
                }
                self.line.truncate(self.line.len() - 2);
                if self.line.is_empty() {
                    self.finish_field();
                    self.done = true;
                } else if self.line[0] == b' ' || self.line[0] == b'\t' {
                    if self.return_path {
                        if self.field.len() + self.line.len() + 1 > HEADER_LIMIT - 2048 {
                            return Err(Error::fatal("unsupported excessive Return-Path"));
                        }
                        self.field.push(b' ');
                        self.field.extend_from_slice(&self.line);
                    }
                } else {
                    self.finish_field();
                    if self
                        .line
                        .get(..12)
                        .is_some_and(|v| v.eq_ignore_ascii_case(b"Return-Path:"))
                    {
                        self.field.extend_from_slice(&self.line[12..]);
                        self.paths += 1;
                        self.return_path = true;
                    }
                }
                self.line.clear();
            }
        }
        Ok(())
    }
    fn finish_field(&mut self) {
        self.return_path = false;
        if !self.field.is_empty() {
            if let Ok(s) = std::str::from_utf8(&self.field) {
                let s = s.trim();
                if s == "<>" {
                    self.sender = Some(String::new());
                } else if s.starts_with('<') && s.ends_with('>') && address(&s[1..s.len() - 1]) {
                    self.sender = Some(s[1..s.len() - 1].to_owned());
                }
            }
            self.field.clear();
        }
    }
    pub fn done(&self) -> bool {
        self.done
    }
    pub fn sender(&self) -> Result<String> {
        if !self.done {
            return Err(Error::fatal("incomplete message headers"));
        }
        Ok(if self.paths == 1 {
            self.sender.clone().unwrap_or_default()
        } else {
            String::new()
        })
    }
}

/// Incremental dot transparency. No normalization: bare LF/CR, NUL and long lines fail.
/// The state spans every chunk boundary, including the final CRLF.
pub struct Framer {
    start: bool,
    cr: bool,
    line: usize,
    bytes: u64,
    allow_8bit: bool,
}
impl Framer {
    pub fn new(allow_8bit: bool) -> Self {
        Self {
            start: true,
            cr: false,
            line: 0,
            bytes: 0,
            allow_8bit,
        }
    }
    pub fn encode(&mut self, input: &[u8], output: &mut Vec<u8>) -> Result<()> {
        output.clear();
        for &b in input {
            if b == 0 || (!self.allow_8bit && b >= 128) {
                return Err(Error::fatal("unsupported SMTP message representation"));
            }
            if self.cr {
                if b != b'\n' {
                    return Err(Error::fatal("bare CR in message"));
                }
                self.cr = false;
                self.start = true;
                self.line = 0;
            } else if b == b'\n' {
                return Err(Error::fatal("bare LF in message"));
            } else if b == b'\r' {
                self.cr = true;
            } else {
                self.line += 1;
                if self.line > 998 {
                    return Err(Error::fatal("SMTP line exceeds supported length"));
                }
                if self.start && b == b'.' {
                    output.push(b'.');
                }
                self.start = false;
            }
            output.push(b);
            self.bytes += 1;
        }
        Ok(())
    }
    pub fn finish(&self) -> Result<()> {
        if self.bytes == 0 || !self.start || self.cr {
            return Err(Error::fatal("message lacks final CRLF"));
        }
        Ok(())
    }
}
