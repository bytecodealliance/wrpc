use core::net::SocketAddr;
use core::pin::pin;

use std::sync::Arc;

use anyhow::{Context as _, bail};
use futures::TryStreamExt as _;
use tokio::io::BufReader;
use tokio::{join, spawn, try_join};
use tracing::Instrument as _;
use wasmtime::component::{Component, Linker, ResourceTable, ResourceType, types};
use wrpc_transport::frame::tcp;
use wrpc_transport::{InvokeExt as _, ServeExt as _};
use wrpc_wasmtime::{
    ResourceTypes, ServeExt as _, SharedResourceTable, SharedStore, WrpcCtx, WrpcCtxView, WrpcView,
    link_item,
};

struct Ctx {
    table: ResourceTable,
    wrpc: WrpcCtxImpl,
}

struct WrpcCtxImpl {
    clt: tcp::Client<SocketAddr>,
    shared_resources: SharedResourceTable,
}

impl WrpcCtx<tcp::Client<SocketAddr>> for WrpcCtxImpl {
    fn context(&self) {}

    fn client(&self) -> &tcp::Client<SocketAddr> {
        &self.clt
    }

    fn shared_resources(&mut self) -> &mut SharedResourceTable {
        &mut self.shared_resources
    }
}

impl WrpcView for Ctx {
    type Invoke = tcp::Client<SocketAddr>;

    fn wrpc(&mut self) -> WrpcCtxView<'_, Self::Invoke> {
        WrpcCtxView {
            ctx: &mut self.wrpc,
            table: &mut self.table,
        }
    }
}

async fn with_store(
    wat: &str,
    f: impl AsyncFnOnce(
        &tcp::Client<SocketAddr>,
        &wrpc_transport::Server<(), BufReader<tcp::ReadHalf>, tcp::WriteHalf>,
        Component,
        wasmtime::Store<Ctx>,
    ) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let lis = wrpc_test::tcp_bind().await?;
    let addr = lis.local_addr()?;
    let clt = tcp::Client::from(addr);
    let srv = wrpc_transport::Server::default();
    let engine = wasmtime::Engine::default();
    let component = Component::new(&engine, wat).map_err(anyhow::Error::from)?;
    let store = wasmtime::Store::new(
        &engine,
        Ctx {
            table: ResourceTable::default(),
            wrpc: WrpcCtxImpl {
                clt: clt.clone(),
                shared_resources: SharedResourceTable::default(),
            },
        },
    );
    wrpc_test::accept_until(f(&clt, &srv, component, store), async || {
        wrpc_test::accept_tcp(&srv, &lis).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn polyfill_async() -> anyhow::Result<()> {
    with_store(
        r#"
(component
  (type $f (func async (param "x" u32) (result u32)))
  (import "foo" (func $foo (type $f)))
  (core func $foo (canon lower (func $foo)))
  (core module $m
    (import "" "foo" (func $foo (param i32) (result i32)))
    (func (export "run") (param i32) (result i32)
      local.get 0
      call $foo
      i32.const 1
      i32.add))
  (core instance $i (instantiate $m (with "" (instance (export "foo" (func $foo))))))
  (func (export "run") (type $f) (canon lift (core func $i "run")))
)"#,
        async |clt, srv, component, mut store| {
            let engine = store.engine().clone();
            let guest_resources: Arc<[ResourceType]> = Arc::default();
            let host_resources = Arc::default();
            let mut linker = Linker::new(&engine);
            for (name, types::ComponentExtern { ty, .. }) in
                component.component_type().imports(&engine)
            {
                link_item(
                    &engine,
                    &mut linker.root(),
                    Arc::clone(&guest_resources),
                    Arc::clone(&host_resources),
                    ty,
                    "",
                    name,
                )
                .map_err(anyhow::Error::from)?;
            }
            let instance = linker
                .instantiate_async(&mut store, &component)
                .await
                .map_err(anyhow::Error::from)?;
            let Some((types::ComponentItem::ComponentFunc(ty), idx)) =
                component.get_export(None, "run")
            else {
                bail!("function export `run` not found");
            };
            assert!(ty.async_());

            let (store, event_loop) = SharedStore::new(store);
            let event_loop = spawn(event_loop.in_current_span());
            let run = srv
                .serve_function_shared(
                    store,
                    instance,
                    idx,
                    ResourceTypes {
                        guest: guest_resources,
                        ..ResourceTypes::default()
                    },
                    host_resources,
                    ty,
                    "",
                    "run",
                )
                .await?;
            let foo = srv
                .serve_values::<(u32,), (u32,)>("", "foo", Arc::default())
                .await?;
            try_join!(
                async {
                    let (v,) = clt
                        .invoke_values_blocking::<_, _, (u32,)>((), "", "run", (42,), &[[]; 0])
                        .await?;
                    assert_eq!(v, 85);
                    anyhow::Ok(())
                },
                async {
                    let Some(((), fut)) = pin!(run).try_next().await? else {
                        bail!("`run` invocation missing");
                    };
                    fut.await.context("failed to serve `run`")
                },
                async {
                    let Some(((), (x,), rx, tx)) = pin!(foo).try_next().await? else {
                        bail!("`foo` invocation missing");
                    };
                    assert!(rx.is_none());
                    assert_eq!(x, 42);
                    tx((x * 2,)).await
                },
            )?;
            let res = event_loop.await?;
            res.context("event loop failed")
        },
    )
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn shared_store_trap() -> anyhow::Result<()> {
    with_store(
        r#"
(component
  (type $f (func))
  (core module $m
    (func (export "run")
      unreachable))
  (core instance $i (instantiate $m))
  (func (export "run") (type $f) (canon lift (core func $i "run")))
)"#,
        async |clt, srv, component, mut store| {
            let instance = Linker::new(store.engine())
                .instantiate_async(&mut store, &component)
                .await
                .map_err(anyhow::Error::from)?;
            let Some((types::ComponentItem::ComponentFunc(ty), idx)) =
                component.get_export(None, "run")
            else {
                bail!("function export `run` not found");
            };

            let (store, event_loop) = SharedStore::new(store);
            let run = srv
                .serve_function_shared(
                    store,
                    instance,
                    idx,
                    ResourceTypes::default(),
                    Arc::default(),
                    ty,
                    "",
                    "run",
                )
                .await?;
            let event_loop = spawn(event_loop.in_current_span());
            let (_, res) = join!(
                clt.invoke_values_blocking::<_, (), ()>((), "", "run", (), &[[]; 0]),
                async {
                    let Some(((), fut)) = pin!(run).try_next().await? else {
                        bail!("`run` invocation missing");
                    };
                    anyhow::Ok(fut.await)
                },
            );
            let res = res?;
            assert!(res.is_err(), "`run` invocation succeeded");
            let res = event_loop.await?;
            let err = res.expect_err("event loop succeeded");
            assert!(format!("{err:#}").contains("unreachable"), "{err:#}");
            Ok(())
        },
    )
    .await
}
