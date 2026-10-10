//! Narrow IMAP adapter. Responses are bounded; raw peer text never reaches errors/logs.
use crate::{
    BATCH, CHUNK,
    config::Config,
    error::{Error, Result},
    wire::Wire,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::time::Duration;

pub struct Imap {
    wire: Wire,
    tag: u64,
    pub generation: u32,
    pub next_uid: u32,
    pub idle: bool,
    utf8_mailboxes: bool,
}
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
/// IMAP4rev1 uses modified UTF-7 until UTF8 is enabled; IMAP4rev2 uses UTF-8.
fn mailbox(value: &str, utf8: bool) -> String {
    if utf8 {
        return quote(value);
    }
    let mut encoded = String::new();
    let mut wide = Vec::new();
    let flush = |wide: &mut Vec<u8>, encoded: &mut String| {
        if !wide.is_empty() {
            encoded.push('&');
            encoded.push_str(
                &STANDARD
                    .encode(&*wide)
                    .trim_end_matches('=')
                    .replace('/', ","),
            );
            encoded.push('-');
            wide.clear();
        }
    };
    for character in value.chars() {
        if (' '..='~').contains(&character) {
            flush(&mut wide, &mut encoded);
            if character == '&' {
                encoded.push_str("&-");
            } else {
                encoded.push(character);
            }
        } else {
            let mut units = [0u16; 2];
            for unit in character.encode_utf16(&mut units) {
                wide.extend_from_slice(&unit.to_be_bytes());
            }
        }
    }
    flush(&mut wide, &mut encoded);
    quote(&encoded)
}
fn number_code(line: &str, name: &str) -> Option<u32> {
    let upper = line.to_ascii_uppercase();
    let tail = upper.split_once(&format!("[{name} "))?.1;
    tail.split(']').next()?.parse().ok()
}
fn status(line: &str) -> Result<()> {
    let upper = line.to_ascii_uppercase();
    if upper.contains("[UNAVAILABLE]")
        || upper.contains("[INUSE]")
        || upper.contains("[OVERQUOTA]")
        || upper.contains("[SERVERBUG]")
    {
        Err(Error::temporary("temporary IMAP rejection"))
    } else {
        Err(Error::fatal("IMAP command rejected"))
    }
}
impl Imap {
    pub fn from_wire(wire: Wire) -> Self {
        Self {
            wire,
            tag: 0,
            generation: 0,
            next_uid: 0,
            idle: false,
            utf8_mailboxes: false,
        }
    }
    pub async fn connect(config: &Config) -> Result<Self> {
        let mut this = Self::from_wire(Wire::connect(&config.imap, 60).await?);
        this.setup(config).await?;
        Ok(this)
    }
    pub async fn setup(&mut self, config: &Config) -> Result<()> {
        let greeting = self.wire.line(60).await?;
        if !greeting.to_ascii_uppercase().starts_with("* OK ") {
            return Err(Error::fatal(
                "IMAP greeting/authentication mode unsupported",
            ));
        }
        let mut auth = Vec::new();
        auth.push(0);
        auth.extend_from_slice(config.imap.username.as_bytes());
        auth.push(0);
        auth.extend_from_slice(config.imap.password.as_bytes());
        let tag = self.begin("AUTHENTICATE PLAIN").await?;
        let continuation = self.wire.line(60).await?;
        if !continuation.starts_with('+') {
            if continuation.starts_with(&format!("{tag} ")) {
                status(&continuation)?;
            }
            return Err(Error::fatal("IMAP authentication rejected"));
        }
        self.wire
            .write(format!("{}\r\n", STANDARD.encode(auth)).as_bytes(), 60)
            .await?;
        self.completion(&tag).await?;
        let tag = self.begin("CAPABILITY").await?;
        let mut move_supported = false;
        loop {
            let line = self.wire.line(60).await?;
            if self.finished(&line, &tag)? {
                break;
            }
            if line.to_ascii_uppercase().starts_with("* CAPABILITY ") {
                for capability in line.split_ascii_whitespace().skip(2) {
                    move_supported |= capability.eq_ignore_ascii_case("MOVE");
                    self.idle |= capability.eq_ignore_ascii_case("IDLE");
                    self.utf8_mailboxes |= capability.eq_ignore_ascii_case("IMAP4REV2")
                        || capability.eq_ignore_ascii_case("UTF8=ONLY");
                }
            } else {
                self.unsolicited(&line)?;
            }
        }
        if !move_supported {
            return Err(Error::fatal("native IMAP MOVE required"));
        }
        self.select(&config.destination).await?;
        self.select(&config.source).await?;
        Ok(())
    }
    fn mailbox(&self, value: &str) -> String {
        mailbox(value, self.utf8_mailboxes)
    }
    async fn begin(&mut self, command: &str) -> Result<String> {
        self.tag = self
            .tag
            .checked_add(1)
            .ok_or_else(|| Error::fatal("IMAP tag exhausted"))?;
        let tag = format!("I{}", self.tag);
        self.wire
            .write(format!("{tag} {command}\r\n").as_bytes(), 60)
            .await?;
        Ok(tag)
    }
    fn finished(&self, line: &str, tag: &str) -> Result<bool> {
        if let Some(tail) = line.strip_prefix(&format!("{tag} ")) {
            if tail
                .split_ascii_whitespace()
                .next()
                .is_some_and(|s| s.eq_ignore_ascii_case("OK"))
            {
                return Ok(true);
            }
            status(line)?;
        }
        Ok(false)
    }
    fn unsolicited(&self, line: &str) -> Result<()> {
        if line.to_ascii_uppercase().starts_with("* BYE") {
            return Err(Error::temporary("IMAP server closed session"));
        }
        // A literal outside the requested FETCH would leave the stream desynchronized.
        if line.ends_with('}') || !line.starts_with("* ") {
            return Err(Error::fatal("unexpected IMAP response"));
        }
        if let Some(generation) = number_code(line, "UIDVALIDITY")
            && self.generation != 0
            && generation != self.generation
        {
            return Err(Error::temporary("IMAP generation changed during operation"));
        }
        Ok(())
    }
    async fn completion(&mut self, tag: &str) -> Result<()> {
        loop {
            let line = self.wire.line(60).await?;
            if self.finished(&line, tag)? {
                return Ok(());
            }
            self.unsolicited(&line)?;
        }
    }
    pub async fn select(&mut self, mailbox: &str) -> Result<()> {
        let tag = self
            .begin(&format!("SELECT {}", self.mailbox(mailbox)))
            .await?;
        let mut generation = None;
        let mut next = None;
        let mut writable = false;
        loop {
            let line = self.wire.line(60).await?;
            generation = number_code(&line, "UIDVALIDITY").or(generation);
            next = number_code(&line, "UIDNEXT").or(next);
            let upper = line.to_ascii_uppercase();
            if upper.contains("[READ-ONLY]") {
                return Err(Error::fatal("IMAP mailbox is read-only"));
            }
            writable |= upper.contains("[READ-WRITE]");
            if self.finished(&line, &tag)? {
                break;
            }
            // SELECT deliberately changes the current generation.
            if !line.starts_with("* ") || line.ends_with('}') {
                return Err(Error::fatal("invalid SELECT response"));
            }
        }
        self.generation = generation
            .filter(|v| *v != 0)
            .ok_or_else(|| Error::fatal("missing UIDVALIDITY"))?;
        self.next_uid = next
            .filter(|v| *v != 0)
            .ok_or_else(|| Error::fatal("missing UIDNEXT"))?;
        if !writable {
            return Err(Error::fatal("mailbox writability not confirmed"));
        }
        Ok(())
    }
    /// A numeric UID window bounds SEARCH replies independently of mailbox size.
    pub async fn search(&mut self, first: u32, last: u32) -> Result<Vec<u32>> {
        if first == 0 || last < first || last - first >= BATCH {
            return Err(Error::fatal("invalid UID discovery window"));
        }
        let tag = self
            .begin(&format!("UID SEARCH UID {first}:{last}"))
            .await?;
        let mut values = Vec::new();
        let mut seen = false;
        loop {
            let line = self.wire.line(60).await?;
            if self.finished(&line, &tag)? {
                break;
            }
            let upper = line.to_ascii_uppercase();
            if upper == "* SEARCH" || upper.starts_with("* SEARCH ") {
                if seen {
                    return Err(Error::fatal("duplicate IMAP search response"));
                }
                seen = true;
                for word in line.split_ascii_whitespace().skip(2) {
                    let uid = word
                        .parse::<u32>()
                        .map_err(|_| Error::fatal("invalid search UID"))?;
                    if uid < first
                        || uid > last
                        || values.len() >= BATCH as usize
                        || values.contains(&uid)
                    {
                        return Err(Error::fatal("invalid or unbounded UID search"));
                    }
                    values.push(uid);
                }
            } else {
                self.unsolicited(&line)?;
            }
        }
        if !seen {
            return Err(Error::fatal("missing IMAP search response"));
        }
        values.sort_unstable();
        Ok(values)
    }
    pub async fn present(&mut self, uid: u32) -> Result<bool> {
        Ok(self.search(uid, uid).await?.contains(&uid))
    }
    pub async fn size(&mut self, uid: u32) -> Result<u64> {
        let tag = self
            .begin(&format!("UID FETCH {uid} (UID RFC822.SIZE)"))
            .await?;
        let mut size = None;
        loop {
            let line = self.wire.line(60).await?;
            if self.finished(&line, &tag)? {
                break;
            }
            let upper = line.to_ascii_uppercase();
            if upper.contains(" FETCH (") && upper.contains("RFC822.SIZE") {
                let tokens: Vec<_> = upper
                    .split(|c: char| c.is_ascii_whitespace() || c == '(' || c == ')')
                    .filter(|s| !s.is_empty())
                    .collect();
                let value = |name: &str| {
                    tokens
                        .iter()
                        .position(|s| *s == name)
                        .and_then(|p| tokens.get(p + 1))
                        .and_then(|s| s.parse::<u64>().ok())
                };
                if value("UID") != Some(u64::from(uid)) || size.is_some() {
                    return Err(Error::fatal("wrong or repeated FETCH identity"));
                }
                size = value("RFC822.SIZE");
                if size.is_none() {
                    return Err(Error::fatal("missing RFC822.SIZE"));
                }
            } else {
                self.unsolicited(&line)?;
            }
        }
        size.ok_or_else(|| Error::fatal("source disappeared before acceptance"))
    }
    pub async fn chunk(&mut self, uid: u32, offset: u64) -> Result<Vec<u8>> {
        let tag = self
            .begin(&format!(
                "UID FETCH {uid} (UID BODY.PEEK[]<{offset}.{CHUNK}>)"
            ))
            .await?;
        let mut data = None;
        loop {
            let line = self.wire.line(60).await?;
            if self.finished(&line, &tag)? {
                break;
            }
            let upper = line.to_ascii_uppercase();
            if upper.contains(" FETCH (") && upper.contains("BODY[]") {
                if data.is_some() {
                    return Err(Error::fatal("repeated body FETCH"));
                }
                let (prefix, literal) = upper
                    .rsplit_once(" {")
                    .ok_or_else(|| Error::fatal("missing bounded body literal"))?;
                let length = literal
                    .strip_suffix('}')
                    .and_then(|s| s.parse::<usize>().ok())
                    .filter(|n| *n <= CHUNK)
                    .ok_or_else(|| Error::fatal("invalid or oversized body literal"))?;
                let body = format!("BODY[]<{offset}>");
                let tokens: Vec<_> = prefix
                    .split(|c: char| c.is_ascii_whitespace() || c == '(' || c == ')')
                    .filter(|s| !s.is_empty())
                    .collect();
                let uid_pos = tokens.iter().position(|s| *s == "UID");
                let actual_uid = uid_pos
                    .and_then(|p| tokens.get(p + 1))
                    .and_then(|s| s.parse::<u32>().ok());
                if actual_uid.is_some_and(|value| value != uid) || !tokens.contains(&body.as_str())
                {
                    return Err(Error::fatal("body identity or partial offset mismatch"));
                }
                let bytes = self.wire.bytes(length, 60).await?;
                let suffix = self.wire.line(60).await?;
                let suffix_uid = suffix
                    .strip_prefix(" UID ")
                    .and_then(|s| s.strip_suffix(')'))
                    .and_then(|s| s.parse::<u32>().ok());
                if !((suffix == ")" && actual_uid == Some(uid))
                    || (actual_uid.is_none() && suffix_uid == Some(uid)))
                {
                    return Err(Error::fatal(
                        "unsupported body FETCH suffix or UID mismatch",
                    ));
                }
                data = Some(bytes);
            } else {
                self.unsolicited(&line)?;
            }
        }
        data.ok_or_else(|| Error::fatal("source disappeared during retrieval"))
    }
    pub async fn move_uid(&mut self, uid: u32, destination: &str) -> Result<()> {
        let tag = self
            .begin(&format!("UID MOVE {uid} {}", self.mailbox(destination)))
            .await?;
        self.completion(&tag).await
    }
    /// Cancellation here always disposes of this session. No subsequent command may race DONE.
    pub async fn wait(&mut self, duration: Duration) -> Result<()> {
        if !self.idle {
            tokio::time::sleep(duration).await;
            let tag = self.begin("NOOP").await?;
            return self.completion(&tag).await;
        }
        let tag = self.begin("IDLE").await?;
        loop {
            let line = self.wire.line(60).await?;
            if line.starts_with('+') {
                break;
            }
            if line.starts_with(&format!("{tag} NO")) || line.starts_with(&format!("{tag} BAD")) {
                self.idle = false;
                tracing::info!(discovery = "poll", event = "idle_unavailable");
                return Ok(());
            }
            self.unsolicited(&line)?;
        }
        tokio::select! {
            biased;
            result = self.wire.line(1501) => { self.unsolicited(&result?)?; }
            _ = tokio::time::sleep(duration.min(Duration::from_secs(1500))) => {}
        }
        self.wire.write(b"DONE\r\n", 60).await?;
        self.completion(&tag).await
    }
}

#[cfg(test)]
mod tests {
    use super::mailbox;
    #[test]
    fn international_folder_names_follow_negotiated_protocol() {
        assert_eq!(
            mailbox("~peter/mail/台北/日本語", false),
            "\"~peter/mail/&U,BTFw-/&ZeVnLIqe-\""
        );
        assert_eq!(mailbox("Trash & Archive", false), "\"Trash &- Archive\"");
        assert_eq!(mailbox("台北", true), "\"台北\"");
    }
}
