# split-tunnel
将一个TCP流量平均分到多个TCP同时传输, 进行网络加速

本应用使用WebSocket创建连接，可以利用cloudflare进行加速

## 利用ssh进行多路复用
客户端：
```shell
tunnel --url "http://server:3000/multi" --to 127.0.0.1:22 --listent 127.0.0.1:2222 --multi 3 --show-progress
```
使用ssh创建socks5服务
```shell
ssh 127.0.0.1 -p2222 -D1080
```

服务器：

```shell
tunnel --server-mode --listent 127.0.0.1:3000 --path "/multi"
```

## 不使用多路复用
本程序只提供端口映射，没有提供socks5服务，需要服务器端另外安装启动socks5服务

客户端：
```shell
tunnel --url "http://server:3000/multi" --to 127.0.0.1:1080 --listent 127.0.0.1:1080 --multi 3 --show-progress
```
服务器：

```shell
tunnel --server-mode --listent 127.0.0.1:3000 --path "/multi"
```


```shell
USAGE:
tunnel [OPTIONS] --url <URL> --to <TO>

OPTIONS:
-h, --help
Print help information

-l, --listent <LISTENT>
监听地址
服务器模式默认监听: 127.0.0.1:3000
客户端模式默认 [default: 127.0.0.1:1081]

-m, --multi <MULTI>
客户端加速使用的并发连接数 [default: 2]

-p, --path <PATH>
服务器模式时监听的 websocket 路径, 可作为密码 [default: /multi]

-P, --show-progress
显示客户端每个隧道建立的进度

-s, --server-mode
使用服务器模式, 默认客户端模式

-S, --scroll-speed
滚动显示网速

-t, --to <TO>
到达服务器后, 映射的出口地址
示例: 127.0.0.1:22 或 127.0.0.1:1080

-u, --url <URL>
客户端模式时设置的服务器地址
示例: https://www.example.com/multi

-V, --version
Print version information
```
