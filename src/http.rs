use crate::{
    error::{ProxyError, Result},
    service::{self, new_connect},
};
use futures::Future;
use hyper::upgrade::Upgraded;
use hyper::{
    header::{HeaderValue, UPGRADE},
    service::{make_service_fn, service_fn},
    Body, Client, Request, Response, Server, StatusCode,
};
use hyper_tls::HttpsConnector;
use std::{collections::HashMap, sync::Arc};
use tokio::net::TcpStream;
use url::form_urlencoded;

/// Websocket Server
pub async fn http_srv(addr: String, path: String) -> Result<()> {
    let multi_service = service::Server::new(new_connect);

    let make_service = make_service_fn(move |_| {
        let multi_service = multi_service.clone();
        let path = path.clone();
        async move {
            Ok::<_, hyper::Error>(service_fn(move |req| {
                server_upgrade(req, path.clone(), multi_service.clone())
            }))
        }
    });

    let addr = addr.parse().unwrap();
    let server = Server::bind(&addr).serve(make_service);

    server
        .await
        .map_err(|err| ProxyError::Other(err.to_string()))
}

async fn server_upgrade<Fun, Ret>(
    mut req: Request<Body>,
    path: String,
    multi: Arc<service::Server<TcpStream, Upgraded, Fun, String, Ret>>,
) -> Result<Response<Body>>
where
    Ret: Future<Output = Result<TcpStream>> + Send + Sync + 'static,
    Fun: Fn(String) -> Ret + Sync + Send + 'static,
{
    let mut res = Response::new(Body::empty());
    let uri = req.uri();

    if uri.path() != path {
        *res.status_mut() = StatusCode::NOT_FOUND;
        return Ok(res);
    }

    let params = match uri.query() {
        Some(query) => form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect::<HashMap<String, String>>(),
        None => {
            *res.status_mut() = StatusCode::BAD_REQUEST;
            return Ok(res);
        }
    };

    let proxy_to = params.get("to");

    let mut res = Response::new(Body::empty());
    if !req.headers().contains_key(UPGRADE) || proxy_to.is_none() {
        *res.status_mut() = StatusCode::BAD_REQUEST;
        return Ok(res);
    }

    let proxy_to = proxy_to.unwrap().to_string();

    tokio::task::spawn(async move {
        match hyper::upgrade::on(&mut req).await {
            Ok(upgraded) => {
                multi.accept_client(upgraded, proxy_to).await;
            }
            Err(e) => eprintln!("upgrade error: {}", e),
        }
    });

    *res.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    res.headers_mut()
        .insert(UPGRADE, HeaderValue::from_static("multi"));
    Ok(res)
}

/// Websocket Client
pub async fn client_upgrade_request(addr: String) -> Result<Upgraded> {
    let req = Request::builder()
        .uri(&addr)
        .header(UPGRADE, "multi")
        .body(Body::empty())
        .unwrap();

    let res = {
        if addr.contains("https") {
            let https = HttpsConnector::new();
            let client = Client::builder()
                .pool_idle_timeout(None)
                .pool_max_idle_per_host(1)
                .build::<_, hyper::Body>(https);
            client
                .request(req)
                .await
                .map_err(|err| ProxyError::Other(err.to_string()))?
        } else {
            Client::new()
                .request(req)
                .await
                .map_err(|err| ProxyError::Other(err.to_string()))?
        }
    };
    if res.status() != StatusCode::SWITCHING_PROTOCOLS {
        return Err(ProxyError::Other(format!(
            "server return invalid status code: {}",
            res.status()
        )));
    }

    hyper::upgrade::on(res)
        .await
        .map_err(|err| ProxyError::Other(err.to_string()))
}
