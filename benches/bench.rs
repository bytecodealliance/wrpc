use core::net::SocketAddr;
use core::pin::Pin;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context as _, bail, ensure};
use bytes::Bytes;
use criterion::measurement::Measurement;
use criterion::{BenchmarkGroup, Criterion};
use futures::{Stream, StreamExt as _};
use hyper_util::rt::TokioExecutor;
use hyper_util::rt::tokio::WithHyperIo;
use tokio::io::BufReader;
use tokio::net::UnixListener;
use tokio::runtime::{Handle, Runtime};
use tokio::task::JoinHandle;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wrpc_transport::frame::{Incoming, Outgoing};
use wrpc_transport::{Invoke, Serve};

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

type Invocations = Pin<
    Box<
        dyn Stream<
                Item = anyhow::Result<
                    Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>,
                >,
            > + Send
            + 'static,
    >,
>;

trait Handler<C>:
    ping_bindings_wrpc::exports::wrpc_bench::bench::ping::Handler<C>
    + greet_bindings_wrpc::exports::wrpc_bench::bench::greet::Handler<C>
    + Clone
    + Send
    + Sync
    + 'static
{
}

impl<C, T> Handler<C> for T where
    T: ping_bindings_wrpc::exports::wrpc_bench::bench::ping::Handler<C>
        + greet_bindings_wrpc::exports::wrpc_bench::bench::greet::Handler<C>
        + Clone
        + Send
        + Sync
        + 'static
{
}

#[derive(Clone, Copy)]
enum Func {
    Ping,
    Greet,
}

impl Func {
    async fn serve<S: Serve>(
        self,
        srv: &S,
        handler: impl Handler<S::Context>,
    ) -> anyhow::Result<Invocations> {
        let invocations = match self {
            Self::Ping => ping_bindings_wrpc::serve(srv, handler).await?,
            Self::Greet => greet_bindings_wrpc::serve(srv, handler).await?,
        };
        let mut invocations = invocations.into_iter();
        let (Some((_, _, invocations)), None) = (invocations.next(), invocations.next()) else {
            bail!("invalid invocation stream vector")
        };
        Ok(invocations)
    }

    async fn call<C: Invoke>(self, clt: &C, cx: C::Context) -> anyhow::Result<()> {
        match self {
            Self::Ping => ping_bindings_wrpc::wrpc_bench::bench::ping::ping(clt, cx)
                .await
                .context("failed to call `ping`"),
            Self::Greet => {
                let greeting =
                    greet_bindings_wrpc::wrpc_bench::bench::greet::greet(clt, cx, "test")
                        .await
                        .context("failed to call `greet`")?;
                ensure!(
                    greeting == "Hello, test",
                    "unexpected greeting `{greeting}`"
                );
                Ok(())
            }
        }
    }

    async fn call_direct(self, handler: &WasmHandler) {
        match self {
            Self::Ping => handler.ping(()).await.expect("failed to call handler"),
            Self::Greet => {
                let greeting = handler
                    .greet((), "test".into())
                    .await
                    .expect("failed to call handler");
                assert_eq!(greeting, "Hello, test");
            }
        }
    }
}

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

    pub fn new(wasm: &[u8]) -> anyhow::Result<Self> {
        let mut opts = wasmtime_cli_flags::CommonOptions::new();
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

struct WebSocketsClient {
    addr: SocketAddr,
    builder: wrpc_websockets::ClientBuilder<'static>,
}

impl Invoke for WebSocketsClient {
    type Context = ();

    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Outgoing, Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = wrpc_test::tcp_connect(self.addr).await?;
        stream
            .set_zero_linger()
            .context("failed to set zero linger")?;
        let (ws, _) = self
            .builder
            .connect_on(stream)
            .await
            .context("failed to establish WebSocket connection")?;
        let (tx, rx) = wrpc_websockets::split(ws);
        wrpc_transport::frame::invoke(tx, rx, instance, func, params, paths).await
    }
}

#[derive(Clone)]
struct Tasks {
    handle: Handle,
    errors: Arc<Mutex<Vec<anyhow::Error>>>,
}

impl Tasks {
    fn new(rt: &Runtime) -> Self {
        Self {
            handle: rt.handle().clone(),
            errors: Arc::default(),
        }
    }

    fn spawn(
        &self,
        fut: impl Future<Output = anyhow::Result<()>> + Send + 'static,
    ) -> JoinHandle<()> {
        let errors = Arc::clone(&self.errors);
        self.handle.spawn(async move {
            if let Err(err) = fut.await {
                errors
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(err);
            }
        })
    }

    fn check(&self) -> anyhow::Result<()> {
        let mut errors = self.errors.lock().unwrap_or_else(PoisonError::into_inner);
        match errors.drain(..).next() {
            Some(err) => Err(err.context("server task failed")),
            None => Ok(()),
        }
    }
}

fn spawn_invocations<S: Serve>(
    rt: &Runtime,
    tasks: &Tasks,
    srv: &S,
    func: Func,
    handler: impl Handler<S::Context>,
) -> anyhow::Result<()> {
    let mut invocations = rt
        .block_on(func.serve(srv, handler))
        .context("failed to serve bindings")?;
    tasks.spawn({
        let tasks = tasks.clone();
        async move {
            while let Some(fut) = invocations.next().await {
                let fut = fut.context("failed to accept invocation")?;
                tasks.spawn(async move { fut.await.context("failed to handle invocation") });
            }
            Ok(())
        }
    });
    Ok(())
}

fn bench_invoke<C: Invoke>(
    g: &mut BenchmarkGroup<impl Measurement>,
    rt: &Runtime,
    tasks: &Tasks,
    transport: &str,
    func: Func,
    clt: &C,
    cx: impl Fn() -> C::Context,
) -> anyhow::Result<()> {
    g.bench_function(transport, |b| {
        b.to_async(rt).iter(|| async {
            if let Err(err) = func.call(clt, cx()).await {
                panic!("{:?}", tasks.check().err().unwrap_or(err));
            }
        });
    });
    tasks.check()
}

fn bench_direct(
    g: &mut BenchmarkGroup<impl Measurement>,
    func: Func,
    handler: &WasmHandler,
) -> anyhow::Result<()> {
    let rt = Runtime::new().context("failed to build Tokio runtime")?;
    g.bench_function("direct", |b| {
        b.to_async(&rt).iter(|| func.call_direct(handler));
    });
    Ok(())
}

fn bench_tcp(
    g: &mut BenchmarkGroup<impl Measurement>,
    func: Func,
    handler: impl Handler<()>,
) -> anyhow::Result<()> {
    let rt = Runtime::new().context("failed to build Tokio runtime")?;
    let lis = rt.block_on(wrpc_test::tcp_bind())?;
    let addr = lis
        .local_addr()
        .context("failed to query listener address")?;
    let srv = Arc::new(wrpc_transport::frame::Server::default());
    let tasks = Tasks::new(&rt);
    spawn_invocations(&rt, &tasks, srv.as_ref(), func, handler)?;
    tasks.spawn({
        let tasks = tasks.clone();
        async move {
            loop {
                let (stream, _) = lis.accept().await.context("failed to accept connection")?;
                stream
                    .set_nodelay(true)
                    .context("failed to set TCP_NODELAY")?;
                let srv = Arc::clone(&srv);
                tasks.spawn(async move {
                    let (rx, tx) = stream.into_split();
                    srv.accept((), tx, BufReader::new(rx))
                        .await
                        .context("failed to accept invocation")
                });
            }
        }
    });
    bench_invoke(
        g,
        &rt,
        &tasks,
        "tcp",
        func,
        &wrpc_transport::frame::tcp::Client::from(addr).set_zero_linger(),
        || (),
    )
}

fn bench_unix(
    g: &mut BenchmarkGroup<impl Measurement>,
    func: Func,
    handler: impl Handler<()>,
) -> anyhow::Result<()> {
    let rt = Runtime::new().context("failed to build Tokio runtime")?;
    let dir = tempfile::tempdir().context("failed to create temporary directory")?;
    let path = dir.path().join("wrpc.sock");
    let lis = {
        let _rt = rt.enter();
        UnixListener::bind(&path).context("failed to bind Unix listener")?
    };
    let srv = Arc::new(wrpc_transport::frame::Server::default());
    let tasks = Tasks::new(&rt);
    spawn_invocations(&rt, &tasks, srv.as_ref(), func, handler)?;
    tasks.spawn({
        let tasks = tasks.clone();
        async move {
            loop {
                let (stream, _) = lis.accept().await.context("failed to accept connection")?;
                let srv = Arc::clone(&srv);
                tasks.spawn(async move {
                    let (rx, tx) = stream.into_split();
                    srv.accept((), tx, BufReader::new(rx))
                        .await
                        .context("failed to accept invocation")
                });
            }
        }
    });
    let clt = wrpc_transport::frame::unix::Client::from(path);
    bench_invoke(g, &rt, &tasks, "unix", func, &clt, || ())
}

fn bench_quic(
    g: &mut BenchmarkGroup<impl Measurement>,
    func: Func,
    handler: impl Handler<()>,
) -> anyhow::Result<()> {
    let rt = Runtime::new().context("failed to build Tokio runtime")?;
    let (clt, conn) = rt
        .block_on(wrpc_test::with_quic(async |clt, srv| Ok((clt, srv))))
        .context("failed to establish QUIC connection")?;
    let srv = Arc::new(wrpc_quic::Server::new());
    let tasks = Tasks::new(&rt);
    spawn_invocations(&rt, &tasks, srv.as_ref(), func, handler)?;
    let accept = tasks.spawn({
        let tasks = tasks.clone();
        async move {
            loop {
                let (tx, rx) = conn.accept_bi().await.context("failed to accept stream")?;
                let srv = Arc::clone(&srv);
                tasks.spawn(async move {
                    srv.accept((), tx, BufReader::new(rx))
                        .await
                        .context("failed to accept invocation")
                });
            }
        }
    });
    let clt = wrpc_quic::Client::from(clt);
    let res = bench_invoke(g, &rt, &tasks, "quic", func, &clt, || ());
    accept.abort();
    _ = rt.block_on(accept);
    res
}

fn bench_webtransport(
    g: &mut BenchmarkGroup<impl Measurement>,
    func: Func,
    handler: impl Handler<()>,
) -> anyhow::Result<()> {
    let rt = Runtime::new().context("failed to build Tokio runtime")?;
    let (clt, conn) = rt
        .block_on(wrpc_test::with_webtransport(async |clt, srv| {
            Ok((clt, srv))
        }))
        .context("failed to establish WebTransport session")?;
    let srv = Arc::new(wrpc_webtransport::Server::new());
    let tasks = Tasks::new(&rt);
    spawn_invocations(&rt, &tasks, srv.as_ref(), func, handler)?;
    let accept = tasks.spawn({
        let tasks = tasks.clone();
        async move {
            loop {
                let (tx, rx) = conn.accept_bi().await.context("failed to accept stream")?;
                let srv = Arc::clone(&srv);
                tasks.spawn(async move {
                    srv.accept((), tx, BufReader::new(rx))
                        .await
                        .context("failed to accept invocation")
                });
            }
        }
    });
    let clt = wrpc_webtransport::Client::from(clt);
    let res = bench_invoke(g, &rt, &tasks, "webtransport", func, &clt, || ());
    accept.abort();
    _ = rt.block_on(accept);
    res
}

fn bench_websockets(
    g: &mut BenchmarkGroup<impl Measurement>,
    func: Func,
    handler: impl Handler<()>,
) -> anyhow::Result<()> {
    let rt = Runtime::new().context("failed to build Tokio runtime")?;
    let (clt, ws, lis) = rt
        .block_on(wrpc_test::with_websockets(async |clt, srv, lis| {
            Ok((clt, Arc::new(srv), lis))
        }))
        .context("failed to set up WebSockets")?;
    let addr = lis
        .local_addr()
        .context("failed to query listener address")?;
    let srv = Arc::new(wrpc_transport::frame::Server::default());
    let tasks = Tasks::new(&rt);
    spawn_invocations(&rt, &tasks, srv.as_ref(), func, handler)?;
    tasks.spawn({
        let tasks = tasks.clone();
        async move {
            loop {
                let (stream, _) = lis.accept().await.context("failed to accept connection")?;
                stream
                    .set_nodelay(true)
                    .context("failed to set TCP_NODELAY")?;
                let srv = Arc::clone(&srv);
                let ws = Arc::clone(&ws);
                tasks.spawn(async move {
                    let (_, ws) = ws
                        .accept(stream)
                        .await
                        .context("failed to perform WebSocket handshake")?;
                    let (tx, rx) = wrpc_websockets::split(ws);
                    srv.accept((), tx, rx)
                        .await
                        .context("failed to accept invocation")
                });
            }
        }
    });
    let clt = WebSocketsClient { addr, builder: clt };
    bench_invoke(g, &rt, &tasks, "websockets", func, &clt, || ())
}

fn bench_http1(
    g: &mut BenchmarkGroup<impl Measurement>,
    func: Func,
    handler: impl Handler<http::request::Parts>,
) -> anyhow::Result<()> {
    let rt = Runtime::new().context("failed to build Tokio runtime")?;
    let (parts, clt, lis) =
        rt.block_on(wrpc_test::with_http_pooling(async |parts, clt, lis| {
            Ok((parts, clt, lis))
        }))?;
    let srv = Arc::new(wrpc_transport::frame::Server::default());
    let tasks = Tasks::new(&rt);
    spawn_invocations(&rt, &tasks, srv.as_ref(), func, handler)?;
    tasks.spawn({
        let tasks = tasks.clone();
        async move {
            loop {
                let (stream, _) = lis.accept().await.context("failed to accept connection")?;
                stream
                    .set_nodelay(true)
                    .context("failed to set TCP_NODELAY")?;
                let conn = hyper::server::conn::http1::Builder::new().serve_connection(
                    WithHyperIo::new(stream),
                    wrpc_http::Server::from(Arc::clone(&srv)),
                );
                tasks.spawn(async move { conn.await.context("failed to serve HTTP/1 connection") });
            }
        }
    });
    let clt = wrpc_http::Client::from(clt);
    bench_invoke(g, &rt, &tasks, "http1", func, &clt, || parts.clone())
}

fn bench_http2(
    g: &mut BenchmarkGroup<impl Measurement>,
    func: Func,
    handler: impl Handler<http::request::Parts>,
) -> anyhow::Result<()> {
    let rt = Runtime::new().context("failed to build Tokio runtime")?;
    let lis = rt.block_on(wrpc_test::tcp_bind())?;
    let addr = lis
        .local_addr()
        .context("failed to query listener address")?;
    let ((sender, conn), (stream, _)) = rt
        .block_on(async {
            tokio::try_join!(
                async {
                    let stream = wrpc_test::tcp_connect(addr).await?;
                    hyper::client::conn::http2::handshake(
                        TokioExecutor::default(),
                        WithHyperIo::new(stream),
                    )
                    .await
                    .context("failed to establish HTTP/2 connection")
                },
                async { lis.accept().await.context("failed to accept connection") },
            )
        })
        .context("failed to connect over HTTP/2")?;
    stream.set_nodelay(true)?;
    let srv = wrpc_http::Server::default();
    let tasks = Tasks::new(&rt);
    tasks.spawn(async move {
        conn.await
            .context("failed to drive HTTP/2 client connection")
    });
    spawn_invocations(&rt, &tasks, srv.as_ref(), func, handler)?;
    let conn = hyper::server::conn::http2::Builder::new(TokioExecutor::default())
        .serve_connection(WithHyperIo::new(stream), srv);
    tasks.spawn(async move { conn.await.context("failed to serve HTTP/2 connection") });
    let (parts, ()) = http::Request::new(()).into_parts();
    let clt = wrpc_http::Client::from(sender);
    bench_invoke(g, &rt, &tasks, "http2", func, &clt, || parts.clone())
}

fn bench_transports<H>(
    g: &mut BenchmarkGroup<impl Measurement>,
    func: Func,
    handler: &H,
) -> anyhow::Result<()>
where
    H: Handler<()> + Handler<http::request::Parts>,
{
    bench_tcp(g, func, handler.clone())?;
    bench_unix(g, func, handler.clone())?;
    bench_quic(g, func, handler.clone())?;
    bench_webtransport(g, func, handler.clone())?;
    bench_websockets(g, func, handler.clone())?;
    bench_http1(g, func, handler.clone())?;
    bench_http2(g, func, handler.clone())?;
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
    let wasm = WasmHandler::new(&reactor).context("failed to construct a Wasm handler")?;
    for (name, func) in [("ping", Func::Ping), ("greet", Func::Greet)] {
        let mut g = c.benchmark_group(format!("wasm-{name}"));
        bench_direct(&mut g, func, &wasm)?;
        bench_transports(&mut g, func, &wasm)?;
        g.finish();
    }
    for (name, func) in [("ping", Func::Ping), ("greet", Func::Greet)] {
        let mut g = c.benchmark_group(name);
        bench_transports(&mut g, func, &NativeHandler)?;
        g.finish();
    }
    c.final_summary();
    Ok(())
}
