use core::net::Ipv6Addr;
use core::pin::pin;

use std::process::Stdio;

use anyhow::{Context as _, bail, ensure};
use futures::{StreamExt as _, stream};
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::{select, spawn};
use tracing::{info, instrument};

pub mod common;

#[instrument(ret)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn js_server() -> anyhow::Result<()> {
    let port = wrpc_test::free_port()
        .await
        .context("failed to find free port")?;
    let addr = format!("[::1]:{port}");

    info!("starting `tests/js/server.js`");
    let mut server = Command::new("node")
        .args(["tests/js/server.js", &addr])
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

    let client = wrpc_transport::frame::tcp::Client::from(addr);
    common::assert_sync((), &client).await?;
    common::assert_async((), &client).await?;

    server.start_kill().context("failed to kill `node`")?;
    server.wait().await.context("failed to wait for `node`")?;
    Ok(())
}

#[instrument(ret)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn js_client() -> anyhow::Result<()> {
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

    let lis = TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
        .await
        .context("failed to start TCP listener")?;
    let addr = lis.local_addr().context("failed to get server address")?;
    let srv = wrpc_transport::frame::Server::default();
    let invocations = bindings::serve(&srv, Echo)
        .await
        .context("failed to serve `wrpc-test:js/echo`")?;
    let mut invocations = stream::select_all(
        invocations
            .into_iter()
            .map(|(instance, name, invocations)| invocations.map(move |res| (instance, name, res))),
    );

    info!("running `tests/js/client.js`");
    let mut client = pin!(
        Command::new("node")
            .args(["--test-force-exit", "tests/js/client.js", &addr.to_string()])
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
                let (rx, tx) = stream.into_split();
                srv.accept(addr, tx, rx).await.expect("failed to accept invocation");
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
