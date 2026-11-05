#![cfg(all(unix, feature = "net"))]

use core::pin::pin;
use core::time::Duration;

use std::sync::Arc;

use anyhow::Context as _;
use bytes::BytesMut;
use futures::TryStreamExt as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::join;
use tokio::net::UnixStream;
use tokio::time::timeout;
use wrpc_transport::frame::encode_invocation;
use wrpc_transport::{InvokeExt as _, ServeExt as _};

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::serve_unix(async |clt, srv| {
        wrpc_test::assert_single_invocation((), &clt, srv).await?;
        wrpc_test::assert_invalid_data_resets((), &clt, srv).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn unclosed_root_fails_void_call() -> anyhow::Result<()> {
    wrpc_test::serve_unix(async |clt, srv| {
        let invocations = srv.serve_framed("foo", "bar", Arc::from([])).await?;
        let mut invocations = pin!(invocations);
        let (res, srv) = join!(
            clt.invoke_values_blocking::<_, (), ()>((), "foo", "bar", (), [[None; 0]; 0]),
            async {
                let invocation = invocations.try_next().await?;
                drop(invocation);
                anyhow::Ok(())
            }
        );
        srv?;
        res.expect_err("a void call must fail if the root is not closed");
        Ok(())
    })
    .await
}

#[test_log::test(tokio::test)]
async fn invalid_data_closes_socket() -> anyhow::Result<()> {
    let srv = wrpc_transport::unix::Server::default();
    let invocations = srv.serve_framed("foo", "bar", Arc::from([])).await?;
    let mut invocations = pin!(invocations);
    let (mut clt, stream) = UnixStream::pair()?;
    let mut buf = BytesMut::default();
    encode_invocation(&mut buf, "foo", "bar", 0)?;
    clt.write_all(&buf).await?;
    let addr = stream.peer_addr()?;
    let (rx, tx) = stream.into_split();
    srv.accept(addr, tx, BufReader::new(rx)).await?;
    let invocation = invocations.try_next().await?;
    let (.., outgoing, incoming) = invocation.context("invocation stream unexpectedly finished")?;
    clt.write_all(b"\x01\x01\x01x").await?;
    incoming
        .closed()
        .await
        .expect_err("invalid data must fail ingress");
    drop(incoming);
    drop(outgoing);
    let res = timeout(Duration::from_secs(5), clt.read_to_end(&mut Vec::default())).await;
    res.context("socket was not closed")??;
    let err = clt.write_all(b"x").await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
    Ok(())
}
