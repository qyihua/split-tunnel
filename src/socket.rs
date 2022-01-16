use crate::error::Result;
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::io::{ReadHalf, WriteHalf};

pub trait Socket
where
    Self: AsyncRead + AsyncWrite + Send + Unpin,
{
    type ReadHalf: AsyncRead + Unpin + Send;
    type WriteHalf: AsyncWrite + Unpin + Send;

    fn split_read_write(self) -> Result<(Self::ReadHalf, Self::WriteHalf)>;
}

#[async_trait]
impl<T> Socket for T
where
    T: AsyncRead + AsyncWrite + Send + Unpin,
{
    type ReadHalf = ReadHalf<T>;
    type WriteHalf = WriteHalf<T>;

    fn split_read_write(self) -> Result<(Self::ReadHalf, Self::WriteHalf)> {
        Ok(tokio::io::split(self))
    }
}
