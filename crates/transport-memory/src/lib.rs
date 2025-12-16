//! wRPC in-memory transport for same-process component communication
//!
//! This transport allows components running in the same process to communicate
//! via in-memory bidirectional streams, without requiring network connections.
//!
//! The main use case is for a "host" component that orchestrates multiple "child" components.
//! Instead of each child requiring its own network connection, you can use in-memory streams
//! to route calls internally.

use anyhow::Context as _;
use bytes::Bytes;
use core::pin::Pin;
use tokio::io::{AsyncReadExt as _, SimplexStream, simplex};
use tokio::sync::mpsc;
use wrpc_transport::frame::{invoke, Accept, Incoming, Outgoing};
use wrpc_transport::Server as TransportServer;
use wrpc_transport::{Invoke, Serve};

/// In-memory wRPC client
/// It routes invocations to a per-component `Server` via in-memory streams.
#[derive(Clone, Debug)]
pub struct Client {
    server_tx: mpsc::UnboundedSender<(tokio::io::WriteHalf<SimplexStream>, tokio::io::ReadHalf<SimplexStream>)>,
}

/// In-memory connection listener
///
/// In our model, the in-memory Client creates new io streams *per* request (per `invoke`)
/// These are received by the listener, to be accepted by the server later
#[derive(Clone, Debug)]
pub struct Listener {
    rx: std::sync::Arc<
        tokio::sync::Mutex<
            mpsc::UnboundedReceiver<(tokio::io::WriteHalf<SimplexStream>, tokio::io::ReadHalf<SimplexStream>)>,
        >,
    >,
}

impl Accept for Listener {
    type Context = ();
    type Outgoing = tokio::io::WriteHalf<SimplexStream>;
    type Incoming = tokio::io::ReadHalf<SimplexStream>;

    async fn accept(&self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        let mut rx = self.rx.lock().await;
        rx.recv()
            .await
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "channel closed"))
            .map(|(tx, rx)| ((), tx, rx))
    }
}

impl Accept for &Listener {
    type Context = ();
    type Outgoing = tokio::io::WriteHalf<SimplexStream>;
    type Incoming = tokio::io::ReadHalf<SimplexStream>;

    async fn accept(&self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        (*self).accept().await
    }
}

/// In-memory wRPC server that handles invocations.
pub struct Server {
    /// `wrpc_transport::Server`, but using an in-memory `tokio::io::SimplexStream` for input and output
    transport_server: TransportServer<(), tokio::io::ReadHalf<SimplexStream>, tokio::io::WriteHalf<SimplexStream>>,
    /// Queued streams created by client invocations, waiting to be accepted
    listener: Listener,
}

impl Server {
    /// Accept a single connection (ex: a single function call) from the listener.
    ///
    /// This processes one connection and routes it to registered handlers.
    pub async fn accept(&self) -> anyhow::Result<()> {
        self.transport_server
            .accept(&self.listener)
            .await
            .map_err(|err| anyhow::anyhow!("failed to accept connection: {err}"))
    }

    /// Start accepting connections in a loop.
    ///
    /// This should be called in a background task. It will continuously accept
    /// connections from the listener and route them to registered handlers.
    pub async fn accept_loop(&self) -> anyhow::Result<()> {
        loop {
            self.accept().await?;
        }
    }
}

/// Create a new client-server pair for in-memory communication.
///
/// Returns `(client, server)` where:
/// - `client` implements `Invoke` and can be used in a WASM Component's Store's context
/// - `server` implements `Serve` and can be used to register handlers
///
/// You should spawn a task that calls `server.accept_loop()` to start processing connections.
pub fn new_memory_transport() -> (Client, Server) {
    // tx → Client
    // rx → Listener (which Server reads from when accepting connections)
    let (tx, rx) = mpsc::unbounded_channel();
    (
        Client { server_tx: tx },
        Server {
            transport_server: TransportServer::new(),
            listener: Listener {
                rx: std::sync::Arc::new(tokio::sync::Mutex::new(rx)),
            },
        },
    )
}

impl Invoke for Client {
    type Context = ();
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        // Create bidirectional in-memory streams
        // Client -> Server: client writes to client_tx, server reads from server_rx
        // Server -> Client: server writes to server_tx, client reads from client_rx
        let (server_rx, client_tx, ) = simplex(65536);
        let (client_rx, server_tx) = simplex(65536);

        // Set up wRPC framing on client side (writes invocation header + params)
        let (client_outgoing, client_incoming) =
            invoke(client_tx, client_rx, instance, func, params, paths)
                .await
                .context("failed to set up client invocation")?;

        // Send raw server-side streams to the listener
        // The Server will use Accept to get these, read the header, and set up framing via serve()
        self.server_tx
            .send((server_tx, server_rx))
            .map_err(|_| anyhow::anyhow!("server channel closed"))?;

        Ok((client_outgoing, client_incoming))
    }
}

impl Serve for Server {
    type Context = ();
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: impl Into<std::sync::Arc<[Box<[Option<usize>]>]>> + Send,
    ) -> anyhow::Result<
        impl futures::Stream<Item = anyhow::Result<(Self::Context, Self::Outgoing, Self::Incoming)>>
            + Send
            + 'static,
    > {
        // Delegate to the transport_server's serve method
        // It will handle routing connections based on instance/func by reading the header
        self.transport_server.serve(instance, func, paths).await
    }
}

/// Extension trait for `Incoming` streams that provides methods to read raw bytes.
/// Using the component model value definition encoding
pub trait IncomingExt {
    /// Read all raw bytes from the incoming stream.
    ///
    /// This reads the entire value as raw bytes (which represent the result encoded using the component model value definition encoding)
    /// Useful if you intend to forward the result (ex: over another stream) without processing it yourself
    /// The stream is consumed after this call.
    ///
    /// # Example
    ///
    /// ```no_run
    /// let (mut outgoing, mut incoming) = client.invoke((), "", "func", bytes::Bytes::new(), &[]).await?;
    /// outgoing.flush().await?;
    /// let raw_bytes = incoming.read_raw_bytes().await?;
    /// // raw_bytes is Vec<u8> containing the component model encoded value
    /// ```
    fn read_raw_bytes(
        &mut self,
    ) -> impl std::future::Future<Output = anyhow::Result<Vec<u8>>> + Send;
}

impl IncomingExt for Incoming {
    async fn read_raw_bytes(&mut self) -> anyhow::Result<Vec<u8>> {
        let mut buf = Vec::new();
        Pin::new(self)
            .read_to_end(&mut buf)
            .await
            .context("failed to read raw bytes from incoming stream")?;
        Ok(buf)
    }
}

/// Extension trait for `Incoming` streams that provides methods to decode to wasmtime `Val` types.
///
/// This trait is available when the `wasmtime` feature is enabled
#[cfg(any(feature = "wasmtime", test))]
pub trait IncomingValExt {
    /// Decode a value from the incoming stream into a wasmtime `Val`.
    ///
    /// Useful if you intend to use the result directly in wasmtime without the need for Rust primitive types
    /// This requires the component type information to properly decode the value.
    /// The stream is consumed after this call.
    ///
    /// # Arguments
    ///
    /// * `store` - A mutable store context for decoding resources
    /// * `ty` - The component model type of the value to decode
    /// * `resources` - Resource types for handling resource values
    /// * `path` - The path to the value (typically `&[]` for root return values)
    ///
    /// # Example
    ///
    /// ```no_run
    /// # #[cfg(feature = "wasmtime")]
    /// let (mut outgoing, mut incoming) = client.invoke((), "", "func", bytes::Bytes::new(), &[]).await?;
    /// outgoing.flush().await?;
    /// let val = incoming.read_value(&mut store, &Type::S64, &[], &[]).await?;
    /// //   ^ val is `wasmtime::component::Val`
    /// ```
    fn read_value<T>(
        &mut self,
        store: &mut impl wasmtime::AsContextMut<Data = T>,
        ty: &wasmtime::component::Type,
        resources: &[wasmtime::component::ResourceType],
        path: &[usize],
    ) -> impl std::future::Future<Output = anyhow::Result<wasmtime::component::Val>>
    where
        T: wrpc_runtime_wasmtime::WrpcView + 'static;
}

#[cfg(any(feature = "wasmtime", test))]
impl IncomingValExt for Incoming {
    async fn read_value<T>(
        &mut self,
        store: &mut impl wasmtime::AsContextMut<Data = T>,
        ty: &wasmtime::component::Type,
        guest_resources: &[wasmtime::component::ResourceType],
        path: &[usize],
    ) -> anyhow::Result<wasmtime::component::Val>
    where
        T: wrpc_runtime_wasmtime::WrpcView + 'static,
    {
        use wrpc_runtime_wasmtime::read_value;
        let mut val = wasmtime::component::Val::Bool(false);
        let mut incoming = Pin::new(self);
        read_value(store, &mut incoming, guest_resources, &mut val, ty, path)
            .await
            .context("failed to decode value from incoming stream")?;
        Ok(val)
    }
}

#[cfg(test)]
mod tests {
    use core::pin::pin;
    use futures::StreamExt;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::sync::Mutex;
    use tokio::task::JoinHandle;
    use wasm_tokio::AsyncReadLeb128 as _;
    use wasmtime::component::ResourceTable;
    use wasmtime::component::{Component, Instance, Linker};
    use wasmtime::Engine;
    use wasmtime::Store;
    use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
    use wrpc_runtime_wasmtime::{
        collect_component_functions, collect_component_resource_exports, ServeExt,
        SharedResourceTable, WrpcCtxView, WrpcView,
    };
    use wrpc_transport::Invoke;

    use super::{IncomingValExt, *};

    pub struct WrpcCtx<C: Invoke> {
        pub wrpc: C,
        pub cx: C::Context,
        pub shared_resources: SharedResourceTable,
        pub timeout: Duration,
    }

    pub struct Ctx<C: Invoke> {
        pub table: ResourceTable,
        pub wasi: WasiCtx,
        pub wrpc: WrpcCtx<C>,
    }

    impl<C> wrpc_runtime_wasmtime::WrpcCtx<C> for WrpcCtx<C>
    where
        C: Invoke,
        C::Context: Clone,
    {
        fn context(&self) -> C::Context {
            self.cx.clone()
        }

        fn client(&self) -> &C {
            &self.wrpc
        }

        fn shared_resources(&mut self) -> &mut SharedResourceTable {
            &mut self.shared_resources
        }

        fn timeout(&self) -> Option<Duration> {
            Some(self.timeout)
        }
    }

    impl<C> WrpcView for Ctx<C>
    where
        C: Invoke,
        C::Context: Clone,
    {
        type Invoke = C;

        fn wrpc(&mut self) -> WrpcCtxView<'_, Self::Invoke> {
            WrpcCtxView {
                ctx: &mut self.wrpc,
                table: &mut self.table,
            }
        }
    }

    impl<C> WasiView for Ctx<C>
    where
        C: Invoke,
    {
        fn ctx(&mut self) -> WasiCtxView<'_> {
            WasiCtxView {
                ctx: &mut self.wasi,
                table: &mut self.table,
            }
        }
    }

    pub fn gen_ctx<C: Invoke>(wrpc: C, cx: C::Context) -> Ctx<C> {
        Ctx {
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().build(),
            wrpc: WrpcCtx {
                wrpc,
                cx,
                shared_resources: SharedResourceTable::default(),
                timeout: Duration::from_secs(10),
            },
        }
    }

    async fn gen_component(
        engine: &Engine,
        client: Client,
    ) -> anyhow::Result<(Component, Instance, Store<Ctx<Client>>)> {
        let component_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("no-args.wasm");
        let component_bytes = std::fs::read(&component_path)
            .with_context(|| format!("failed to read component from {:?}", component_path))?;

        let component =
            Component::new(engine, component_bytes).context("failed to parse component")?;

        let linker = Linker::new(engine);
        let ctx = gen_ctx(client, ());
        let mut store = Store::new(engine, ctx);

        let instance = linker
            .instantiate_async(&mut store, &component)
            .await
            .context("failed to instantiate component")?;

        Ok((component, instance, store))
    }

    async fn register_handlers(
        server: &Arc<Server>,
        component: &Component,
        instance: Instance,
        store: Store<Ctx<Client>>,
    ) -> anyhow::Result<Vec<JoinHandle<()>>> {
        // Collect guest resources
        let mut guest_resources_vec = Vec::new();
        collect_component_resource_exports(
            store.engine(),
            &component.component_type(),
            &mut guest_resources_vec,
        );
        let guest_resources = Arc::from(guest_resources_vec);

        // goal: register handlers for all exported functions (both at the root level, and (non-deeply) nested component instances)
        let mut invocation_handles = Vec::new();
        let component_ty = component.component_type();

        let mut functions = Vec::new();
        collect_component_functions(store.engine(), component_ty, &mut functions);

        let store_arc = Arc::new(Mutex::new(store));

        for (name, instance_name, ty) in functions {
            let pretty_name = if instance_name.is_empty() {
                "root".to_string()
            } else {
                instance_name.clone()
            };
            // note: all WASM Components instantiated (no `InstancePre`)
            // i.e. always use serve_function_shared (i.e. unlike `serve_stateless`)
            let invocations_stream = server
                .serve_function_shared(
                    Arc::clone(&store_arc),
                    instance,
                    Arc::clone(&guest_resources),
                    std::collections::HashMap::default(),
                    ty,
                    &instance_name,
                    &name,
                )
                .await
                .with_context(|| {
                    format!("failed to register handler for {pretty_name} function `{name}`")
                })?;

            // Spawn task to process invocations for this function
            let invocation_handle = tokio::spawn(async move {
                let mut invocations = pin!(invocations_stream);
                while let Some(invocation) = invocations.as_mut().next().await {
                    match invocation {
                        Ok((_, fut)) => {
                            if let Err(err) = fut.await {
                                eprintln!("failed to serve invocation for {pretty_name} function `{name}`: {err:?}");
                            }
                        }
                        Err(err) => {
                            eprintln!("failed to accept invocation for {pretty_name} function `{name}`: {err:?}");
                        }
                    }
                }
            });
            invocation_handles.push(invocation_handle);
        }

        Ok(invocation_handles)
    }

    #[test_log::test(tokio::test)]
    async fn test_transport() -> anyhow::Result<()> {
        let (client, server) = new_memory_transport();

        let mut config = wasmtime::Config::default();
        config.async_support(true);
        let engine = Engine::new(&config).context("failed to create engine with async support")?;

        let (component, instance, store) = gen_component(&engine, client.clone()).await?;

        let server_arc = Arc::new(server);
        // register handlers for all functions
        let invocation_handles =
            register_handlers(&server_arc, &component, instance, store).await?;

        // Spawn server accept loop (after registering handlers)
        let server_handle = tokio::spawn(async move {
            server_arc.accept_loop().await?;
            Ok::<(), anyhow::Error>(())
        });

        let func_name = "get-value";

        // Test 1: Read raw bytes (component model value definition encoding)
        {
            let params = bytes::Bytes::new();
            let paths: &[&[Option<usize>]] = &[];

            let (mut outgoing, mut incoming) = client
                .invoke((), "", func_name, params, paths)
                .await
                .context("failed to invoke function via wRPC")?;

            // Flush outgoing to ensure the request is sent
            outgoing
                .flush()
                .await
                .context("failed to flush outgoing stream")?;

            let raw_bytes = incoming
                .read_raw_bytes()
                .await
                .context("failed to read raw bytes from incoming stream")?;

            assert_eq!(raw_bytes, vec![0x05]);
        }

        // Test 2: Read as wasmtime Val
        {
            let paths: &[&[Option<usize>]] = &[];
            let (mut outgoing, mut incoming) = client
                .invoke((), "", func_name, bytes::Bytes::new(), paths)
                .await
                .context("failed to invoke function via wRPC")?;

            outgoing
                .flush()
                .await
                .context("failed to flush outgoing stream")?;

            // Decode to wasmtime Val
            let mut store = Store::new(&Engine::default(), gen_ctx(client.clone(), ()));
            let ty = wasmtime::component::Type::S64;
            let val = incoming
                .read_value(&mut store, &ty, &[], &[])
                .await
                .context("failed to decode value to wasmtime Val")?;

            // Verify the result
            match val {
                wasmtime::component::Val::S64(v) => assert_eq!(v, 5),
                _ => panic!("expected S64 value, got {:?}", val),
            }
        }

        // Test 3: Read as Rust value
        {
            let paths: &[&[Option<usize>]] = &[];
            let (mut outgoing, mut incoming) = client
                .invoke((), "", func_name, bytes::Bytes::new(), paths)
                .await
                .context("failed to invoke function via wRPC")?;

            outgoing
                .flush()
                .await
                .context("failed to flush outgoing stream")?;

            let result = incoming
                .read_i64_leb128()
                .await
                .context("failed to read result from incoming stream")?;

            // Verify the result
            assert_eq!(result, 5);
        }

        // Clean up
        server_handle.abort();
        for handle in invocation_handles {
            handle.abort();
        }

        Ok(())
    }
}
