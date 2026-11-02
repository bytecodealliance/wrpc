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
    for shutdown in [true, false] {
        let (clt, mut srv) = UnixStream::pair()?;
        let (mut rx, mut tx) = unix::split(clt);
        tx.write_all(b"foo").await?;
        if shutdown {
            tx.shutdown().await?;
        }
        drop(tx);
        let mut buf = Vec::default();
        srv.read_to_end(&mut buf).await?;
        assert_eq!(buf, b"foo");
        let res = srv.write_all(b"bar").await;
        if shutdown {
            res?;
        } else {
            assert_eq!(res.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
            let err = rx.read_to_end(&mut Vec::default()).await.unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::ConnectionAborted);
        }
    }
    Ok(())
}
