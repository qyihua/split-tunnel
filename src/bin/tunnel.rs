use clap::{Parser, ValueHint};
use split_tunnel::{error::Result, http, service, tunnel};
use tokio::net::TcpListener;

#[derive(Parser, Debug)]
#[clap(name = "网络加速")]
#[clap(author = "QLTK <qltkyf@gmail.com>")]
#[clap(version = "0.1")]
#[clap(about = "将一个TCP流量平均分到多个TCP同时传输, 进行网络加速")]
struct Cli {
    #[clap(next_line_help(true))]

    /// 使用服务器模式, 默认客户端模式
    #[clap(short, long)]
    server_mode: bool,

    /// 滚动显示网速
    #[clap(short('S'), long)]
    scroll_speed: bool,

    /// 服务器模式时监听的 websocket 路径, 可作为密码
    #[clap(short, long)]
    #[clap(default_value("/multi"))]
    #[clap(conflicts_with("url"))]
    path: String,

    /// 监听地址
    #[clap(short, long)]
    #[clap(default_value("127.0.0.1:1081"))]
    #[clap(default_value_if("server-mode", None, Some("127.0.0.1:3000")))]
    #[clap(help("监听地址\n服务器模式默认监听: 127.0.0.1:3000\n客户端模式默认"))]
    listent: String,

    /// 客户端模式时设置的服务器地址
    #[clap(short, long)]
    #[clap(default_value_if("server-mode", None, Some("")))]
    #[clap(value_hint(ValueHint::Url))]
    #[clap(help("客户端模式时设置的服务器地址\n示例: https://www.example.com/multi"))]
    #[clap(conflicts_with("server-mode"))]
    url: String,

    /// 客户端加速使用的并发连接数
    #[clap(short, long)]
    #[clap(default_value_t = 2)]
    #[clap(conflicts_with("server-mode"))]
    multi: u8,

    /// 连接到服务器时, 映射地址的出口地址
    #[clap(short, long)]
    #[clap(default_value_if("server-mode", None, Some("")))]
    #[clap(value_hint(ValueHint::Hostname))]
    #[clap(help("到达服务器后, 映射的出口地址\n示例: 127.0.0.1:22 或 127.0.0.1:1080"))]
    #[clap(conflicts_with("server-mode"))]
    to: String,

    /// 显示客户端每个隧道建立的进度
    #[clap(short('P'), long)]
    #[clap(conflicts_with("server-mode"))]
    show_progress: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.server_mode {
        println!(
            "服务器模式, 对外网址: \n\thttp://{}{}\n",
            cli.listent, cli.path
        );
        tunnel::show_speed(cli.scroll_speed);
        http::http_srv(cli.listent, cli.path).await?;
        return Ok(());
    }

    println!(
        "客户端模式:\n\t加速连接: {}\n\t本地: {}\n\t服务器地址: {}",
        cli.multi, cli.listent, cli.url
    );
    println!("\t服务器出口: {}\n", cli.to);

    let url = format!("{}?to={}", cli.url, cli.to);

    tunnel::show_speed(cli.scroll_speed);

    let listener = TcpListener::bind(cli.listent).await?;
    loop {
        let (socket, _) = listener.accept().await?;
        let mut c = service::Client::new(
            http::client_upgrade_request,
            cli.multi,
            url.clone(),
            cli.show_progress,
        );
        c.set_local(socket);
        c.run();
    }
}
