use core::net::{Ipv6Addr, SocketAddr};
use core::pin::pin;
use core::time::Duration;

use std::io::ErrorKind;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context as _, bail, ensure};
use futures::{StreamExt as _, stream};
use hyper::client::conn::http2;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tempfile::{TempDir, tempdir};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio::time::timeout;
use tokio::{select, spawn};
use tracing::{info, instrument};
use wrpc_transport::Invoke as _;

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

/// Serve the `echo-server` world of `tests/js/wit` on `srv`, passing each
/// connection accepted on `lis` to `accept`, until `tests/js/client.js`, run
/// against `addr`, exits.
async fn serve_echo<S>(
    srv: &S,
    lis: TcpListener,
    addr: &str,
    mut accept: impl AsyncFnMut(TcpStream, SocketAddr),
) -> anyhow::Result<()>
where
    S: wrpc_transport::Serve,
    S::Context: 'static,
{
    mod bindings {
        wit_bindgen_wrpc::generate!({
            world: "echo-server",
            path: "tests/js/wit",
        });
    }

    use bindings::exports::wrpc_test::js::echo::{Asyncs, Handler, Values};

    #[derive(Clone, Copy)]
    struct Echo;

    impl<C: Send> Handler<C> for Echo {
        async fn echo(&self, _cx: C, v: Values) -> anyhow::Result<Values> {
            Ok(v)
        }

        async fn echo_async(&self, _cx: C, v: Asyncs) -> anyhow::Result<Asyncs> {
            Ok(v)
        }
    }

    let invocations = bindings::serve(srv, Echo)
        .await
        .context("failed to serve `wrpc-test:js/echo`")?;
    let mut invocations = stream::select_all(
        invocations
            .into_iter()
            .map(|(instance, name, invocations)| invocations.map(move |res| (instance, name, res))),
    );

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
            res = lis.accept() => {
                let (stream, addr) = res.expect("failed to accept connection");
                accept(stream, addr).await;
            }
            Some((instance, name, invocation)) = invocations.next() => {
                let invocation = invocation
                    .unwrap_or_else(|err| panic!("failed to accept `{instance}.{name}` invocation: {err:?}"));
                spawn(async move {
                    if let Err(err) = invocation.await {
                        panic!("failed to serve `{instance}.{name}`: {err:?}");
                    }
                });
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

    let client = wrpc_transport::frame::tcp::Client::from(addr);
    {
        let (tx, mut rx) = client
            .invoke((), "wrpc-test:integration/foo", "foo", &[[]; 0])
            .await?;
        let mut invalid = tx.index(&[1])?;
        invalid.write_all(b"x").await?;
        let res = timeout(Duration::from_secs(5), rx.read_to_end(&mut Vec::default()))
            .await
            .context("invalid data did not reset the invocation")?;
        let err = res.expect_err("invalid data must reset the invocation");
        assert_eq!(err.kind(), ErrorKind::ConnectionReset);
    }

    common::assert_sync((), &client).await?;
    common::assert_async((), &client).await?;

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
    let client = wrpc_http::Client::from(sender);
    let (mut cx, ()) = http::Request::new(()).into_parts();
    cx.uri = format!("http://{addr}/")
        .parse()
        .context("failed to parse request URI")?;
    common::assert_sync(cx.clone(), &client).await?;
    common::assert_async(cx, &client).await?;

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
    let srv = wrpc_transport::frame::Server::default();
    serve_echo(&srv, lis, &addr.to_string(), async |stream, addr| {
        let (rx, tx) = wrpc_transport::tcp::split(stream);
        srv.accept(addr, tx, BufReader::new(rx))
            .await
            .expect("failed to accept invocation");
    })
    .await
}

#[instrument(ret)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn js_http_client() -> anyhow::Result<()> {
    let lis = TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
        .await
        .context("failed to start TCP listener")?;
    let addr = lis.local_addr().context("failed to get server address")?;
    let srv = wrpc_http::Server::default();
    serve_echo(
        srv.0.as_ref(),
        lis,
        &format!("http://{addr}"),
        async |stream, _| {
            let svc = wrpc_http::Server::from(Arc::clone(&srv.0));
            spawn(async move {
                http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await
                    .expect("failed to serve HTTP connection");
            });
        },
    )
    .await
}
