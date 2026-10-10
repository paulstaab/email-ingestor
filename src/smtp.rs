//! Streaming SMTP submission. Only finish() may emit the DATA terminator.
use crate::{
    config::Config,
    envelope::{Framer, address},
    error::{Error, Result},
    wire::Wire,
};
use base64::{Engine, engine::general_purpose::STANDARD};

pub struct Smtp {
    wire: Wire,
    eight_bit: bool,
    utf8: bool,
    size: Option<u64>,
    framer: Option<Framer>,
    output: Vec<u8>,
}
impl Smtp {
    pub fn from_wire(wire: Wire) -> Self {
        Self {
            wire,
            eight_bit: false,
            utf8: false,
            size: None,
            framer: None,
            output: Vec::new(),
        }
    }
    pub async fn connect(config: &Config) -> Result<Self> {
        let mut this = Self::from_wire(Wire::connect(&config.smtp, 60).await?);
        this.setup(config).await?;
        Ok(this)
    }
    async fn reply(&mut self, expected: u16, seconds: u64) -> Result<Vec<String>> {
        let mut lines = Vec::new();
        let mut code = None;
        loop {
            let line = self.wire.line(seconds).await?;
            let bytes = line.as_bytes();
            if bytes.len() < 4
                || !bytes[..3].iter().all(u8::is_ascii_digit)
                || !matches!(bytes[3], b' ' | b'-')
            {
                return Err(Error::fatal("invalid SMTP reply"));
            }
            let current = line[..3]
                .parse::<u16>()
                .map_err(|_| Error::fatal("invalid SMTP status"))?;
            if code.is_some_and(|c| c != current) {
                return Err(Error::fatal("inconsistent SMTP multiline status"));
            }
            code = Some(current);
            if lines.len() >= 100
                || lines.iter().map(String::len).sum::<usize>() + line.len() > 64 * 1024
            {
                return Err(Error::fatal("unbounded SMTP reply"));
            }
            let last = bytes[3] == b' ';
            lines.push(line[4..].to_owned());
            if last {
                if current == expected {
                    return Ok(lines);
                }
                return Err(if (400..500).contains(&current) {
                    Error::temporary("temporary SMTP rejection")
                } else {
                    Error::fatal("permanent or unexpected SMTP reply")
                });
            }
        }
    }
    async fn command(&mut self, command: &str, code: u16) -> Result<Vec<String>> {
        self.wire
            .write(format!("{command}\r\n").as_bytes(), 300)
            .await?;
        self.reply(code, 300).await
    }
    pub async fn setup(&mut self, config: &Config) -> Result<()> {
        self.reply(220, 300).await?;
        let capabilities = self.command("EHLO email-ingestor.invalid", 250).await?;
        let mut plain = false;
        for line in capabilities.iter().skip(1) {
            let mut words = line.split_ascii_whitespace();
            match words
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase()
                .as_str()
            {
                "8BITMIME" => self.eight_bit = true,
                "SMTPUTF8" => self.utf8 = true,
                "SIZE" => {
                    if let Some(value) = words.next() {
                        self.size = Some(
                            value
                                .parse()
                                .map_err(|_| Error::fatal("invalid SMTP size capability"))?,
                        );
                    }
                }
                "AUTH" => plain |= words.any(|s| s.eq_ignore_ascii_case("PLAIN")),
                _ => {}
            }
        }
        if !plain {
            return Err(Error::fatal("SMTP AUTH PLAIN required"));
        }
        let mut auth = Vec::new();
        auth.push(0);
        auth.extend_from_slice(config.smtp.username.as_bytes());
        auth.push(0);
        auth.extend_from_slice(config.smtp.password.as_bytes());
        self.command(&format!("AUTH PLAIN {}", STANDARD.encode(auth)), 235)
            .await?;
        Ok(())
    }
    pub fn validate_headers(&self, prefix: &[u8]) -> Result<()> {
        let end = prefix
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| Error::fatal("incomplete headers"))?;
        if !self.utf8 && !prefix[..end].is_ascii() {
            return Err(Error::fatal("SMTPUTF8 required for original headers"));
        }
        Ok(())
    }
    pub async fn start(&mut self, sender: &str, recipient: &str, bytes: u64) -> Result<()> {
        if self.framer.is_some() || (!sender.is_empty() && !address(sender)) || !address(recipient)
        {
            return Err(Error::fatal("invalid SMTP envelope or transaction state"));
        }
        if self.size.is_some_and(|limit| limit != 0 && bytes > limit) {
            return Err(Error::fatal("message exceeds SMTP size capability"));
        }
        let international = !sender.is_ascii() || !recipient.is_ascii();
        if international && !self.utf8 {
            return Err(Error::fatal("SMTPUTF8 required for envelope"));
        }
        // Request SMTPUTF8 whenever supported: original header bytes may be internationalized.
        let options = format!(
            "{}{}",
            if self.eight_bit { " BODY=8BITMIME" } else { "" },
            if self.utf8 { " SMTPUTF8" } else { "" }
        );
        self.command(&format!("MAIL FROM:<{sender}>{options}"), 250)
            .await?;
        self.command(&format!("RCPT TO:<{recipient}>"), 250).await?;
        self.command("DATA", 354).await?;
        self.framer = Some(Framer::new(self.eight_bit));
        Ok(())
    }
    pub async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > crate::CHUNK {
            return Err(Error::fatal("unbounded SMTP input chunk"));
        }
        self.framer
            .as_mut()
            .ok_or_else(|| Error::fatal("SMTP DATA not started"))?
            .encode(bytes, &mut self.output)?;
        self.wire.write(&self.output, 180).await
    }
    /// Caller must establish source completeness and exact raw-byte limits first.
    pub async fn finish(&mut self) -> Result<()> {
        self.framer
            .as_ref()
            .ok_or_else(|| Error::fatal("SMTP DATA not started"))?
            .finish()?;
        self.wire.write(b".\r\n", 180).await?;
        self.reply(250, 600).await?;
        self.framer = None;
        Ok(())
    }
}
