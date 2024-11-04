use core::pin::pin;

use std::sync::Arc;

use anyhow::Context as _;
use futures::StreamExt as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::try_join;
use tracing::info;
use wrpc_transport::{Index as _, Invoke as _, Serve as _};
use wrpc_transport_quic::Client;

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::with_quic(|clt, srv| async {
        let clt = Client::from(clt);
        let srv_conn = Client::from(srv);
        let srv = Arc::new(wrpc_transport_quic::Server::new());
        let invocations = srv
            .serve("foo", "bar", [Box::from([Some(42), Some(0)])])
            .await
            .context("failed to serve `foo.bar`")?;
        let mut invocations = pin!(invocations);
        try_join!(
            async {
                let (mut outgoing, mut incoming) = clt
                    .invoke((), "foo", "bar", "test".into(), &[&[Some(0), Some(42)]])
                    .await
                    .context("failed to invoke `foo.bar`")?;
                let mut nested_tx = outgoing.index(&[42, 0]).context("failed to index `42.0`")?;
                let mut nested_rx = incoming.index(&[0, 42]).context("failed to index `0.42`")?;
                try_join!(
                    async {
                        info!("reading `foo`");
                        let mut buf = vec![];
                        let n = incoming
                            .read_to_end(&mut buf)
                            .await
                            .context("failed to read `foo`")?;
                        assert_eq!(n, 3);
                        assert_eq!(buf, b"foo");
                        info!("read `foo`");
                        anyhow::Ok(())
                    },
                    async {
                        info!("writing `bar`");
                        outgoing
                            .write_all(b"bar")
                            .await
                            .context("failed to write `bar`")?;
                        outgoing
                            .shutdown()
                            .await
                            .context("failed to shutdown stream")?;
                        drop(outgoing);
                        info!("wrote `bar`");
                        anyhow::Ok(())
                    },
                    async {
                        info!("writing `client->server`");
                        nested_tx
                            .write_all(b"client->server")
                            .await
                            .context("failed to write `client->server`")?;
                        nested_tx
                            .shutdown()
                            .await
                            .context("failed to shutdown stream")?;
                        drop(nested_tx);
                        info!("wrote `client->server`");
                        anyhow::Ok(())
                    },
                    async {
                        info!("reading `server->client`");
                        let mut buf = vec![];
                        nested_rx
                            .read_to_end(&mut buf)
                            .await
                            .context("failed to read `server->client`")?;
                        assert_eq!(buf, b"server->client");
                        info!("read `server->client`");
                        anyhow::Ok(())
                    },
                )?;
                anyhow::Ok(())
            },
            async {
                srv.accept(srv_conn)
                    .await
                    .context("failed to accept invocation")?;
                let ((), mut outgoing, mut incoming) = invocations
                    .next()
                    .await
                    .context("invocation stream unexpectedly finished")?
                    .context("failed to get invocation")?;
                let mut nested_tx = outgoing.index(&[0, 42]).context("failed to index `0.42`")?;
                let mut nested_rx = incoming.index(&[42, 0]).context("failed to index `42.0`")?;
                try_join!(
                    async {
                        info!("reading `test`");
                        let mut buf = vec![0; 4];
                        incoming
                            .read_exact(&mut buf)
                            .await
                            .context("failed to read `test`")?;
                        assert_eq!(buf, b"test");
                        info!("read `test`");

                        info!("reading `bar`");
                        let mut buf = vec![];
                        let n = incoming
                            .read_to_end(&mut buf)
                            .await
                            .context("failed to read `bar`")?;
                        assert_eq!(n, 3);
                        assert_eq!(buf, b"bar");
                        info!("read `bar`");
                        anyhow::Ok(())
                    },
                    async {
                        info!("writing `foo`");
                        outgoing
                            .write_all(b"foo")
                            .await
                            .context("failed to write `foo`")?;
                        outgoing
                            .shutdown()
                            .await
                            .context("failed to shutdown stream")?;
                        drop(outgoing);
                        info!("wrote `foo`");
                        anyhow::Ok(())
                    },
                    async {
                        info!("writing `server->client`");
                        nested_tx
                            .write_all(b"server->client")
                            .await
                            .context("failed to write `server->client`")?;
                        nested_tx
                            .shutdown()
                            .await
                            .context("failed to shutdown stream")?;
                        drop(nested_tx);
                        info!("wrote `server->client`");
                        anyhow::Ok(())
                    },
                    async {
                        info!("reading `client->server`");
                        let mut buf = vec![];
                        let n = nested_rx
                            .read_to_end(&mut buf)
                            .await
                            .context("failed to read `client->server`")?;
                        assert_eq!(n, 14);
                        assert_eq!(buf, b"client->server");
                        info!("read `client->server`");
                        anyhow::Ok(())
                    },
                )?;
                Ok(())
            }
        )?;
        Ok(())
    })
    .await
}
