#![cfg(all(unix, feature = "net"))]

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixStream;
use wrpc_transport::unix;

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::serve_unix(async |clt, srv| wrpc_test::assert_single_invocation((), &clt, srv).await)
        .await
}

#[test_log::test(tokio::test)]
async fn write_half_dropped() -> anyhow::Result<()> {
    let (clt, mut srv) = UnixStream::pair()?;
    let (mut rx, mut tx) = unix::split(clt);
    tx.write_all(b"foo").await?;
    drop(tx);
    let mut buf = Vec::default();
    srv.read_to_end(&mut buf).await?;
    assert_eq!(buf, b"foo");
    srv.write_all(b"bar").await?;
    drop(srv);
    buf.clear();
    rx.read_to_end(&mut buf).await?;
    assert_eq!(buf, b"bar");
    Ok(())
}
