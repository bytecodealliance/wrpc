use anyhow::{bail, Context as _};
use bytes::Bytes;
use criterion::measurement::Measurement;
use criterion::{BenchmarkGroup, Criterion};
use futures::StreamExt as _;
use tokio::select;
use tokio::sync::oneshot;

mod rtt_bindings {
    wit_bindgen_wrpc::generate!({
        inline: "
            package wrpc-bench:rtt;

            interface rtt {
                ping: func();
            }

            world proxy {
                import rtt;
                export rtt;
            }
"
    });
}

#[derive(Clone, Copy)]
struct RttHandler;

impl<T: Send> rtt_bindings::exports::wrpc_bench::rtt::rtt::Handler<T> for RttHandler {
    async fn ping(&self, _cx: T) -> anyhow::Result<()> {
        Ok(())
    }
}

pub fn with_nats<T>(
    f: impl FnOnce(&tokio::runtime::Runtime, async_nats::Client) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let rt = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;

    let (_, nats_clt, nats_srv, stop_tx) = rt.block_on(wrpc_test::start_nats())?;
    let res = f(&rt, nats_clt).context("closure failed")?;
    stop_tx.send(()).expect("failed to stop NATS.io server");
    rt.block_on(nats_srv)
        .context("failed to await NATS.io server stop")?
        .context("NATS.io server failed to stop")?;
    Ok(res)
}

fn bench_nats_base_rtt(g: &mut BenchmarkGroup<impl Measurement>) -> anyhow::Result<()> {
    with_nats(|rt, nats_clt| {
        const SUBJECT: &str = "wrpc.0.0.1.wrpc-bench:rtt/rtt.ping";
        let mut sub = rt
            .block_on(nats_clt.subscribe(SUBJECT))
            .context("failed to subscribe on echo subject")?;

        let (stop_tx, mut stop_rx) = oneshot::channel();
        let srv = rt.spawn({
            let nats_clt = nats_clt.clone();
            async move {
                loop {
                    select! {
                        Some(async_nats::Message {
                            payload,
                            status,
                            reply,
                            ..
                        }) = sub.next() => {
                            assert!(status.is_none());
                            let reply = reply.context("reply subject missing")?;
                            nats_clt.publish(reply, payload)
                                .await
                                .context("failed to publish message")?;
                        }
                        _ = &mut stop_rx => {
                            sub.unsubscribe().await.context("failed to unsubscribe")?;
                            return anyhow::Ok(())
                        }
                    }
                }
            }
        });
        g.bench_function("direct", |b| {
            b.to_async(rt).iter(|| async {
                let async_nats::Message {
                    payload, status, ..
                } = nats_clt
                    .request(SUBJECT, Bytes::default())
                    .await
                    .expect("failed to send message");
                assert!(status.is_none());
                assert!(payload.is_empty());
            })
        });
        stop_tx.send(()).expect("failed to stop `echo`");
        rt.block_on(async { srv.await.context("`echo` task panicked")? })?;
        Ok(())
    })
}

fn bench_nats_wrpc_rtt(g: &mut BenchmarkGroup<impl Measurement>) -> anyhow::Result<()> {
    with_nats(|rt, nats_clt| {
        let wrpc = wrpc_transport_nats::Client::new(nats_clt, "", None);

        let invocations = rt
            .block_on(rtt_bindings::serve(&wrpc, RttHandler))
            .context("failed to serve bindings")?;
        let mut invocations = invocations.into_iter();
        let (Some((_, _, mut invocations)), None) = (invocations.next(), invocations.next()) else {
            bail!("invalid invocation stream vector")
        };
        let (stop_tx, mut stop_rx) = oneshot::channel();
        let handle = rt.handle().clone();
        let srv = handle.spawn({
            let handle = handle.clone();
            async move {
                loop {
                    select! {
                        Some(res) = invocations.next() => {
                            let fut = res.expect("failed to accept invocation");
                            handle.spawn(fut);
                        },
                        _ = &mut stop_rx => {
                            return anyhow::Ok(())
                        },
                    }
                }
            }
        });
        g.bench_function("wRPC", |b| {
            b.to_async(rt).iter(|| async {
                rtt_bindings::wrpc_bench::rtt::rtt::ping(&wrpc, None)
                    .await
                    .expect("failed to call `ping`");
            })
        });
        stop_tx.send(()).expect("failed to stop server");
        rt.block_on(async { srv.await.context("server task panicked")? })?;
        Ok(())
    })
}

fn main() -> anyhow::Result<()> {
    let mut c = Criterion::default().configure_from_args();
    {
        let mut g = c.benchmark_group("NATS.io ping RTT");
        bench_nats_base_rtt(&mut g)?;
        bench_nats_wrpc_rtt(&mut g)?;
        g.finish();
    }
    c.final_summary();
    Ok(())
}
