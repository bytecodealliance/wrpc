use anyhow::Context as _;
use bytes::BytesMut;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::join;
use tokio_util::codec::Encoder as _;
use wrpc_transport::frame::{Incoming, Server};
use wrpc_transport::{FrameEncoder, FrameRef};
use wrpc_websockets::{Client, split};

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    let srv = Server::default();
    wrpc_test::with_websockets(|clt, server, lis| async move {
        wrpc_test::assert_single_invocation(
            (),
            &Client::from(clt),
            &srv,
            wrpc_test::accept_websocket(&srv, &lis, &server, split),
        )
        .await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn writer_dropped() -> anyhow::Result<()> {
    wrpc_test::with_websockets(|clt, server, lis| async move {
        for shutdown in [true, false] {
            let (ws, connected) = join!(
                wrpc_test::accept_websocket_stream(&lis, &server),
                clt.connect(),
            );
            let (clt, _res) = connected.context("failed to connect")?;
            let (mut tx, rx) = split(clt);
            let mut frame = BytesMut::default();
            FrameEncoder.encode(
                FrameRef {
                    path: &[],
                    data: b"\x2a",
                },
                &mut frame,
            )?;
            tx.write_all(&frame).await.context("failed to write")?;
            tx.flush().await.context("failed to flush")?;
            if shutdown {
                tx.shutdown().await.context("failed to shut down")?;
            }
            drop(tx);
            drop(rx);
            let ws = ws?;
            let (_tx, rx) = split(ws);
            let mut rx = Incoming::new(rx, [[Some(0)]]);
            let finished = rx.finished();
            let mut buf = Vec::default();
            let res = rx.read_to_end(&mut buf).await;
            assert_eq!(buf, b"\x2a");
            drop(rx);
            if shutdown {
                res.context("failed to read")?;
                finished.await.context("ingress failed")?;
            } else {
                let err = res.unwrap_err();
                assert_eq!(err.kind(), std::io::ErrorKind::ConnectionAborted);
                let err = finished.await.unwrap_err();
                assert_eq!(err.kind(), std::io::ErrorKind::ConnectionAborted);
            }
        }
        Ok(())
    })
    .await
}
