use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("no peer")]
    NoPeer,
    #[error("IoError")]
    IoError(#[from] std::io::Error),
    #[error("Other {0}")]
    Other(String),
    #[error("Need Data")]
    NeedData,
    #[error("Serialize")]
    Serialize,
    #[error("Closed")]
    Closed,
    #[error("ReadEOF")]
    ReadEOF,
}

pub type Result<T> = core::result::Result<T, ProxyError>;
