use core::net::SocketAddr;
use core::pin::{Pin, pin};

use anyhow::{Context as _, bail};
use futures::{Stream, StreamExt as _, TryStreamExt as _, stream};
use tokio::io::BufReader;
use tokio::net::TcpStream;
use tokio::task::yield_now;
use tokio::{join, spawn, try_join};
use tracing::Instrument as _;
use wasmtime::component::{Component, Instance, Linker, ResourceTable, types};
use wrpc_transport::InvokeExt as _;
use wrpc_transport::frame::{ReadHalf, WriteHalf, tcp};
use wrpc_wasmtime::{
    ResourceTypes, ServeExt as _, SharedResourceTable, SharedStore, WrpcCtxView, WrpcView,
};

type Server = wrpc_transport::Server<(), BufReader<ReadHalf<TcpStream>>, WriteHalf<TcpStream>>;

struct Ctx {
    table: ResourceTable,
    shared_resources: SharedResourceTable,
}

impl WrpcView for Ctx {
    fn wrpc(&mut self) -> WrpcCtxView<'_> {
        WrpcCtxView {
            shared_resources: &mut self.shared_resources,
            table: &mut self.table,
        }
    }
}

async fn with_store(
    wat: &str,
    f: impl AsyncFnOnce(
        &tcp::Client<SocketAddr>,
        &Server,
        &Component,
        Instance,
        &SharedStore<Ctx>,
    ) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |clt, srv| {
        let engine = wasmtime::Engine::default();
        let component = Component::new(&engine, wat).map_err(anyhow::Error::from)?;
        let mut store = wasmtime::Store::new(
            &engine,
            Ctx {
                table: ResourceTable::default(),
                shared_resources: SharedResourceTable::default(),
            },
        );
        let instance = Linker::new(&engine)
            .instantiate_async(&mut store, &component)
            .await
            .map_err(anyhow::Error::from)?;
        let (store, event_loop) = SharedStore::new(store);
        let event_loop = spawn(event_loop.in_current_span());
        f(&clt, srv, &component, instance, &store).await?;
        drop(store);
        event_loop.await?
    })
    .await
}

async fn serve(
    srv: &Server,
    store: &SharedStore<Ctx>,
    component: &Component,
    instance: Instance,
    name: &str,
) -> anyhow::Result<
    impl Stream<Item = anyhow::Result<((), impl Future<Output = anyhow::Result<()>>)>>,
> {
    let Some((types::ComponentItem::ComponentFunc(ty), idx)) = component.get_export(None, name)
    else {
        bail!("function export `{name}` not found");
    };
    let invocations = srv
        .serve_function_shared(
            store.clone(),
            instance,
            idx,
            ResourceTypes::default(),
            ty,
            "",
            name,
        )
        .await?;
    Ok(invocations)
}

async fn handle(
    invocations: impl Stream<Item = anyhow::Result<((), impl Future<Output = anyhow::Result<()>>)>>,
) -> anyhow::Result<()> {
    let Some(((), fut)) = pin!(invocations).try_next().await? else {
        bail!("invocation missing");
    };
    fut.await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn shared_store_trap() -> anyhow::Result<()> {
    let res = with_store(
        r#"
(component
  (type $f (func))
  (core module $m
    (func (export "run")
      unreachable))
  (core instance $i (instantiate $m))
  (func (export "run") (type $f) (canon lift (core func $i "run")))
)"#,
        async |clt, srv, component, instance, store| {
            let run = serve(srv, store, component, instance, "run").await?;
            let (_, res) = join!(
                clt.invoke_values_blocking::<(), ()>((), "", "run", ()),
                async {
                    let Some(((), fut)) = pin!(run).try_next().await? else {
                        bail!("`run` invocation missing");
                    };
                    anyhow::Ok(fut.await)
                },
            );
            let res = res?;
            assert!(res.is_err(), "`run` invocation succeeded");
            Ok(())
        },
    )
    .await;
    let err = res.expect_err("event loop succeeded");
    assert!(format!("{err:#}").contains("unreachable"), "{err:#}");
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn async_values() -> anyhow::Result<()> {
    type ByteStream = Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>;
    type StringFuture = Pin<Box<dyn Future<Output = String> + Send>>;
    type UnitFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

    async fn receive<T>(
        io: Option<impl Future<Output = anyhow::Result<()>>>,
        values: impl Future<Output = T>,
    ) -> anyhow::Result<T> {
        let ((), values) = try_join!(
            async {
                if let Some(io) = io {
                    io.await?;
                }
                anyhow::Ok(())
            },
            async { anyhow::Ok(values.await) },
        )?;
        Ok(values)
    }

    with_store(
        r#"
(component
  (type $s (stream u8))
  (type $f (future string))
  (type $u (future))
  (type $t (tuple $s $f $u))
  (type $g (tuple $s $f))
  (type $fn (func async (param "s" $s) (param "f" $f) (param "u" $u) (result $t)))
  (core module $mem (memory (export "memory") 1))
  (core instance $mem (instantiate $mem))
  (alias core export $mem "memory" (core memory $memory))
  (core func $stream.new (canon stream.new $s))
  (core func $stream.write (canon stream.write $s async (memory $memory)))
  (core func $stream.drop-writable (canon stream.drop-writable $s))
  (core func $future.new (canon future.new $f))
  (core func $future.write (canon future.write $f async (memory $memory)))
  (core func $future.drop-writable (canon future.drop-writable $f))
  (core func $waitable-set.new (canon waitable-set.new))
  (core func $waitable.join (canon waitable.join))
  (core func $task.return (canon task.return (result $g)))
  (core module $m
    (import "" "memory" (memory 1))
    (import "" "stream.new" (func $stream.new (result i64)))
    (import "" "stream.write" (func $stream.write (param i32 i32 i32) (result i32)))
    (import "" "stream.drop-writable" (func $stream.drop-writable (param i32)))
    (import "" "future.new" (func $future.new (result i64)))
    (import "" "future.write" (func $future.write (param i32 i32) (result i32)))
    (import "" "future.drop-writable" (func $future.drop-writable (param i32)))
    (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
    (import "" "waitable.join" (func $waitable.join (param i32 i32)))
    (import "" "task.return" (func $task.return (param i32 i32)))
    (global $ws (mut i32) (i32.const 0))
    (global $s (mut i32) (i32.const 0))
    (global $f (mut i32) (i32.const 0))
    (data (i32.const 0x100) "foobar")
    (func (export "run") (param i32 i32 i32) (result i32)
      (i32.store (i32.const 0) (local.get 0))
      (i32.store (i32.const 4) (local.get 1))
      (i32.store (i32.const 8) (local.get 2))
      i32.const 0)
    (func $wait (param $w i32) (result i32)
      (call $waitable.join (local.get $w) (global.get $ws))
      (i32.or (i32.const 2) (i32.shl (global.get $ws) (i32.const 4))))
    (func $write-future (result i32)
      (call $stream.drop-writable (global.get $s))
      (if (i32.eq (call $future.write (global.get $f) (i32.const 0x200))
                  (i32.const -1))
        (then (return (call $wait (global.get $f)))))
      (call $future.drop-writable (global.get $f))
      i32.const 0)
    (func (export "gen") (result i32)
      (local $s i64)
      (local $f i64)
      (local.set $s (call $stream.new))
      (local.set $f (call $future.new))
      (global.set $s (i32.wrap_i64 (i64.shr_u (local.get $s) (i64.const 32))))
      (global.set $f (i32.wrap_i64 (i64.shr_u (local.get $f) (i64.const 32))))
      (global.set $ws (call $waitable-set.new))
      (call $task.return (i32.wrap_i64 (local.get $s)) (i32.wrap_i64 (local.get $f)))
      (i32.store (i32.const 0x200) (i32.const 0x103))
      (i32.store (i32.const 0x204) (i32.const 3))
      (if (i32.eq (call $stream.write (global.get $s) (i32.const 0x100) (i32.const 3))
                  (i32.const -1))
        (then (return (call $wait (global.get $s)))))
      (call $write-future))
    (func (export "cb") (param i32 i32 i32) (result i32)
      (call $waitable.join (local.get 1) (i32.const 0))
      (if (i32.eq (local.get 1) (global.get $s))
        (then (return (call $write-future))))
      (call $future.drop-writable (global.get $f))
      i32.const 0))
  (core instance $i (instantiate $m (with "" (instance
    (export "memory" (memory $memory))
    (export "stream.new" (func $stream.new))
    (export "stream.write" (func $stream.write))
    (export "stream.drop-writable" (func $stream.drop-writable))
    (export "future.new" (func $future.new))
    (export "future.write" (func $future.write))
    (export "future.drop-writable" (func $future.drop-writable))
    (export "waitable-set.new" (func $waitable-set.new))
    (export "waitable.join" (func $waitable.join))
    (export "task.return" (func $task.return))))))
  (func (export "run") (type $fn) (canon lift (core func $i "run") (memory $memory)))
  (func (export "gen") async (result $g)
    (canon lift (core func $i "gen") async (callback (core func $i "cb"))))
)"#,
        async |clt, srv, component, instance, store| {
            let run = serve(srv, store, component, instance, "run").await?;
            let gen_ = serve(srv, store, component, instance, "gen").await?;
            try_join!(
                async {
                    let (((s, f, u),), io) = clt
                        .invoke_values::<_, ((ByteStream, StringFuture, UnitFuture),)>(
                            (),
                            "",
                            "run",
                            (
                                Box::pin(
                                    stream::once(async {
                                        yield_now().await;
                                        b"foo".to_vec()
                                    })
                                    .chain(stream::iter([b"bar".to_vec()])),
                                ) as ByteStream,
                                Box::pin(async {
                                    yield_now().await;
                                    "baz".to_string()
                                }) as StringFuture,
                                Box::pin(yield_now()) as UnitFuture,
                            ),
                        )
                        .await?;
                    let (s, f, ()) =
                        receive(io, async { join!(s.collect::<Vec<_>>(), f, u) }).await?;
                    assert_eq!(s.concat(), b"foobar");
                    assert_eq!(f, "baz");

                    let (((s, f),), io) = clt
                        .invoke_values::<(), ((ByteStream, StringFuture),)>((), "", "gen", ())
                        .await?;
                    let (s, f) = receive(io, async { join!(s.collect::<Vec<_>>(), f) }).await?;
                    assert_eq!(s.concat(), b"foo");
                    assert_eq!(f, "bar");
                    anyhow::Ok(())
                },
                async { handle(run).await.context("failed to serve `run`") },
                async { handle(gen_).await.context("failed to serve `gen`") },
            )?;
            Ok(())
        },
    )
    .await
}
