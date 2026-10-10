use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Temporary,
    Fatal,
}
/// Only static, local reasons are retained. Peer replies and OS errors may contain secrets.
#[derive(Debug)]
pub struct Error {
    pub kind: Kind,
    pub reason: &'static str,
}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn fatal(reason: &'static str) -> Self {
        Self {
            kind: Kind::Fatal,
            reason,
        }
    }
    pub fn temporary(reason: &'static str) -> Self {
        Self {
            kind: Kind::Temporary,
            reason,
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason)
    }
}
impl std::error::Error for Error {}
