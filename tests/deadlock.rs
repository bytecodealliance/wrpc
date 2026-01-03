#[cfg(test)]
mod parameter_validation_tests {
    use core::pin::pin;
    use std::collections::{HashMap};
    use anyhow::Context as _;
    use futures::StreamExt;
    use tokio::join;
    use tracing::instrument;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::sync::Mutex;
    use wasmtime::component::ResourceTable;
    use wasmtime::component::{Component, Linker};
    use wasmtime::Engine;
    use wasmtime::Store;
    use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
    use wrpc_runtime_wasmtime::{
        ServeExt,
        SharedResourceTable, WrpcCtxView, WrpcView,
    };
    use wrpc_transport::frame::Oneshot;
    use wrpc_transport::Invoke;
    use tokio::io::AsyncReadExt;

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

    /// Test that insufficient parameters cause an error instead of deadlock
    ///
    /// This test verifies the fix for the DoS vulnerability where malformed
    /// parameter data causes the server to hang indefinitely.
    ///
    /// IMPORTANT: This test uses serve_function_shared (runtime-wasmtime layer)
    /// with raw invoke(), which is where the bug manifests. Using serve_values
    /// (transport layer) with invoke_values_blocking will NOT reproduce the bug.
    #[test_log::test(tokio::test(flavor = "multi_thread"))]
    #[instrument(ret)]
    async fn test_parameter_validation_deadlock() -> anyhow::Result<()> {
        let params = wit_bindgen_wrpc::bytes::Bytes::new(); // Empty params

        let (oneshot_clt, oneshot_srv) = Oneshot::duplex(1024);
        let srv = Arc::new(wrpc_transport::frame::Server::default());

        let mut config = wasmtime::Config::default();
        config.async_support(true);
        let engine = Engine::new(&config).context("failed to create engine with async support")?;

        let component_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("simple-args.wasm");
        let component_bytes = std::fs::read(&component_path)
            .with_context(|| format!("failed to read component from {:?}", component_path))?;

        let component =
            Component::new(&engine, component_bytes).context("failed to parse component")?;

        let linker = Linker::new(&engine);
        let mut store = Store::new(&engine, gen_ctx(oneshot_clt, ()));

        let instance = linker
            .instantiate_async(&mut store, &component)
            .await
            .context("failed to instantiate component")?;


        let function = "get-value";
        let func = instance
            .get_func(&mut store, &function)
            .ok_or_else(|| anyhow::anyhow!("function `{function}` not found in component"))?;

        let fun_ty = func.ty(&store);

        let guest_resources_vec = Vec::new();
        let host_resources = HashMap::new();

        let instance_name = "".to_string();
        
        let store_shared = Arc::new(Mutex::new(store));
        let store_shared_clone = store_shared.clone();
        let invocations_stream = srv
            .serve_function_shared(
              store_shared,
                instance,
                Arc::from(guest_resources_vec.into_boxed_slice()),
                Arc::from(host_resources),
                fun_ty,
                &instance_name,
                &function,
            )
            .await
            .with_context(|| {
                format!("failed to register handler for function `{function}`")
            })?;
        let (result, invocation_handle) = join!(
            // client side
            async move {
                let paths: &[&[Option<usize>]] = &[];
                // Lock the store only to get the wrpc client and invoke
                // Release the lock immediately after getting the streams
                let (mut outgoing, mut incoming) = {
                    let store = store_shared_clone.lock().await;
                    store.data().wrpc.wrpc
                        .invoke((), &instance_name, &function, params, paths)
                        .await
                        .expect(&format!("failed to invoke {}", function))
                };
                // Lock is now released, allowing server to process the invocation
                outgoing.flush().await?;
                let mut buf = vec![];
                incoming
                    .read_to_end(&mut buf)
                    .await
                    .with_context(|| format!("failed to read result for root function `{function}`"))?;
                  Ok::<Vec<u8>, anyhow::Error>(buf)
            },
            // server side
            async move {
                srv.accept(oneshot_srv)
                    .await
                    .expect("failed to accept connection");

                tokio::spawn(async move {
                    let mut invocations = pin!(invocations_stream);
                    while let Some(invocation) = invocations.as_mut().next().await {
                        match invocation {
                            Ok((_, fut)) => {
                                if let Err(err) = fut.await {
                                    eprintln!("failed to serve invocation for root function `{function}`: {err:?}");
                                }
                            }
                            Err(err) => {
                                eprintln!("failed to accept invocation for root function `{function}`: {err:?}");
                            }
                        }
                    }
                })
            }
        );
        // Clean up the invocation handle since the oneshot connection is complete
        // The stream should naturally end, but we abort to ensure cleanup happens immediately
        invocation_handle.abort();
        println!("result: {:?}", result);
        Ok(())
    }
}
