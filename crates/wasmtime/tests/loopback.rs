use core::net::SocketAddr;
use core::pin::Pin;

use anyhow::bail;
use futures::{Stream, StreamExt as _, stream};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::yield_now;
use tokio::{join, spawn, try_join};
use tracing::Instrument as _;
use wasmtime::component::{Component, Linker, ResourceTable};
use wrpc_test::serve_until;
use wrpc_transport::{Connect as _, invoke, invoke_values, invoke_values_blocking, tcp};
use wrpc_wasmtime::{
    ResourceTypes, SharedExports, SharedResourceTable, SharedStore, WrpcCtxView, WrpcView,
};

type ByteStream = Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>;

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
        &TcpListener,
        &SharedExports<Ctx>,
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
        let exports = SharedExports::new(store, instance, &component, ResourceTypes::default())?;
        f(&clt, srv, &exports).await?;
        drop(exports);
        event_loop.await?
    })
    .await
}

async fn serve<T>(
    srv: &TcpListener,
    exports: &SharedExports<Ctx>,
    fut: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    serve_until(tcp::incoming(srv), fut, |cx, header, tx, rx| {
        let res = exports.handle(cx, &header.instance, &header.name, tx, rx);
        async move {
            let Ok(fut) = res else {
                bail!("unexpected invocation");
            };
            fut.await
        }
    })
    .await
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
        async |clt, srv, exports| {
            serve_until(
                tcp::incoming(srv),
                async {
                    let (tx, rx) = clt.connect(()).await?;
                    let res = invoke_values_blocking::<(), ()>(tx, rx, "", "run", ()).await;
                    res.expect_err("`run` invocation succeeded");
                    Ok(())
                },
                |cx, header, tx, rx| {
                    let res = exports.handle(cx, &header.instance, &header.name, tx, rx);
                    async move {
                        let Ok(fut) = res else {
                            bail!("unexpected invocation");
                        };
                        let res = fut.await;
                        res.expect_err("`run` invocation succeeded");
                        Ok(())
                    }
                },
            )
            .await
        },
    )
    .await;
    let err = res.expect_err("event loop succeeded");
    assert!(format!("{err:#}").contains("unreachable"), "{err:#}");
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn async_values() -> anyhow::Result<()> {
    type StringFuture = Pin<Box<dyn Future<Output = String> + Send>>;
    type UnitFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

    async fn receive<T>(
        io: impl Future<Output = anyhow::Result<()>>,
        values: impl Future<Output = T>,
    ) -> anyhow::Result<T> {
        let ((), values) = try_join!(io, async { anyhow::Ok(values.await) })?;
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
        async |clt, srv, exports| {
            serve(srv, exports, async {
                let (tx, rx) = clt.connect(()).await?;
                let (((s, f, u),), io) =
                    invoke_values::<_, ((ByteStream, StringFuture, UnitFuture),)>(
                        tx,
                        rx,
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
                let (s, f, ()) = receive(io, async { join!(s.collect::<Vec<_>>(), f, u) }).await?;
                assert_eq!(s.concat(), b"foobar");
                assert_eq!(f, "baz");

                let (tx, rx) = clt.connect(()).await?;
                let (((s, f),), io) =
                    invoke_values::<(), ((ByteStream, StringFuture),)>(tx, rx, "", "gen", ())
                        .await?;
                let (s, f) = receive(io, async { join!(s.collect::<Vec<_>>(), f) }).await?;
                assert_eq!(s.concat(), b"foo");
                assert_eq!(f, "bar");
                Ok(())
            })
            .await
        },
    )
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn async_params_after_return() -> anyhow::Result<()> {
    with_store(
        r#"
(component
  (type $s (stream u8))
  (core module $mem (memory (export "memory") 1))
  (core instance $mem (instantiate $mem))
  (alias core export $mem "memory" (core memory $memory))
  (core func $stream.read (canon stream.read $s async (memory $memory)))
  (core func $stream.drop-readable (canon stream.drop-readable $s))
  (core func $waitable-set.new (canon waitable-set.new))
  (core func $waitable.join (canon waitable.join))
  (core func $task.return (canon task.return))
  (core module $m
    (import "" "memory" (memory 1))
    (import "" "stream.read" (func $stream.read (param i32 i32 i32) (result i32)))
    (import "" "stream.drop-readable" (func $stream.drop-readable (param i32)))
    (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
    (import "" "waitable.join" (func $waitable.join (param i32 i32)))
    (import "" "task.return" (func $task.return))
    (global $ws (mut i32) (i32.const 0))
    (global $s (mut i32) (i32.const 0))
    (global $n (mut i32) (i32.const 0))
    (func $read (result i32)
      (local $res i32)
      (loop $l
        (local.set $res (call $stream.read (global.get $s) (i32.const 0) (i32.const 16)))
        (if (i32.eq (local.get $res) (i32.const -1))
          (then
            (call $waitable.join (global.get $s) (global.get $ws))
            (return (i32.or (i32.const 2) (i32.shl (global.get $ws) (i32.const 4))))))
        (global.set $n (i32.add (global.get $n) (i32.shr_u (local.get $res) (i32.const 4))))
        (br_if $l (i32.eqz (i32.and (local.get $res) (i32.const 0xf)))))
      (call $stream.drop-readable (global.get $s))
      i32.const 0)
    (func (export "run") (param i32) (result i32)
      (global.set $s (local.get 0))
      (global.set $ws (call $waitable-set.new))
      (call $task.return)
      (call $read))
    (func (export "cb") (param i32 i32 i32) (result i32)
      (call $waitable.join (local.get 1) (i32.const 0))
      (global.set $n (i32.add (global.get $n) (i32.shr_u (local.get 2) (i32.const 4))))
      (if (i32.and (local.get 2) (i32.const 0xf))
        (then
          (call $stream.drop-readable (global.get $s))
          (return (i32.const 0))))
      (call $read))
    (func (export "count") (result i32)
      global.get $n))
  (core instance $i (instantiate $m (with "" (instance
    (export "memory" (memory $memory))
    (export "stream.read" (func $stream.read))
    (export "stream.drop-readable" (func $stream.drop-readable))
    (export "waitable-set.new" (func $waitable-set.new))
    (export "waitable.join" (func $waitable.join))
    (export "task.return" (func $task.return))))))
  (func (export "run") async (param "s" $s)
    (canon lift (core func $i "run") async (callback (core func $i "cb"))))
  (func (export "count") (result u32) (canon lift (core func $i "count")))
)"#,
        async |clt, srv, exports| {
            serve(srv, exports, async {
                let (tx, rx) = oneshot::channel();
                let s = Box::pin(stream::once(async {
                    _ = rx.await;
                    b"foo".to_vec()
                })) as ByteStream;
                let ((), io) = {
                    let (tx, rx) = clt.connect(()).await?;
                    invoke_values::<_, ()>(tx, rx, "", "run", (s,)).await?
                };
                _ = tx.send(());
                io.await?;

                let (tx, rx) = clt.connect(()).await?;
                let (n,) = invoke_values_blocking::<(), (u32,)>(tx, rx, "", "count", ()).await?;
                assert_eq!(n, 3);
                Ok(())
            })
            .await
        },
    )
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn params_byte_by_byte() -> anyhow::Result<()> {
    const VALUE: &[u8] = b"\x02\x03foo\x01\x07\x01\x02hi\x03\x03bar\x00\x00\x05\x00";

    with_store(
        r#"
(component
  (type $v' (variant (case "x" u32) (case "y" string)))
  (export $v "v" (type $v'))
  (type $f' (flags "a" "b"))
  (export $f "f" (type $f'))
  (type $l (list (tuple string (option u32) $v $f)))
  (core module $m
    (memory (export "memory") 1)
    (global $heap (mut i32) (i32.const 8))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32)
      (local $p i32)
      (local.set $p (i32.and
        (i32.add (global.get $heap) (i32.sub (local.get 2) (i32.const 1)))
        (i32.sub (i32.const 0) (local.get 2))))
      (global.set $heap (i32.add (local.get $p) (local.get 3)))
      local.get $p)
    (func (export "echo") (param i32 i32) (result i32)
      (i32.store (i32.const 0) (local.get 0))
      (i32.store (i32.const 4) (local.get 1))
      i32.const 0))
  (core instance $i (instantiate $m))
  (alias core export $i "memory" (core memory $memory))
  (alias core export $i "realloc" (core func $realloc))
  (func (export "echo") (param "x" $l) (result $l)
    (canon lift (core func $i "echo") (memory $memory) (realloc $realloc)))
)"#,
        async |clt, srv, exports| {
            serve(srv, exports, async {
                let (tx, rx) = clt.connect(()).await?;
                let (mut tx, mut rx) = invoke(tx, rx, "", "echo", [[None; 0]; 0])?;
                for b in VALUE.chunks(1) {
                    tx.write_all(b).await?;
                    tx.flush().await?;
                }
                tx.shutdown().await?;
                let mut buf = Vec::default();
                rx.read_to_end(&mut buf).await?;
                assert_eq!(buf, VALUE);
                Ok(())
            })
            .await
        },
    )
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn unread_param() -> anyhow::Result<()> {
    with_store(
        r#"
(component
  (type $s (stream u8))
  (type $fn (func async (param "s" $s)))
  (core func $stream.drop-readable (canon stream.drop-readable $s))
  (core module $m
    (import "" "stream.drop-readable" (func $stream.drop-readable (param i32)))
    (func (export "run") (param i32)
      (call $stream.drop-readable (local.get 0))))
  (core instance $i (instantiate $m (with "" (instance
    (export "stream.drop-readable" (func $stream.drop-readable))))))
  (func (export "run") (type $fn) (canon lift (core func $i "run")))
)"#,
        async |clt, srv, exports| {
            serve_until(
                tcp::incoming(srv),
                async {
                    let (tx, rx) = clt.connect(()).await?;
                    let s = stream::once(async {
                        yield_now().await;
                        b"foo".to_vec()
                    });
                    let ((), io) = invoke_values::<(ByteStream,), ()>(
                        tx,
                        rx,
                        "",
                        "run",
                        (Box::pin(s) as ByteStream,),
                    )
                    .await?;
                    _ = io.await;
                    Ok(())
                },
                |cx, header, tx, rx| {
                    let res = exports.handle(cx, &header.instance, &header.name, tx, rx);
                    async move {
                        let Ok(fut) = res else {
                            bail!("unexpected invocation");
                        };
                        let res = fut.await;
                        res.expect_err("`run` invocation succeeded");
                        Ok(())
                    }
                },
            )
            .await
        },
    )
    .await
}
