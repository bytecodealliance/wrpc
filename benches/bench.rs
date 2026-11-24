use core::pin::Pin;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use anyhow::{Context as _, anyhow, ensure};
use bytes::Bytes;
use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion};
use futures::TryFutureExt as _;
use http::request::Parts;
use http_body_util::BodyExt;
use http_body_util::combinators::UnsyncBoxBody;
use hyper::body::Incoming;
use hyper::client::conn::http2::SendRequest;
use hyper_util::client::legacy::ResponseFuture;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use hyper_util::rt::tokio::WithHyperIo;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpStream, UnixListener};
use tokio::runtime::{Handle, Runtime};
use tokio::task::JoinHandle;
use tokio::try_join;
use tokio_util::codec::{Decoder, Encoder};
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wrpc_core::{Codec, Encode, Framed, Header, Invoke, Listener, Receive, Serve, Transmit};
use wrpc_http::{Body, OutgoingRequest};

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

trait Handler:
    ping_bindings_wrpc::exports::wrpc_bench::bench::ping::Handler
    + greet_bindings_wrpc::exports::wrpc_bench::bench::greet::Handler
    + Clone
    + Send
    + Sync
    + 'static
{
}

impl<T> Handler for T where
    T: ping_bindings_wrpc::exports::wrpc_bench::bench::ping::Handler
        + greet_bindings_wrpc::exports::wrpc_bench::bench::greet::Handler
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
    async fn call<C>(self, clt: C) -> anyhow::Result<()>
    where
        C: Invoke<<() as Encode>::Encoder> + Invoke<<(&'static str,) as Encode>::Encoder>,
    {
        match self {
            Self::Ping => ping_bindings_wrpc::wrpc_bench::bench::ping::ping(clt)
                .await
                .context("failed to call `ping`"),
            Self::Greet => {
                let greeting = greet_bindings_wrpc::wrpc_bench::bench::greet::greet(clt, "test")
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

    fn handle<'a, S: Serve + Send + 'a>(
        self,
        handler: &impl Handler,
        instance: &str,
        name: &str,
        srv: S,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<S::Output>> + Send + 'a>> {
        let res = match self {
            Self::Ping => ping_bindings_wrpc::handle(handler, instance, name, srv)
                .map(|fut| Box::pin(fut.err_into()) as Pin<Box<dyn Future<Output = _> + Send>>),
            Self::Greet => greet_bindings_wrpc::handle(handler, instance, name, srv)
                .map(|fut| Box::pin(fut.err_into()) as Pin<Box<dyn Future<Output = _> + Send>>),
        };
        res.unwrap_or_else(|_| {
            let err = anyhow!("`{instance}#{name}` is not exported");
            Box::pin(async { Err(err) })
        })
    }

    async fn call_direct(self, handler: &WasmHandler) {
        match self {
            Self::Ping => handler.ping().await.expect("failed to call handler"),
            Self::Greet => {
                let greeting = handler
                    .greet("test".into())
                    .await
                    .expect("failed to call handler");
                assert_eq!(greeting, "Hello, test");
            }
        }
    }
}

#[derive(Clone, Copy)]
struct NativeHandler;

impl ping_bindings_wrpc::exports::wrpc_bench::bench::ping::Handler for NativeHandler {
    async fn ping(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl greet_bindings_wrpc::exports::wrpc_bench::bench::greet::Handler for NativeHandler {
    async fn greet(&self, name: String) -> anyhow::Result<String> {
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
        let ty = wasmtime::MemoryType::new64(0, Some(1 << (BITS_TO_TEST - 16)))?;
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

impl ping_bindings_wrpc::exports::wrpc_bench::bench::ping::Handler for WasmHandler {
    async fn ping(&self) -> anyhow::Result<()> {
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

impl greet_bindings_wrpc::exports::wrpc_bench::bench::greet::Handler for WasmHandler {
    async fn greet(&self, name: String) -> anyhow::Result<String> {
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

    fn serve(
        &self,
        func: Func,
        tx: impl AsyncWrite + Send + Unpin + 'static,
        rx: impl AsyncRead + Send + Unpin + 'static,
        handler: impl Handler,
    ) {
        self.spawn(async move {
            let mut lis = Listener::new(tx, rx);
            while let Some((Header { instance, name }, srv)) = lis.accept().await? {
                func.handle(&handler, &instance, &name, srv).await?;
            }
            Ok(())
        });
    }

    fn check(&self) -> anyhow::Result<()> {
        let mut errors = self.errors.lock().unwrap_or_else(PoisonError::into_inner);
        match errors.drain(..).next() {
            Some(err) => Err(err.context("server task failed")),
            None => Ok(()),
        }
    }
}

/// Client invoking functions with HTTP/1 requests sent by a pooling client
struct Http1<'a> {
    clt:
        &'a hyper_util::client::legacy::Client<HttpConnector, UnsyncBoxBody<Bytes, std::io::Error>>,
    parts: Parts,
}

impl<'a, E: Transmit + Send + 'static> Invoke<E> for Http1<'a> {
    type Invocation<D: Receive> = wrpc_http::Invocation<ResponseFuture, Incoming, D>;

    fn invoke<D, T>(
        self,
        codec: Codec<E, D>,
        instance: &str,
        func: &str,
        params: T,
    ) -> impl Future<Output = std::io::Result<(D::Item, Self::Invocation<D>)>> + use<'a, E, D, T>
    where
        E: Encoder<T, Error = std::io::Error>,
        D: Decoder<Error = std::io::Error> + Receive,
    {
        let Self { clt, parts } = self;
        OutgoingRequest {
            parts,
            send: |req: http::Request<Body<E>>| clt.request(req.map(BodyExt::boxed_unsync)),
        }
        .invoke(codec, instance, func, params)
    }
}

/// Client invoking functions with HTTP/2 requests sent on a connection
struct Http2<'a> {
    sender: &'a mut SendRequest<UnsyncBoxBody<Bytes, std::io::Error>>,
    parts: Parts,
}

impl<'a, E: Transmit + Send + 'static> Invoke<E> for Http2<'a> {
    type Invocation<D: Receive> = wrpc_http::Invocation<
        Pin<Box<dyn Future<Output = hyper::Result<http::Response<Incoming>>> + Send + 'a>>,
        Incoming,
        D,
    >;

    fn invoke<D, T>(
        self,
        codec: Codec<E, D>,
        instance: &str,
        func: &str,
        params: T,
    ) -> impl Future<Output = std::io::Result<(D::Item, Self::Invocation<D>)>> + use<'a, E, D, T>
    where
        E: Encoder<T, Error = std::io::Error>,
        D: Decoder<Error = std::io::Error> + Receive,
    {
        let Self { sender, parts } = self;
        OutgoingRequest {
            parts,
            send: |req: http::Request<Body<E>>| {
                Box::pin(async move {
                    sender.ready().await?;
                    sender.send_request(req.map(BodyExt::boxed_unsync)).await
                }) as Pin<Box<dyn Future<Output = _> + Send>>
            },
        }
        .invoke(codec, instance, func, params)
    }
}

fn bench_invoke(
    g: &mut BenchmarkGroup<WallTime>,
    rt: &Runtime,
    tasks: &Tasks,
    transport: &str,
    func: Func,
    connect: impl Future<
        Output = anyhow::Result<(
            impl AsyncWrite + Send + Unpin,
            impl AsyncRead + Send + Unpin,
        )>,
    >,
) -> anyhow::Result<()> {
    let (tx, rx) = rt.block_on(connect)?;
    let mut clt = Framed::new(tx, rx);
    bench_calls(g, rt, tasks, transport, async || func.call(&mut clt).await)
}

fn bench_calls(
    g: &mut BenchmarkGroup<WallTime>,
    rt: &Runtime,
    tasks: &Tasks,
    transport: &str,
    mut call: impl AsyncFnMut() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    g.bench_function(transport, |b| {
        b.iter_custom(|n| {
            rt.block_on(async {
                let start = Instant::now();
                for _ in 0..n {
                    if let Err(err) = call().await {
                        panic!("{:?}", tasks.check().err().unwrap_or(err));
                    }
                }
                start.elapsed()
            })
        });
    });
    tasks.check()
}

fn bench_direct(
    g: &mut BenchmarkGroup<WallTime>,
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
    g: &mut BenchmarkGroup<WallTime>,
    func: Func,
    handler: impl Handler,
) -> anyhow::Result<()> {
    let rt = Runtime::new().context("failed to build Tokio runtime")?;
    let lis = rt.block_on(wrpc_test::tcp_bind())?;
    let addr = lis
        .local_addr()
        .context("failed to query listener address")?;
    let tasks = Tasks::new(&rt);
    tasks.spawn({
        let tasks = tasks.clone();
        async move {
            loop {
                let (tx, rx) = wrpc_test::tcp_accept(&lis).await?;
                tasks.serve(func, tx, rx, handler.clone());
            }
        }
    });
    bench_invoke(g, &rt, &tasks, "tcp", func, async {
        let stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;
        let (rx, tx) = stream.into_split();
        Ok((tx, rx))
    })
}

fn bench_unix(
    g: &mut BenchmarkGroup<WallTime>,
    func: Func,
    handler: impl Handler,
) -> anyhow::Result<()> {
    let rt = Runtime::new().context("failed to build Tokio runtime")?;
    let dir = tempfile::tempdir().context("failed to create temporary directory")?;
    let path = dir.path().join("wrpc.sock");
    let lis = {
        let _rt = rt.enter();
        UnixListener::bind(&path).context("failed to bind Unix listener")?
    };
    let tasks = Tasks::new(&rt);
    tasks.spawn({
        let tasks = tasks.clone();
        async move {
            loop {
                let (tx, rx) = wrpc_test::unix_accept(&lis).await?;
                tasks.serve(func, tx, rx, handler.clone());
            }
        }
    });
    bench_invoke(g, &rt, &tasks, "unix", func, async {
        let (tx, rx) = wrpc_test::unix_streams(path.clone()).await?;
        Ok((tx, rx))
    })
}

fn bench_quic(
    g: &mut BenchmarkGroup<WallTime>,
    func: Func,
    handler: impl Handler,
) -> anyhow::Result<()> {
    let rt = Runtime::new().context("failed to build Tokio runtime")?;
    let (clt, conn) = rt
        .block_on(wrpc_test::with_quic(async |clt, srv| Ok((clt, srv))))
        .context("failed to establish QUIC connection")?;
    let tasks = Tasks::new(&rt);
    let accept = tasks.spawn({
        let tasks = tasks.clone();
        async move {
            loop {
                let (tx, rx) = conn.accept_bi().await?;
                tasks.serve(func, tx, rx, handler.clone());
            }
        }
    });
    let res = bench_invoke(g, &rt, &tasks, "quic", func, async {
        let (tx, rx) = clt.open_bi().await?;
        Ok((tx, rx))
    });
    accept.abort();
    _ = rt.block_on(accept);
    res
}

fn bench_webtransport(
    g: &mut BenchmarkGroup<WallTime>,
    func: Func,
    handler: impl Handler,
) -> anyhow::Result<()> {
    let rt = Runtime::new().context("failed to build Tokio runtime")?;
    let (clt, conn) = rt
        .block_on(wrpc_test::with_webtransport(async |clt, srv| {
            Ok((clt, srv))
        }))
        .context("failed to establish WebTransport session")?;
    let tasks = Tasks::new(&rt);
    let accept = tasks.spawn({
        let tasks = tasks.clone();
        async move {
            loop {
                let (tx, rx) = conn.accept_bi().await?;
                tasks.serve(func, tx, rx, handler.clone());
            }
        }
    });
    let res = bench_invoke(g, &rt, &tasks, "webtransport", func, async {
        let (tx, rx) = clt.open_bi().await?;
        Ok((tx, rx))
    });
    accept.abort();
    _ = rt.block_on(accept);
    res
}

fn bench_http1(
    g: &mut BenchmarkGroup<WallTime>,
    func: Func,
    handler: impl Handler,
) -> anyhow::Result<()> {
    let rt = Runtime::new().context("failed to build Tokio runtime")?;
    let (parts, clt, lis) =
        rt.block_on(wrpc_test::with_http_pooling(async |parts, clt, lis| {
            Ok((parts, clt, lis))
        }))?;
    let tasks = Tasks::new(&rt);
    tasks.spawn({
        let tasks = tasks.clone();
        async move {
            loop {
                let (stream, _) = lis.accept().await.context("failed to accept connection")?;
                stream
                    .set_nodelay(true)
                    .context("failed to set TCP_NODELAY")?;
                let handler = handler.clone();
                tasks.spawn(wrpc_test::serve_http(
                    stream,
                    move |_, Header { instance, name }, srv| {
                        func.handle(&handler, &instance, &name, srv)
                    },
                ));
            }
        }
    });
    bench_calls(g, &rt, &tasks, "http1", async || {
        func.call(Http1 {
            clt: &clt,
            parts: parts.clone(),
        })
        .await
    })
}

fn bench_http2(
    g: &mut BenchmarkGroup<WallTime>,
    func: Func,
    handler: impl Handler,
) -> anyhow::Result<()> {
    let rt = Runtime::new().context("failed to build Tokio runtime")?;
    let lis = rt.block_on(wrpc_test::tcp_bind())?;
    let addr = lis
        .local_addr()
        .context("failed to query listener address")?;
    let ((mut sender, conn), (stream, ..)) = rt
        .block_on(async {
            try_join!(
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
    let tasks = Tasks::new(&rt);
    tasks.spawn(async move {
        conn.await
            .context("failed to drive HTTP/2 client connection")
    });
    tasks.spawn(async move {
        wrpc_test::serve_http(stream, move |_, Header { instance, name }, srv| {
            func.handle(&handler, &instance, &name, srv)
        })
        .await
        .context("failed to serve HTTP/2 connection")
    });
    let (parts, ()) = http::Request::new(()).into_parts();
    bench_calls(g, &rt, &tasks, "http2", async || {
        func.call(Http2 {
            sender: &mut sender,
            parts: parts.clone(),
        })
        .await
    })
}

fn bench_transports(
    g: &mut BenchmarkGroup<WallTime>,
    func: Func,
    handler: &impl Handler,
) -> anyhow::Result<()> {
    bench_tcp(g, func, handler.clone())?;
    bench_unix(g, func, handler.clone())?;
    bench_quic(g, func, handler.clone())?;
    bench_webtransport(g, func, handler.clone())?;
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
