use crate::{
    config::Endpoint,
    error::{Error, Result},
};
use std::{io, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::TcpStream,
    time::timeout,
};
use tokio_rustls::TlsConnector;

pub trait Transport: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Transport for T {}
pub struct Wire {
    io: BufReader<Box<dyn Transport>>,
    pending_line: Vec<u8>,
}
impl Wire {
    pub fn new<T: Transport + 'static>(io: T) -> Self {
        Self {
            io: BufReader::with_capacity(16 * 1024, Box::new(io)),
            pending_line: Vec::new(),
        }
    }
    pub async fn connect(endpoint: &Endpoint, seconds: u64) -> Result<Self> {
        let tcp = timeout(
            Duration::from_secs(seconds),
            TcpStream::connect((endpoint.host.as_str(), endpoint.port)),
        )
        .await
        .map_err(|_| Error::temporary("connection timeout"))?
        .map_err(|_| Error::temporary("TCP connection failed"))?;
        let name = rustls::pki_types::ServerName::try_from(endpoint.host.clone())
            .map_err(|_| Error::fatal("invalid TLS hostname"))?;
        let tls = timeout(
            Duration::from_secs(seconds),
            TlsConnector::from(endpoint.tls.clone()).connect(name, tcp),
        )
        .await
        .map_err(|_| Error::temporary("TLS connection timeout"))?
        .map_err(|e| {
            if matches!(
                e.kind(),
                io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput
            ) {
                Error::fatal("TLS certificate or protocol validation failed")
            } else {
                Error::temporary("TLS connection failed")
            }
        })?;
        Ok(Self::new(tls))
    }
    pub async fn line(&mut self, seconds: u64) -> Result<String> {
        loop {
            let buf = timeout(Duration::from_secs(seconds), self.io.fill_buf())
                .await
                .map_err(|_| Error::temporary("protocol read timeout"))?
                .map_err(|_| Error::temporary("protocol read failed"))?;
            if buf.is_empty() {
                return Err(Error::temporary("protocol connection closed"));
            }
            let n = buf
                .iter()
                .position(|b| *b == b'\n')
                .map_or(buf.len(), |n| n + 1);
            if self.pending_line.len() + n > 64 * 1024 {
                return Err(Error::fatal("protocol line exceeds limit"));
            }
            self.pending_line.extend_from_slice(&buf[..n]);
            self.io.consume(n);
            if self.pending_line.ends_with(b"\n") {
                break;
            }
        }
        if !self.pending_line.ends_with(b"\r\n") {
            return Err(Error::fatal("invalid protocol line ending"));
        }
        let mut line = std::mem::take(&mut self.pending_line);
        line.truncate(line.len() - 2);
        String::from_utf8(line).map_err(|_| Error::fatal("invalid protocol text"))
    }
    pub async fn bytes(&mut self, length: usize, seconds: u64) -> Result<Vec<u8>> {
        if length > crate::CHUNK {
            return Err(Error::fatal("unbounded IMAP literal"));
        }
        let mut bytes = vec![0; length];
        let mut pos = 0;
        while pos < length {
            let n = timeout(
                Duration::from_secs(seconds),
                self.io.read(&mut bytes[pos..]),
            )
            .await
            .map_err(|_| Error::temporary("literal read timeout"))?
            .map_err(|_| Error::temporary("literal read failed"))?;
            if n == 0 {
                return Err(Error::temporary("truncated IMAP literal"));
            }
            pos += n;
        }
        Ok(bytes)
    }
    pub async fn write(&mut self, bytes: &[u8], seconds: u64) -> Result<()> {
        let mut pos = 0;
        while pos < bytes.len() {
            let n = timeout(
                Duration::from_secs(seconds),
                self.io.get_mut().write(&bytes[pos..]),
            )
            .await
            .map_err(|_| Error::temporary("protocol write timeout"))?
            .map_err(|_| Error::temporary("protocol write failed"))?;
            if n == 0 {
                return Err(Error::temporary("protocol write stopped"));
            }
            pos += n;
        }
        timeout(Duration::from_secs(seconds), self.io.get_mut().flush())
            .await
            .map_err(|_| Error::temporary("protocol flush timeout"))?
            .map_err(|_| Error::temporary("protocol flush failed"))?;
        Ok(())
    }
}
