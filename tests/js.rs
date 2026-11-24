use core::net::{Ipv6Addr, SocketAddr};
use core::pin::pin;
use core::time::Duration;

use std::path::Path;
use std::process::Stdio;

use anyhow::{Context as _, bail, ensure};
use bytes::BytesMut;
use hyper::client::conn::http2;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tempfile::{TempDir, tempdir};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::time::timeout;
use tokio::{select, spawn};
use tokio_util::codec::Encoder as _;
use tracing::{info, instrument};
use wrpc_core::{FrameEncoder, FrameRef, Framed, Header, HeaderEncoder, HeaderRef, Listener};

pub mod common;

/// Generate the bindings of each of `worlds` of the WIT at `wit` into a
/// temporary directory, in a subdirectory named after the world.
async fn bindgen(wit: &str, worlds: &[&str]) -> anyhow::Result<TempDir> {
    let dir = tempdir().context("failed to create temporary directory")?;
    for world in worlds {
        let status = Command::new(env!("CARGO_BIN_EXE_wit-bindgen-wrpc"))
            .args(["js", "--world", world, "--wrpc-path"])
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/js/src/index.js"))
            .arg("--out-dir")
            .arg(dir.path().join(world))
            .arg(wit)
            .kill_on_drop(true)
            .status()
            .await
            .context("failed to run `wit-bindgen-wrpc`")?;
        ensure!(status.success(), "`wit-bindgen-wrpc` failed for `{world}`");
    }
    Ok(dir)
}

/// Start `tests/js/server.js` listening on `addr` with the bindings in
/// `bindings` and wait until it is ready.
async fn start_server(addr: &str, bindings: &Path) -> anyhow::Result<Child> {
    info!("starting `tests/js/server.js`");
    let mut server = Command::new("node")
        .arg("tests/js/server.js")
        .arg(addr)
        .arg(bindings)
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("failed to run `node`")?;
    let mut stdout = BufReader::new(server.stdout.take().context("stdout missing")?).lines();
    let Some(line) = stdout.next_line().await? else {
        bail!("`tests/js/server.js` exited before listening");
    };
    ensure!(
        line == "listening",
        "unexpected `tests/js/server.js` output: {line}"
    );
    Ok(server)
}

mod echo {
    mod bindings {
        wit_bindgen_wrpc::generate!({
            world: "echo-server",
            path: "tests/js/wit",
        });
    }

    pub use bindings::handle;

    use bindings::exports::wrpc_test::js::echo::{Asyncs, Handler, Values};

    #[derive(Clone, Copy)]
    pub struct Echo;

    impl Handler for Echo {
        async fn echo(&self, v: Values) -> std::io::Result<Values> {
            Ok(v)
        }

        async fn echo_async(&self, v: Asyncs) -> std::io::Result<Asyncs> {
            Ok(v)
        }
    }
}

/// Serve the `echo-server` world of `tests/js/wit` on the connections accepted using `accept`
/// with `serve` until `tests/js/client.js`, run against `addr`, exits.
async fn serve_echo<T, E, Fut>(
    mut accept: impl AsyncFnMut() -> Result<T, E>,
    serve: impl Fn(T) -> Fut,
    addr: &str,
) -> anyhow::Result<()>
where
    E: core::error::Error + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let bindings = bindgen("tests/js/wit", &["echo-client"]).await?;
    info!("running `tests/js/client.js`");
    let mut client = pin!(
        Command::new("node")
            .args(["--test-force-exit", "tests/js/client.js", addr])
            .arg(bindings.path())
            .kill_on_drop(true)
            .status()
    );
    loop {
        select! {
            status = &mut client => {
                let status = status.context("failed to run `node`")?;
                ensure!(status.success(), "`tests/js/client.js` failed");
                return Ok(());
            }
            res = accept() => {
                let fut = serve(res.expect("failed to accept connection"));
                spawn(async move { fut.await.expect("failed to serve connection") });
            }
        }
    }
}

#[instrument(ret)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn js_server() -> anyhow::Result<()> {
    let bindings = bindgen("tests/wit", &["sync-server", "async-server"]).await?;
    let port = wrpc_test::free_port()
        .await
        .context("failed to find free port")?;
    let addr = SocketAddr::from((Ipv6Addr::LOCALHOST, port));
    let mut server = start_server(&addr.to_string(), bindings.path()).await?;

    {
        let mut stream = wrpc_test::tcp_connect(addr).await?;
        let (mut rx, mut tx) = stream.split();
        let mut buf = BytesMut::default();
        HeaderEncoder.encode(
            HeaderRef {
                instance: "foo",
                name: "foo",
            },
            &mut buf,
        )?;
        FrameEncoder.encode(
            FrameRef {
                path: &[1],
                data: b"x",
            },
            &mut buf,
        )?;
        tx.write_all(&buf).await?;
        tx.flush().await?;
        let res = timeout(Duration::from_secs(5), rx.read_to_end(&mut Vec::default()))
            .await
            .context("invalid data did not reset the invocation")?;
        let err = res.expect_err("invalid data must reset the invocation");
        assert_eq!(err.kind(), std::io::ErrorKind::ConnectionReset);
    }

    let (tx, rx) = wrpc_test::tcp_streams(addr).await?;
    let mut clt = Framed::new(tx, rx);
    common::assert_sync(&mut clt).await?;
    common::assert_async(&mut clt).await?;
    common::assert_eof(clt).await?;

    server.start_kill().context("failed to kill `node`")?;
    server.wait().await.context("failed to wait for `node`")?;
    Ok(())
}

#[instrument(ret)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn js_http_server() -> anyhow::Result<()> {
    let bindings = bindgen("tests/wit", &["sync-server", "async-server"]).await?;
    let port = wrpc_test::free_port()
        .await
        .context("failed to find free port")?;
    let addr = SocketAddr::from((Ipv6Addr::LOCALHOST, port));
    let mut server = start_server(&format!("http://{addr}"), bindings.path()).await?;

    let conn = wrpc_test::tcp_connect(addr).await?;
    let (sender, conn) = http2::handshake(TokioExecutor::new(), TokioIo::new(conn))
        .await
        .context("failed to establish HTTP/2 connection")?;
    spawn(conn);
    let (mut parts, ()) = http::Request::new(()).into_parts();
    parts.uri = format!("http://{addr}/")
        .parse()
        .context("failed to parse request URI")?;
    let mut clt = (sender, parts);
    common::assert_sync(&mut clt).await?;
    common::assert_async(&mut clt).await?;

    server.start_kill().context("failed to kill `node`")?;
    server.wait().await.context("failed to wait for `node`")?;
    Ok(())
}

#[instrument(ret)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn js_client() -> anyhow::Result<()> {
    let lis = TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
        .await
        .context("failed to start TCP listener")?;
    let addr = lis.local_addr().context("failed to get server address")?;
    serve_echo(
        async || wrpc_test::tcp_accept(&lis).await,
        |(tx, rx)| async move {
            let mut lis = Listener::new(tx, rx);
            while let Some((Header { instance, name }, srv)) = lis.accept().await? {
                let Ok(fut) = echo::handle(&echo::Echo, &instance, &name, srv) else {
                    bail!("`{instance}#{name}` is not exported");
                };
                fut.await?;
            }
            Ok(())
        },
        &addr.to_string(),
    )
    .await
}

#[instrument(ret)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn js_http_client() -> anyhow::Result<()> {
    let lis = TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
        .await
        .context("failed to start TCP listener")?;
    let addr = lis.local_addr().context("failed to get server address")?;
    serve_echo(
        async || {
            let (stream, ..) = lis.accept().await?;
            std::io::Result::Ok(stream)
        },
        |stream| {
            wrpc_test::serve_http(stream, |_, Header { instance, name }, srv| async move {
                let Ok(fut) = echo::handle(&echo::Echo, &instance, &name, srv) else {
                    bail!("`{instance}#{name}` is not exported");
                };
                let res = fut.await?;
                Ok(res)
            })
        },
        &format!("http://{addr}"),
    )
    .await
}
