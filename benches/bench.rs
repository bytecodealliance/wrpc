use core::iter;
use core::net::Ipv6Addr;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context as _, bail};
use clap::Parser as _;
use criterion::measurement::Measurement;
use criterion::{BenchmarkGroup, Criterion};
use futures::StreamExt as _;
use tokio::select;
use tokio::sync::oneshot;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

mod ping_bindings_wrpc {
    wit_bindgen_wrpc::generate!({
        world: "ping-proxy",
        path: "benches/wit",
    });
}
mod ping_bindings_wasmtime {
    wasmtime::component::bindgen!({
        exports: { default: async },
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
        exports: { default: async },
        world: "greet-proxy",
        path: "benches/wit",
    });
}

use greet_bindings_wasmtime::GreetProxyPre;
use greet_bindings_wrpc::exports::wrpc_bench::bench::greet::Handler as _;
use ping_bindings_wasmtime::PingProxyPre;
use ping_bindings_wrpc::exports::wrpc_bench::bench::ping::Handler as _;

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

impl WasiView for Ctx {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
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
            .map_err(anyhow::Error::from)
            .context("failed to construct Wasmtime config")?;
        config.wasm_component_model(true);
        let engine = wasmtime::Engine::new(&config)
            .map_err(anyhow::Error::from)
            .context("failed to initialize Wasmtime engine")?;

        let component = Component::new(&engine, wasm)
            .map_err(anyhow::Error::from)
            .context("failed to compile component")?;

        let mut linker = Linker::<Ctx>::new(&engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)
            .map_err(anyhow::Error::from)
            .context("failed to link WASI")?;

        let pre = linker
            .instantiate_pre(&component)
            .map_err(anyhow::Error::from)
            .context("failed to pre-instantiate component")?;
        let ping_pre = PingProxyPre::new(pre.clone())
            .map_err(anyhow::Error::from)
            .context("failed to pre-instantiate `ping-proxy` world")?;
        let greet_pre = GreetProxyPre::new(pre)
            .map_err(anyhow::Error::from)
            .context("failed to pre-instantiate `greet-proxy` world")?;
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
        let ping = self
            .ping_pre
            .instantiate_async(&mut store)
            .await
            .map_err(anyhow::Error::from)?;
        ping.wrpc_bench_bench_ping()
            .call_ping(&mut store)
            .await
            .map_err(anyhow::Error::from)
    }
}

impl<T: Send> greet_bindings_wrpc::exports::wrpc_bench::bench::greet::Handler<T> for WasmHandler {
    async fn greet(&self, _cx: T, name: String) -> anyhow::Result<String> {
        let mut store = self.new_store();
        let greet = self
            .greet_pre
            .instantiate_async(&mut store)
            .await
            .map_err(anyhow::Error::from)?;
        greet
            .wrpc_bench_bench_greet()
            .call_greet(&mut store, &name)
            .await
            .map_err(anyhow::Error::from)
    }
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
            handler.ping(()).await.expect("failed to call handler");
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
                .greet((), "test".into())
                .await
                .expect("failed to call handler");
            assert_eq!(greeting, "Hello, test");
        });
    });
    Ok(())
}

fn bench_wasm_ping_wrpc(
    g: &mut BenchmarkGroup<impl Measurement>,
    wasm: &[u8],
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;
    let handler = rt
        .block_on(WasmHandler::new(wasm))
        .context("failed to construct a Wasm handler")?;

    let lis = rt
        .block_on(tokio::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0)))
        .context("failed to bind TCP listener")?;
    let addr = lis
        .local_addr()
        .context("failed to query listener address")?;
    let srv = Arc::new(wrpc_transport::frame::Server::default());
    let wrpc = wrpc_transport::frame::tcp::Client::from(addr);

    let invocations = rt
        .block_on(ping_bindings_wrpc::serve(srv.as_ref(), handler))
        .context("failed to serve bindings")?;
    let mut invocations = invocations.into_iter();
    let (Some((_, _, mut invocations)), None) = (invocations.next(), invocations.next()) else {
        bail!("invalid invocation stream vector")
    };
    let (stop_tx, mut stop_rx) = oneshot::channel();
    let server = rt.spawn({
        let srv = Arc::clone(&srv);
        async move {
            loop {
                select! {
                    Some(res) = invocations.next() => {
                        let fut = res.expect("failed to accept invocation");
                        tokio::spawn(fut);
                    }
                    conn = lis.accept() => {
                        let (stream, _) = conn.expect("failed to accept TCP connection");
                        let (rx, tx) = stream.into_split();
                        srv.accept((), tx, rx)
                            .await
                            .expect("failed to accept connection");
                    }
                    _ = &mut stop_rx => {
                        drop(invocations);
                        return anyhow::Ok(());
                    }
                }
            }
        }
    });
    g.bench_function("wRPC", |b| {
        b.to_async(&rt).iter(|| async {
            ping_bindings_wrpc::wrpc_bench::bench::ping::ping(&wrpc, ())
                .await
                .expect("failed to call `ping`");
        });
    });
    stop_tx.send(()).expect("failed to stop server");
    rt.block_on(async { server.await.context("server task panicked")? })?;
    Ok(())
}

fn bench_wasm_greet_wrpc(
    g: &mut BenchmarkGroup<impl Measurement>,
    wasm: &[u8],
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;
    let handler = rt
        .block_on(WasmHandler::new(wasm))
        .context("failed to construct a Wasm handler")?;

    let lis = rt
        .block_on(tokio::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0)))
        .context("failed to bind TCP listener")?;
    let addr = lis
        .local_addr()
        .context("failed to query listener address")?;
    let srv = Arc::new(wrpc_transport::frame::Server::default());
    let wrpc = wrpc_transport::frame::tcp::Client::from(addr);

    let invocations = rt
        .block_on(greet_bindings_wrpc::serve(srv.as_ref(), handler))
        .context("failed to serve bindings")?;
    let mut invocations = invocations.into_iter();
    let (Some((_, _, mut invocations)), None) = (invocations.next(), invocations.next()) else {
        bail!("invalid invocation stream vector")
    };
    let (stop_tx, mut stop_rx) = oneshot::channel();
    let server = rt.spawn({
        let srv = Arc::clone(&srv);
        async move {
            loop {
                select! {
                    Some(res) = invocations.next() => {
                        let fut = res.expect("failed to accept invocation");
                        tokio::spawn(fut);
                    }
                    conn = lis.accept() => {
                        let (stream, _) = conn.expect("failed to accept TCP connection");
                        let (rx, tx) = stream.into_split();
                        srv.accept((), tx, rx)
                            .await
                            .expect("failed to accept connection");
                    }
                    _ = &mut stop_rx => {
                        drop(invocations);
                        return anyhow::Ok(());
                    }
                }
            }
        }
    });
    g.bench_function("wRPC", |b| {
        b.to_async(&rt).iter(|| async {
            greet_bindings_wrpc::wrpc_bench::bench::greet::greet(&wrpc, (), "test")
                .await
                .expect("failed to call `greet`");
        });
    });
    stop_tx.send(()).expect("failed to stop server");
    rt.block_on(async { server.await.context("server task panicked")? })?;
    Ok(())
}

fn bench_wrpc_ping(g: &mut BenchmarkGroup<impl Measurement>) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;

    let lis = rt
        .block_on(tokio::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0)))
        .context("failed to bind TCP listener")?;
    let addr = lis
        .local_addr()
        .context("failed to query listener address")?;
    let srv = Arc::new(wrpc_transport::frame::Server::default());
    let wrpc = wrpc_transport::frame::tcp::Client::from(addr);

    let invocations = rt
        .block_on(ping_bindings_wrpc::serve(srv.as_ref(), NativeHandler))
        .context("failed to serve bindings")?;
    let mut invocations = invocations.into_iter();
    let (Some((_, _, mut invocations)), None) = (invocations.next(), invocations.next()) else {
        bail!("invalid invocation stream vector")
    };
    let (stop_tx, mut stop_rx) = oneshot::channel();
    let server = rt.spawn({
        let srv = Arc::clone(&srv);
        async move {
            loop {
                select! {
                    Some(res) = invocations.next() => {
                        let fut = res.expect("failed to accept invocation");
                        tokio::spawn(fut);
                    }
                    conn = lis.accept() => {
                        let (stream, _) = conn.expect("failed to accept TCP connection");
                        let (rx, tx) = stream.into_split();
                        srv.accept((), tx, rx)
                            .await
                            .expect("failed to accept connection");
                    }
                    _ = &mut stop_rx => {
                        drop(invocations);
                        return anyhow::Ok(());
                    }
                }
            }
        }
    });
    g.bench_function("wRPC", |b| {
        b.to_async(&rt).iter(|| async {
            ping_bindings_wrpc::wrpc_bench::bench::ping::ping(&wrpc, ())
                .await
                .expect("failed to call `ping`");
        });
    });
    stop_tx.send(()).expect("failed to stop server");
    rt.block_on(async { server.await.context("server task panicked")? })?;
    Ok(())
}

fn bench_wrpc_greet(g: &mut BenchmarkGroup<impl Measurement>) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;

    let lis = rt
        .block_on(tokio::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0)))
        .context("failed to bind TCP listener")?;
    let addr = lis
        .local_addr()
        .context("failed to query listener address")?;
    let srv = Arc::new(wrpc_transport::frame::Server::default());
    let wrpc = wrpc_transport::frame::tcp::Client::from(addr);

    let invocations = rt
        .block_on(greet_bindings_wrpc::serve(srv.as_ref(), NativeHandler))
        .context("failed to serve bindings")?;
    let mut invocations = invocations.into_iter();
    let (Some((_, _, mut invocations)), None) = (invocations.next(), invocations.next()) else {
        bail!("invalid invocation stream vector")
    };
    let (stop_tx, mut stop_rx) = oneshot::channel();
    let server = rt.spawn({
        let srv = Arc::clone(&srv);
        async move {
            loop {
                select! {
                    Some(res) = invocations.next() => {
                        let fut = res.expect("failed to accept invocation");
                        tokio::spawn(fut);
                    }
                    conn = lis.accept() => {
                        let (stream, _) = conn.expect("failed to accept TCP connection");
                        let (rx, tx) = stream.into_split();
                        srv.accept((), tx, rx)
                            .await
                            .expect("failed to accept connection");
                    }
                    _ = &mut stop_rx => {
                        drop(invocations);
                        return anyhow::Ok(());
                    }
                }
            }
        }
    });
    g.bench_function("wRPC", |b| {
        b.to_async(&rt).iter(|| async {
            let greeting = greet_bindings_wrpc::wrpc_bench::bench::greet::greet(&wrpc, (), "test")
                .await
                .expect("failed to call `greet`");
            assert_eq!(greeting, "Hello, test");
        });
    });
    stop_tx.send(()).expect("failed to stop server");
    rt.block_on(async { server.await.context("server task panicked")? })?;
    Ok(())
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
        bench_wasm_ping_wrpc(&mut g, &reactor)?;
        g.finish();
    }
    {
        let mut g = c.benchmark_group("Wasm greet RTT");
        bench_wasm_greet_direct(&mut g, &reactor)?;
        bench_wasm_greet_wrpc(&mut g, &reactor)?;
        g.finish();
    }
    {
        let mut g = c.benchmark_group("wRPC ping RTT");
        bench_wrpc_ping(&mut g)?;
        g.finish();
    }
    {
        let mut g = c.benchmark_group("wRPC greet RTT");
        bench_wrpc_greet(&mut g)?;
        g.finish();
    }
    c.final_summary();
    Ok(())
}
