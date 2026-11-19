use futures::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, duplex};
use tokio::net::TcpStream;
use tokio::try_join;
use tokio_websockets::{ClientBuilder, CloseCode, Limits, Message, ServerBuilder};
use wrpc_websockets::MAX_MESSAGE_LEN;

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |addr, lis| {
        let connect = async || {
            let stream = TcpStream::connect(addr).await?;
            wrpc_test::websocket_streams(stream).await
        };
        wrpc_test::assert_invalid_data_resets(connect, async || {
            wrpc_test::websocket_accept(&lis).await
        })
        .await
    })
    .await
}

#[test_log::test(tokio::test)]
async fn messages() -> anyhow::Result<()> {
    let failed = CloseCode::try_from(4000)?;
    for (msg, err) in [
        (Message::close(Some(CloseCode::NORMAL_CLOSURE), ""), None),
        (
            Message::close(Some(failed), ""),
            Some(std::io::ErrorKind::ConnectionAborted),
        ),
        (Message::text("bar"), Some(std::io::ErrorKind::InvalidData)),
        (
            Message::binary("barbaz"),
            Some(std::io::ErrorKind::InvalidData),
        ),
    ] {
        let (clt, srv) = duplex(64);
        let mut clt = ClientBuilder::new().take_over(clt);
        clt.send(Message::binary("foo")).await?;
        clt.send(msg).await?;
        let (.., mut rx) = wrpc_websockets::split(
            ServerBuilder::new()
                .limits(Limits::default().max_payload_len(Some(3)))
                .serve(srv),
        );
        let mut buf = Vec::default();
        let res = rx.read_to_end(&mut buf).await;
        assert_eq!(res.map_err(|err| err.kind()).err(), err);
        assert_eq!(buf, b"foo");
    }
    Ok(())
}

#[test_log::test(tokio::test)]
async fn shutdown() -> anyhow::Result<()> {
    let (clt, srv) = duplex(64);
    let mut srv = ServerBuilder::new().serve(srv);
    srv.send(Message::binary("foo")).await?;
    let (mut tx, mut rx) = wrpc_websockets::split(ClientBuilder::new().take_over(clt));
    let mut buf = Vec::default();
    try_join!(
        async {
            tx.shutdown().await?;
            rx.read_to_end(&mut buf).await?;
            anyhow::Ok(())
        },
        async {
            while let Some(msg) = srv.next().await {
                msg?;
            }
            anyhow::Ok(())
        },
    )?;
    assert_eq!(buf, b"foo");
    Ok(())
}

#[test_log::test(tokio::test)]
async fn max_message_len() -> anyhow::Result<()> {
    let (clt, srv) = duplex(64);
    let (mut tx, ..) = wrpc_websockets::split(ClientBuilder::new().take_over(clt));
    let (.., mut rx) = wrpc_websockets::split(
        ServerBuilder::new()
            .limits(Limits::default().max_payload_len(Some(MAX_MESSAGE_LEN)))
            .serve(srv),
    );
    let data = vec![0x42; MAX_MESSAGE_LEN + 1];
    let mut buf = Vec::default();
    try_join!(
        async {
            tx.write_all(&data).await?;
            tx.shutdown().await
        },
        rx.read_to_end(&mut buf),
    )?;
    assert_eq!(buf, data);
    Ok(())
}
