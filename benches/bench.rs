use core::future::Future;
use core::iter;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context as _};
use bytes::Bytes;
use clap::Parser as _;
use criterion::measurement::Measurement;
use criterion::{BenchmarkGroup, Criterion};
use futures::StreamExt as _;
use tokio::select;
use tokio::sync::oneshot;
use wasmtime::component::{Component, Linker};
use wasmtime_wasi::{IoView, ResourceTable, WasiCtx, WasiCtxBuilder, WasiView};

mod ping_bindings_wrpc {
    wit_bindgen_wrpc::generate!({
        world: "ping-proxy",
        path: "benches/wit",
    });
}
mod ping_bindings_wasmtime {
    wasmtime::component::bindgen!({
        async: true,
        world: "ping-proxy",
        path: "benches/wit",
    });
}
mod greet_bindings_wrpc {
    wit_bindgen_wrpc::generate!({
        world: "greet-proxy",
        path: "benches/wit",
    });
}
mod greet_bindings_wasmtime {
    wasmtime::component::bindgen!({
        async: true,
        world: "greet-proxy",
        path: "benches/wit",
    });
}

use greet_bindings_wasmtime::GreetProxyPre;
use greet_bindings_wrpc::exports::wrpc_bench::bench::greet::Handler as _;
use ping_bindings_wasmtime::PingProxyPre;
use ping_bindings_wrpc::exports::wrpc_bench::bench::ping::Handler as _;

const PING_SUBJECT: &str = "wrpc.0.0.1.wrpc-bench:bench/ping.ping";
const GREET_SUBJECT: &str = "wrpc.0.0.1.wrpc-bench:bench/greet.greet";

#[derive(Clone, Copy)]
struct NativeHandler;

impl<T: Send> ping_bindings_wrpc::exports::wrpc_bench::bench::ping::Handler<T> for NativeHandler {
    async fn ping(&self, _cx: T) -> anyhow::Result<()> {
        Ok(())
    }
}

impl<T: Send> greet_bindings_wrpc::exports::wrpc_bench::bench::greet::Handler<T> for NativeHandler {
    async fn greet(&self, _cx: T, name: String) -> anyhow::Result<String> {
        Ok(format!("Hello, {name}"))
    }
}

struct Ctx {
    table: ResourceTable,
    wasi: WasiCtx,
}

impl IoView for Ctx {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

impl WasiView for Ctx {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
}

#[derive(Clone)]
struct WasmHandler {
    engine: wasmtime::Engine,
    ping_pre: PingProxyPre<Ctx>,
    greet_pre: GreetProxyPre<Ctx>,
}

impl WasmHandler {
    // https://github.com/bytecodealliance/wasmtime/blob/b943666650696f1eb7ff8b217762b58d5ef5779d/src/commands/serve.rs#L641-L656
    fn use_pooling_allocator_by_default() -> anyhow::Result<Option<bool>> {
        const BITS_TO_TEST: u32 = 42;
        let mut config = wasmtime::Config::new();
        config.wasm_memory64(true);
        config.memory_reservation(1 << BITS_TO_TEST);
        let engine = wasmtime::Engine::new(&config)?;
        let mut store = wasmtime::Store::new(&engine, ());
        // NB: the maximum size is in wasm pages to take out the 16-bits of wasm
        // page size here from the maximum size.
        let ty = wasmtime::MemoryType::new64(0, Some(1 << (BITS_TO_TEST - 16)));
        if wasmtime::Memory::new(&mut store, ty).is_ok() {
            Ok(Some(true))
        } else {
            Ok(None)
        }
    }

    fn new_store(&self) -> wasmtime::Store<Ctx> {
        wasmtime::Store::new(
            &self.engine,
            Ctx {
                table: ResourceTable::new(),
                wasi: WasiCtxBuilder::new()
                    .inherit_env()
                    .inherit_stdio()
                    .inherit_network()
                    .allow_ip_name_lookup(true)
                    .allow_tcp(true)
                    .allow_udp(true)
                    .build(),
            },
        )
    }

    #[allow(clippy::unused_async)]
    pub async fn new(wasm: &[u8]) -> anyhow::Result<Self> {
        let mut opts =
            wasmtime_cli_flags::CommonOptions::try_parse_from(iter::empty::<&'static str>())
                .context("failed to construct common Wasmtime options")?;
        let mut config = opts
            .config(Self::use_pooling_allocator_by_default().unwrap_or(None))
            .context("failed to construct Wasmtime config")?;
        config.wasm_component_model(true);
        config.async_support(true);
        let engine =
            wasmtime::Engine::new(&config).context("failed to initialize Wasmtime engine")?;

        let component = Component::new(&engine, wasm).context("failed to compile component")?;

        let mut linker = Linker::<Ctx>::new(&engine);
        wasmtime_wasi::add_to_linker_async(&mut linker).context("failed to link WASI")?;

        let pre = linker
            .instantiate_pre(&component)
            .context("failed to pre-instantiate component")?;
        let ping_pre = PingProxyPre::new(pre.clone())
            .context("failed to pre-instantiate `ping-proxy` world")?;
        let greet_pre =
            GreetProxyPre::new(pre).context("failed to pre-instantiate `greet-proxy` world")?;
        Ok(Self {
            engine,
            ping_pre,
            greet_pre,
        })
    }
}

impl<T: Send> ping_bindings_wrpc::exports::wrpc_bench::bench::ping::Handler<T> for WasmHandler {
    async fn ping(&self, _cx: T) -> anyhow::Result<()> {
        let mut store = self.new_store();
        let ping = self.ping_pre.instantiate_async(&mut store).await?;
        ping.wrpc_bench_bench_ping().call_ping(&mut store).await
    }
}

impl<T: Send> greet_bindings_wrpc::exports::wrpc_bench::bench::greet::Handler<T> for WasmHandler {
    async fn greet(&self, _cx: T, name: String) -> anyhow::Result<String> {
        let mut store = self.new_store();
        let greet = self.greet_pre.instantiate_async(&mut store).await?;
        greet
            .wrpc_bench_bench_greet()
            .call_greet(&mut store, &name)
            .await
    }
}

pub fn with_nats<T>(
    rt: &tokio::runtime::Runtime,
    f: impl FnOnce(async_nats::Client) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let (_, nats_clt, nats_srv, stop_tx) = rt.block_on(wrpc_test::start_nats())?;
    let res = f(nats_clt).context("closure failed")?;
    stop_tx.send(()).expect("failed to stop NATS.io server");
    rt.block_on(nats_srv)
        .context("failed to await NATS.io server stop")?
        .context("NATS.io server failed to stop")?;
    Ok(res)
}

fn bench_wasm_ping_direct(
    g: &mut BenchmarkGroup<impl Measurement>,
    wasm: &[u8],
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;
    let handler = rt
        .block_on(WasmHandler::new(wasm))
        .context("failed to construct a Wasm handler")?;
    g.bench_function("direct", |b| {
        b.to_async(&rt).iter(|| async {
            handler
                .ping(None::<async_nats::HeaderMap>)
                .await
                .expect("failed to call handler");
        });
    });
    Ok(())
}

fn bench_wasm_greet_direct(
    g: &mut BenchmarkGroup<impl Measurement>,
    wasm: &[u8],
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;
    let handler = rt
        .block_on(WasmHandler::new(wasm))
        .context("failed to construct a Wasm handler")?;
    g.bench_function("direct", |b| {
        b.to_async(&rt).iter(|| async {
            let greeting = handler
                .greet(None::<async_nats::HeaderMap>, "test".into())
                .await
                .expect("failed to call handler");
            assert_eq!(greeting, "Hello, test");
        });
    });
    Ok(())
}

fn bench_raw_nats<Fut>(
    rt: &tokio::runtime::Runtime,
    g: &mut BenchmarkGroup<impl Measurement>,
    id: &str,
    subject: &'static str,
    handle: impl Fn(async_nats::Client, async_nats::Message) -> Fut + Send + 'static,
    payload: &Bytes,
    expect: &[u8],
) -> anyhow::Result<()>
where
    Fut: Future<Output = anyhow::Result<()>> + Send,
{
    with_nats(rt, |nats| {
        let mut sub = rt
            .block_on(nats.subscribe(subject))
            .context("failed to subscribe on subject")?;

        let (stop_tx, mut stop_rx) = oneshot::channel();
        let srv = rt.spawn({
            let nats = nats.clone();
            async move {
                loop {
                    select! {
                        Some(msg) = sub.next() => { handle(nats.clone(), msg).await? }
                        _ = &mut stop_rx => {
                            sub.unsubscribe().await.context("failed to unsubscribe")?;
                            return anyhow::Ok(())
                        }
                    }
                }
            }
        });
        g.bench_function(id, |b| {
            b.to_async(rt).iter(|| async {
                let async_nats::Message {
                    payload, status, ..
                } = nats
                    .request(subject, payload.clone())
                    .await
                    .expect("failed to send message");
                assert!(status.is_none());
                assert_eq!(payload, expect);
            });
        });
        stop_tx.send(()).expect("failed to stop server");
        rt.block_on(async { srv.await.context("server task panicked")? })?;
        Ok(())
    })
}

fn bench_nats_raw_ping(g: &mut BenchmarkGroup<impl Measurement>) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;
    bench_raw_nats(
        &rt,
        g,
        "raw",
        PING_SUBJECT,
        |nats,
         async_nats::Message {
             payload,
             status,
             reply,
             headers,
             ..
         }| async move {
            assert!(status.is_none());
            assert!(payload.is_empty());
            let reply = reply.context("reply subject missing")?;
            NativeHandler
                .ping(headers)
                .await
                .context("failed to call handler")?;
            nats.clone()
                .publish(reply, Bytes::default())
                .await
                .context("failed to publish response")
        },
        &Bytes::default(),
        &[],
    )
}

fn bench_nats_raw_greet(g: &mut BenchmarkGroup<impl Measurement>) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;
    bench_raw_nats(
        &rt,
        g,
        "raw",
        GREET_SUBJECT,
        |nats,
         async_nats::Message {
             payload,
             status,
             reply,
             headers,
             ..
         }| async move {
            assert!(status.is_none());
            let reply = reply.context("reply subject missing")?;
            let payload = String::from_utf8(Vec::from(payload)).context("payload in not UTF-8")?;
            let payload = NativeHandler
                .greet(headers, payload)
                .await
                .context("failed to call handler")?;
            nats.publish(reply, Bytes::from(payload))
                .await
                .context("failed to publish response")
        },
        &Bytes::from("test"),
        b"Hello, test",
    )
}

fn bench_wasm_ping_nats_raw(
    g: &mut BenchmarkGroup<impl Measurement>,
    wasm: &[u8],
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;
    let handler = rt
        .block_on(WasmHandler::new(wasm))
        .context("failed to construct a Wasm handler")?;
    bench_raw_nats(
        &rt,
        g,
        "raw NATS.io",
        PING_SUBJECT,
        move |nats,
              async_nats::Message {
                  payload,
                  status,
                  reply,
                  headers,
                  ..
              }| {
            let handler = handler.clone();
            async move {
                assert!(status.is_none());
                assert!(payload.is_empty());
                let reply = reply.context("reply subject missing")?;
                handler
                    .ping(headers)
                    .await
                    .context("failed to call handler")?;
                nats.clone()
                    .publish(reply, Bytes::default())
                    .await
                    .context("failed to publish response")
            }
        },
        &Bytes::default(),
        &[],
    )
}

fn bench_wasm_greet_nats_raw(
    g: &mut BenchmarkGroup<impl Measurement>,
    wasm: &[u8],
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;
    let handler = rt
        .block_on(WasmHandler::new(wasm))
        .context("failed to construct a Wasm handler")?;
    bench_raw_nats(
        &rt,
        g,
        "raw NATS.io",
        GREET_SUBJECT,
        move |nats,
              async_nats::Message {
                  payload,
                  status,
                  reply,
                  headers,
                  ..
              }| {
            let handler = handler.clone();
            async move {
                assert!(status.is_none());
                let reply = reply.context("reply subject missing")?;
                let payload =
                    String::from_utf8(Vec::from(payload)).context("payload in not UTF-8")?;
                let payload = handler
                    .greet(headers, payload)
                    .await
                    .context("failed to call handler")?;
                nats.clone()
                    .publish(reply, payload.into())
                    .await
                    .context("failed to publish response")
            }
        },
        &Bytes::from("test"),
        b"Hello, test",
    )
}

fn bench_wasm_ping_nats_wrpc(
    g: &mut BenchmarkGroup<impl Measurement>,
    wasm: &[u8],
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;
    let handler = rt
        .block_on(WasmHandler::new(wasm))
        .context("failed to construct a Wasm handler")?;
    with_nats(&rt, |nats| {
        let wrpc = rt
            .block_on(wrpc_transport_nats::Client::new(nats, "", None))
            .context("failed to construct client")?;

        let invocations = rt
            .block_on(ping_bindings_wrpc::serve(&wrpc, handler))
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
                            drop(invocations);
                            return anyhow::Ok(())
                        },
                    }
                }
            }
        });
        g.bench_function("wRPC NATS.io", |b| {
            b.to_async(&rt).iter(|| async {
                ping_bindings_wrpc::wrpc_bench::bench::ping::ping(&wrpc, None)
                    .await
                    .expect("failed to call `ping`");
            });
        });
        stop_tx.send(()).expect("failed to stop server");
        rt.block_on(async { srv.await.context("server task panicked")? })?;
        Ok(())
    })
}

fn bench_wasm_greet_nats_wrpc(
    g: &mut BenchmarkGroup<impl Measurement>,
    wasm: &[u8],
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;
    let handler = rt
        .block_on(WasmHandler::new(wasm))
        .context("failed to construct a Wasm handler")?;
    with_nats(&rt, |nats| {
        let wrpc = rt
            .block_on(wrpc_transport_nats::Client::new(nats, "", None))
            .context("failed to construct client")?;

        let invocations = rt
            .block_on(greet_bindings_wrpc::serve(&wrpc, handler))
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
                            drop(invocations);
                            return anyhow::Ok(())
                        },
                    }
                }
            }
        });
        g.bench_function("wRPC NATS.io", |b| {
            b.to_async(&rt).iter(|| async {
                greet_bindings_wrpc::wrpc_bench::bench::greet::greet(&wrpc, None, "test")
                    .await
                    .expect("failed to call `greet`");
            });
        });
        stop_tx.send(()).expect("failed to stop server");
        rt.block_on(async { srv.await.context("server task panicked")? })?;
        Ok(())
    })
}

fn bench_nats_wrpc_ping(g: &mut BenchmarkGroup<impl Measurement>) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;
    with_nats(&rt, |nats| {
        let wrpc = rt
            .block_on(wrpc_transport_nats::Client::new(nats, "", None))
            .context("failed to construct client")?;

        let invocations = rt
            .block_on(ping_bindings_wrpc::serve(&wrpc, NativeHandler))
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
                            drop(invocations);
                            return anyhow::Ok(())
                        },
                    }
                }
            }
        });
        g.bench_function("wRPC", |b| {
            b.to_async(&rt).iter(|| async {
                ping_bindings_wrpc::wrpc_bench::bench::ping::ping(&wrpc, None)
                    .await
                    .expect("failed to call `ping`");
            });
        });
        stop_tx.send(()).expect("failed to stop server");
        rt.block_on(async { srv.await.context("server task panicked")? })?;
        Ok(())
    })
}

fn bench_nats_wrpc_greet(g: &mut BenchmarkGroup<impl Measurement>) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;
    with_nats(&rt, |nats| {
        let wrpc = rt
            .block_on(wrpc_transport_nats::Client::new(nats, "", None))
            .context("failed to construct client")?;

        let invocations = rt
            .block_on(greet_bindings_wrpc::serve(&wrpc, NativeHandler))
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
            b.to_async(&rt).iter(|| async {
                let greeting =
                    greet_bindings_wrpc::wrpc_bench::bench::greet::greet(&wrpc, None, "test")
                        .await
                        .expect("failed to call `greet`");
                assert_eq!(greeting, "Hello, test");
            });
        });
        stop_tx.send(()).expect("failed to stop server");
        rt.block_on(async { srv.await.context("server task panicked")? })?;
        Ok(())
    })
}

fn main() -> anyhow::Result<()> {
    let mut c = Criterion::default().configure_from_args();
    let res = Command::new(env!("CARGO"))
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-wasip2",
            "--manifest-path",
        ])
        .arg(PathBuf::from_iter([
            env!("CARGO_MANIFEST_DIR"),
            "benches",
            "reactor",
            "Cargo.toml",
        ]))
        .status()
        .context("failed to build `reactor.wasm`")?;
    assert!(res.success());
    let reactor = fs::read(PathBuf::from_iter([
        env!("CARGO_MANIFEST_DIR"),
        "target",
        "wasm32-wasip2",
        "release",
        "reactor.wasm",
    ]))
    .context("failed to read `reactor.wasm`")?;
    {
        let mut g = c.benchmark_group("Wasm ping RTT");
        bench_wasm_ping_direct(&mut g, &reactor)?;
        bench_wasm_ping_nats_raw(&mut g, &reactor)?;
        bench_wasm_ping_nats_wrpc(&mut g, &reactor)?;
        g.finish();
    }
    {
        let mut g = c.benchmark_group("Wasm greet RTT");
        bench_wasm_greet_direct(&mut g, &reactor)?;
        bench_wasm_greet_nats_raw(&mut g, &reactor)?;
        bench_wasm_greet_nats_wrpc(&mut g, &reactor)?;
        g.finish();
    }
    {
        let mut g = c.benchmark_group("NATS.io ping RTT");
        bench_nats_raw_ping(&mut g)?;
        bench_nats_wrpc_ping(&mut g)?;
        g.finish();
    }
    {
        let mut g = c.benchmark_group("NATS.io greet RTT");
        bench_nats_raw_greet(&mut g)?;
        bench_nats_wrpc_greet(&mut g)?;
        g.finish();
    }
    c.final_summary();
    Ok(())
}
