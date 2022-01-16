use crate::{
    error::{ProxyError, Result},
    socket::Socket,
    tunnel::Tunnel,
};
use futures::{future::try_join_all, Future, TryFutureExt};
use pnet_macros::Packet;
use pnet_macros_support::packet::{FromPacket, Packet};
use std::{
    collections::HashMap,
    io::Write,
    marker::PhantomData,
    sync::{atomic::AtomicU8, Arc},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    sync::Mutex,
};
use uuid::Uuid;

/// The command for establish a tunnel
#[derive(Packet, Debug)]
pub struct Cmd {
    pub tunnel_size: u8,

    // id is generate by uuid
    #[length = "16"]
    pub id: Vec<u8>,

    // All packets must specify a `#[payload]` in libpnet,
    // which should be a `Vec<u8>`
    #[payload]
    pub payload: Vec<u8>,
}

impl Cmd {
    pub fn new() -> Self {
        Self {
            tunnel_size: 0,
            id: vec![0; 16],
            payload: Vec::new(),
        }
    }
}

/// The total size of `CmdPacket`
const CMD_PACKET_SIZE: usize = CmdPacket::minimum_packet_size() + 16;

pub async fn new_connect(arg: String) -> Result<TcpStream> {
    Ok(TcpStream::connect(arg).await?)
}

/// The server of tunnel
pub struct Server<O, M, Fun, Arg, Ret>
where
    O: Socket + 'static,
    M: Socket + 'static,
    Ret: Future<Output = Result<O>>,
    Fun: Fn(Arg) -> Ret,
{
    tunnels: Mutex<HashMap<u128, Tunnel<O, M>>>,
    new_one_socket: Fun,
    _arg: PhantomData<Arg>,
}

impl<O, M, Fun, Arg, Ret> Server<O, M, Fun, Arg, Ret>
where
    O: Socket + 'static,
    M: Socket + 'static,
    Ret: Future<Output = Result<O>> + Send + 'static,
    Fun: Fn(Arg) -> Ret + Send + 'static,
{
    /// Make a server with a fun which create a new connection for proxy to
    pub fn new(new_one_socket: Fun) -> Arc<Self> {
        Arc::new(Self {
            tunnels: Mutex::new(HashMap::new()),
            new_one_socket,
            _arg: PhantomData,
        })
    }

    /// Accept a tunnel connection
    pub async fn accept_client(self: &Arc<Self>, mut multi_socket: M, arg: Arg) {
        let service = self.clone();
        let mut buf = Vec::new();
        loop {
            let size = multi_socket.read_buf(&mut buf).await.unwrap();
            if size == 0 {
                break;
            }
            let cmd = match CmdPacket::new(&buf) {
                Some(p) => p.from_packet(),
                None => continue,
            };
            service
                .new_tunnel_connection(cmd, multi_socket, arg)
                .await
                .unwrap();
            break;
        }
    }

    /// check the cmd packet from client
    ///
    /// if the number of connection reach the tunnel_size, the tunnel will run proxy
    async fn new_tunnel_connection(&self, cmd: Cmd, mut multi_socket: M, arg: Arg) -> Result<()> {
        let buf = vec![0u8; CMD_PACKET_SIZE];
        let mut packet = MutableCmdPacket::owned(buf).unwrap();
        packet.populate(&cmd);
        multi_socket.write_all(packet.packet()).await?;

        let id = cmd.id[..16].try_into().unwrap();
        let id = u128::from_be_bytes(id);

        let tunnel = {
            let mut tunnels = self.tunnels.lock().await;
            let tunnel = match tunnels.get_mut(&id) {
                Some(t) => t,
                None => {
                    let t = Tunnel::new(cmd.tunnel_size);
                    tunnels.insert(id, t);
                    tunnels.get_mut(&id).unwrap()
                }
            };
            tunnel.multi_socket.push(multi_socket);
            if tunnel.multi_socket.len() == tunnel.size as usize {
                tunnels.remove(&id)
            } else {
                None
            }
        };
        if let Some(mut tunnel) = tunnel {
            let one_socket = (self.new_one_socket)(arg).await?;
            tunnel.one_socket = Some(one_socket);
            if let Err(err) = tunnel.proxy().await {
                eprintln!("{:?}", err)
            }
        }
        Ok(())
    }
}

/// Record the progress for establish a tunnel in client
#[derive(Clone)]
pub struct ConnectCount {
    max: u8,
    current: Arc<AtomicU8>,
    current_ok: Arc<AtomicU8>,
    show_progress: bool,
}

impl ConnectCount {
    pub fn new(max: u8, show_progress: bool) -> Self {
        let current = Arc::new(AtomicU8::new(1));
        let current_ok = Arc::new(AtomicU8::new(1));
        Self {
            max,
            current,
            show_progress,
            current_ok,
        }
    }
}

/// The client for eatablish a tunnel
pub struct Client<M, Fun, Ret>
where
    Fun: Fn(String) -> Ret + 'static,
    Ret: Future<Output = Result<M>>,
{
    tunnel: Tunnel<TcpStream, M>,
    stream_fun: Fun,
    arg: String,
    show_progress: bool,
}

impl<M, Fun, Ret> Client<M, Fun, Ret>
where
    M: Send + Unpin + AsyncWrite + AsyncRead + 'static,
    Fun: Fn(String) -> Ret + 'static + Send + Clone,
    Ret: Future<Output = Result<M>> + 'static + Send,
{
    /// Make a client with a fun which provide a new connection for establish tunnel
    pub fn new(stream_fun: Fun, size: u8, arg: String, show_progress: bool) -> Self {
        let tunnel = Tunnel::new(size);
        Self {
            tunnel,
            stream_fun,
            arg,
            show_progress,
        }
    }

    /// Set a local socket for the tunnel
    pub fn set_local(&mut self, tcp: TcpStream) {
        self.tunnel.one_socket = Some(tcp);
    }

    pub fn run(mut self) {
        tokio::spawn(
            async move {
                let uuid = Uuid::new_v4().as_bytes().to_vec();
                let mut sockets = Vec::new();
                let size = self.tunnel.size;
                let count = ConnectCount::new(size, self.show_progress);
                for _ in 0..size {
                    sockets.push(Self::get_tunnel_stream(
                        count.clone(),
                        self.stream_fun.clone(),
                        self.arg.clone(),
                        size,
                        uuid.clone(),
                    ));
                }
                let sockets = try_join_all(sockets).await?;
                self.tunnel.multi_socket.extend(sockets);
                self.tunnel.proxy().await
            }
            .unwrap_or_else(|err| eprintln!("Error for establish a tunnel:{:?}", err)),
        );
    }

    async fn get_tunnel_stream(
        count: ConnectCount,
        tunnel_fun: Fun,
        arg: String,
        tunnel_size: u8,
        uuid: Vec<u8>,
    ) -> Result<M> {
        if count.show_progress {
            let current = count
                .current
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            print!("\rEstablishing: {}/{} ", current, count.max);
            std::io::stdout().flush().ok();
        }
        let stream = {
            let fut = tunnel_fun(arg);
            fut.await?
        };
        let stream = Self::handshake(tunnel_size, uuid, stream).await?;
        if count.show_progress {
            let current = count
                .current_ok
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            print!("\rEstabled:    {}/{} ", current, count.max);
            std::io::stdout().flush().ok();
        }

        Ok(stream)
    }

    async fn handshake(tunnel_size: u8, uuid: Vec<u8>, mut stream: M) -> Result<M> {
        let mut packet = Cmd::new();
        packet.id = uuid;
        packet.tunnel_size = tunnel_size;
        let packet_size = CmdPacket::minimum_packet_size() + 16;
        let mut buf = vec![0u8; packet_size];
        buf.shrink_to_fit();
        let mut send_packet = MutableCmdPacket::new(&mut buf).unwrap();
        send_packet.populate(&packet);
        stream.write_all(send_packet.packet()).await?;

        stream.read_exact(&mut buf).await?;

        let recv_packet = CmdPacket::new(&buf).unwrap().from_packet();
        if recv_packet.id != packet.id {
            return Err(ProxyError::Other(
                "Error for establish a tunnel".to_string(),
            ));
        }
        Ok(stream)
    }
}
