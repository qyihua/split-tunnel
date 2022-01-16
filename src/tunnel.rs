use crate::{
    error::{ProxyError, Result},
    socket::Socket,
};
use futures::future::try_join_all;
use pnet_macros::Packet;
use pnet_macros_support::{
    packet::{FromPacket, Packet},
    types::*,
};
use std::io::Write;
use std::sync::{
    atomic::{AtomicU8, AtomicUsize, Ordering},
    Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{broadcast, Mutex},
    time::{sleep, Duration},
};

/// Record inbound bytes
static INBOUND_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Record outbound bytes
static OUTBOUND_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Show network speed of all tunnels
pub fn show_speed(enable_scroll: bool) {
    tokio::spawn(async move {
        let mut out = std::io::stdout();
        const SLEEP_TIME: u64 = 5;
        loop {
            let input = INBOUND_COUNT.swap(0, Ordering::SeqCst);
            let output = OUTBOUND_COUNT.swap(0, Ordering::SeqCst);

            let input = input as f64 / (1024 * SLEEP_TIME) as f64;
            let output = output as f64 / (1024 * SLEEP_TIME) as f64;

            if enable_scroll {
                println!(
                    "                  Speed: Out: {:>6.1} KB/s, In: {:>6.1} KB/s",
                    output, input
                );
            } else {
                print!(
                    "\r                  Speed: Out: {:>6.1} KB/s, In: {:>6.1} KB/s",
                    output, input
                );
                out.flush().ok();
            }
            sleep(Duration::from_secs(SLEEP_TIME)).await;
        }
    });
}

/// The packet of a chunk
/// The `Chunk` represent a chunk data in tunnel for the communication of client and server
#[derive(Packet, Debug)]
pub struct Chunk {
    index: u8,
    data_size: u16be,
    #[payload]
    #[length = "data_size"]
    payload: Vec<u8>,
}

impl Chunk {
    pub fn new(index: u8, data_size: u16) -> Self {
        Self {
            index,
            data_size,
            payload: Vec::new(),
        }
    }
}

const CHUNK_PACKET_SIZE: usize = ChunkPacket::minimum_packet_size();
const BUF_SIZE: usize = 1500;

#[derive(Clone, Debug)]
enum SynMsg {
    Reset,
    Closed,
}

/// The lock is used to split data into chunk and regroup chunk into data in right order
struct SynLock {
    max: u8,
    count: Arc<AtomicU8>,
    receiver: broadcast::Receiver<SynMsg>,
    sender: broadcast::Sender<SynMsg>,
}

impl SynLock {
    fn new(max: u8) -> Self {
        let count = Arc::new(AtomicU8::new(0));
        let (sender, receiver) = broadcast::channel(10);
        Self {
            max,
            count,
            sender,
            receiver,
        }
    }

    fn clone(&self) -> Self {
        let max = self.max;
        let count = self.count.clone();
        let sender = self.sender.clone();
        let receiver = sender.subscribe();
        Self {
            max,
            count,
            receiver,
            sender,
        }
    }

    fn get_index(&self) -> (u8, bool) {
        let pre = self.count.fetch_add(1, Ordering::SeqCst);
        if pre == self.max - 1 {
            self.count.store(0, Ordering::SeqCst);
            return (pre, true);
        }
        (pre, false)
    }
}

/// Write a chunk data to destination socket in right order
pub struct ChunkWriter<W>
where
    W: AsyncWrite + Unpin,
{
    output: W,
    chunks: Vec<Option<Chunk>>,
    size: u8,
    current: u8,
}

impl<W> ChunkWriter<W>
where
    W: AsyncWrite + Unpin,
{
    fn new(output: W, size: u8) -> Self {
        let mut chunks = Vec::with_capacity(size as usize);
        for _ in 0..size {
            chunks.push(None);
        }
        Self {
            output,
            size,
            current: 0,
            chunks,
        }
    }

    async fn write(&mut self, chunk: Chunk) -> Result<()> {
        if chunk.index == self.current {
            self.output.write_all(&chunk.payload).await?;
            self.current += 1;
        } else {
            let _ = self
                .chunks
                .get_mut(chunk.index as usize)
                .unwrap()
                .insert(chunk);
            return Ok(());
        }
        for i in self.current..self.size {
            if let Some(p) = self.chunks[i as usize].take() {
                self.output.write_all(&p.payload).await?;
                self.current += 1;
            } else {
                break;
            }
        }
        if self.current == self.size {
            self.current = 0;
        }
        Ok(())
    }
}

/// The tunnel for split one connection into many connections
pub struct Tunnel<O, M> {
    pub size: u8,
    pub multi_socket: Vec<M>,
    pub one_socket: Option<O>,
}

impl<O, M> Tunnel<O, M>
where
    O: Socket + Unpin + Send + 'static,
    M: Socket + Unpin + Send + 'static,
{
    pub fn new(size: u8) -> Self {
        Self {
            one_socket: None,
            multi_socket: Vec::new(),
            size,
        }
    }

    pub async fn proxy(mut self) -> Result<()> {
        let max = self.size;
        let (one_read, one_write) = Option::take(&mut self.one_socket)
            .unwrap()
            .split_read_write()
            .unwrap();

        let one_socket_read = Arc::new(Mutex::new(one_read));
        let one_socket_write = Arc::new(Mutex::new(ChunkWriter::new(one_write, max)));
        let one_to_multi_lock = SynLock::new(max);
        let multi_to_one_lock = SynLock::new(max);
        let multis = std::mem::replace(&mut self.multi_socket, Vec::new());
        let mut one_to_multi_futures = Vec::new();
        let mut multi_to_one_futures = Vec::new();

        for multi_socket in multis {
            let (multi_socket_read, multi_socket_write) = multi_socket.split_read_write().unwrap();
            one_to_multi_futures.push(Self::one_to_multi(
                one_to_multi_lock.clone(),
                one_socket_read.clone(),
                multi_socket_write,
            ));
            multi_to_one_futures.push(Self::multi_to_one(
                multi_to_one_lock.clone(),
                multi_socket_read,
                one_socket_write.clone(),
            ));
        }

        let one_to_multi_futures = try_join_all(one_to_multi_futures);
        let multi_to_one_futures = try_join_all(multi_to_one_futures);
        tokio::try_join!(one_to_multi_futures, multi_to_one_futures)?;
        Ok(())
    }

    async fn one_to_multi(
        mut lock: SynLock,
        one_socket_read: Arc<Mutex<<O as Socket>::ReadHalf>>,
        mut multi_socket_write: <M as Socket>::WriteHalf,
    ) -> Result<()> {
        let mut buf = vec![0; BUF_SIZE];
        loop {
            let payload_buf = &mut buf[CHUNK_PACKET_SIZE..];
            let payload_size = match one_socket_read.lock().await.read(payload_buf).await {
                Ok(s) => s,
                Err(_err) => 0,
            };
            if payload_size == 0 {
                lock.sender
                    .send(SynMsg::Closed)
                    .map_err(|err| ProxyError::Other(err.to_string()))?;
                break;
            }

            OUTBOUND_COUNT.fetch_add(payload_size, Ordering::SeqCst);

            let (index, reset) = lock.get_index();

            let multi = Chunk::new(index, payload_size as u16);
            let mut packet =
                MutableChunkPacket::new(&mut buf[..CHUNK_PACKET_SIZE + payload_size]).unwrap();
            packet.populate(&multi);
            multi_socket_write.write_all(packet.packet()).await?;

            if reset {
                lock.sender
                    .send(SynMsg::Reset)
                    .map_err(|_| ProxyError::Closed)?;
            }
            let msg = loop {
                match lock.receiver.recv().await {
                    Ok(msg) => break msg,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(err) => return Err(ProxyError::Other(err.to_string())),
                }
            };
            match msg {
                SynMsg::Reset => continue,
                SynMsg::Closed => break,
            };
        }
        Ok(())
    }

    async fn multi_to_one(
        mut lock: SynLock,
        mut multi_socket_read: <M as Socket>::ReadHalf,
        one_socket_write: Arc<Mutex<ChunkWriter<<O as Socket>::WriteHalf>>>,
    ) -> Result<()> {
        let mut buf = vec![0; BUF_SIZE];
        loop {
            let read_size = match multi_socket_read
                .read_exact(&mut buf[0..CHUNK_PACKET_SIZE])
                .await
            {
                Ok(s) => s,
                Err(_err) => {
                    // println!(" multi read err:{:?}", err);
                    0
                }
            };
            if read_size == 0 {
                lock.sender
                    .send(SynMsg::Closed)
                    .map_err(|_| ProxyError::Closed)?;
                break;
            }
            let mut multi = match ChunkPacket::new(&buf[0..CHUNK_PACKET_SIZE]) {
                Some(p) => p.from_packet(),
                None => {
                    unreachable!();
                }
            };
            if multi.data_size != 0 {
                let size = match multi_socket_read
                    .read_exact(
                        &mut buf[CHUNK_PACKET_SIZE..CHUNK_PACKET_SIZE + multi.data_size as usize],
                    )
                    .await
                {
                    Ok(s) => s,
                    Err(_) => 0,
                };
                if size == 0 {
                    lock.sender
                        .send(SynMsg::Closed)
                        .map_err(|err| ProxyError::Other(err.to_string()))?;
                    break;
                }
                multi = match ChunkPacket::new(&buf[..CHUNK_PACKET_SIZE + size]) {
                    Some(p) => p.from_packet(),
                    None => continue,
                };
            }

            INBOUND_COUNT.fetch_add(multi.data_size as usize, Ordering::SeqCst);

            one_socket_write
                .lock()
                .await
                .write(multi)
                .await
                .map_err(|_| ProxyError::Closed)?;
            let (_, reset) = lock.get_index();
            if reset {
                lock.sender
                    .send(SynMsg::Reset)
                    .map_err(|_| ProxyError::Closed)?;
            }

            let msg = loop {
                match lock.receiver.recv().await {
                    Ok(msg) => break msg,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    _ => return Err(ProxyError::Closed),
                }
            };
            match msg {
                SynMsg::Reset => continue,
                SynMsg::Closed => break,
            };
        }
        Ok(())
    }
}
