#![allow(clippy::disallowed_names, clippy::type_complexity)]
#![recursion_limit = "256"]

pub mod common;

use core::iter;
use core::pin::Pin;
use core::time::Duration;

use std::sync::Arc;

use anyhow::{Context as _, bail, ensure};
use bytes::{Bytes, BytesMut};
use common::{assert_async, assert_eof, assert_sync};
use futures::future::BoxFuture;
use futures::stream::{BoxStream, FuturesUnordered};
use futures::{StreamExt as _, TryStreamExt as _, future, stream};
use tokio::io::{
    AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, duplex, sink, split,
};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::yield_now;
use tokio::time;
use tokio::{join, select, spawn, try_join};
use tokio_util::codec::{Encoder as _, FramedParts, FramedRead, FramedWrite};
use tokio_util::io::StreamReader;
use tracing::instrument;
use wrpc_test::next_invocation;
use wrpc_transport::{
    Decode, Encode, FrameDecoder, FrameEncoder, FrameRef, Framed, Header, HeaderDecoder,
    HeaderEncoder, HeaderRef, Listener, invoke, invoke_blocking, receive, serve, transmit,
};

mod sync_bindgen {
    use std::sync::{Arc, Mutex};

    use anyhow::{Context as _, ensure};
    use bytes::Bytes;
    use wrpc_transport::{ResourceBorrow, ResourceOwn};

    use crate::common::Connection;

    pub mod sync_client {
        wit_bindgen_wrpc::generate!({
            world: "sync-client",
            path: "tests/wit",
        });
    }

    pub mod sync_server {
        wit_bindgen_wrpc::generate!({
            world: "sync-server",
            path: "tests/wit",
        });
    }

    use sync_server::exports::wrpc_test::integration::resources::{self, Counter, Foo};
    use sync_server::exports::wrpc_test::integration::sync::{
        self, Abc, Foobar, Rec, RecNested, Var,
    };

    fn rec(s: impl Into<String>) -> Rec {
        Rec {
            nested: RecNested { foo: s.into() },
        }
    }

    fn ensure_foo(handle: impl Into<Bytes>) -> anyhow::Result<()> {
        ensure!(handle.into() == "foo", "unknown resource");
        Ok(())
    }

    #[derive(Clone)]
    pub struct Handler<F> {
        pub counts: Arc<Mutex<Vec<u32>>>,
        pub count: F,
    }

    impl<F> Handler<F> {
        fn push(&self, count: u32) -> ResourceOwn<Counter> {
            let mut counts = self.counts.lock().unwrap();
            counts.push(count);
            ResourceOwn::from(Bytes::copy_from_slice(
                &counts.len().saturating_sub(1).to_ne_bytes(),
            ))
        }

        fn with<T>(
            &self,
            handle: ResourceBorrow<Counter>,
            f: impl FnOnce(&mut u32) -> T,
        ) -> anyhow::Result<T> {
            let handle = Bytes::from(handle);
            let i = handle.as_ref().try_into()?;
            let i = usize::from_ne_bytes(i);
            let mut counts = self.counts.lock().unwrap();
            let count = counts.get_mut(i).context("unknown counter")?;
            Ok(f(count))
        }
    }

    impl<F: Send + Sync + 'static> resources::HandlerFoo for Handler<F> {
        async fn new(&self) -> anyhow::Result<ResourceOwn<Foo>> {
            Ok(ResourceOwn::from(Bytes::from_static(b"foo")))
        }

        async fn foo(&self, v: ResourceOwn<Foo>) -> anyhow::Result<String> {
            ensure_foo(v)?;
            Ok("foo".into())
        }

        async fn bar(&self, self_: ResourceBorrow<Foo>) -> anyhow::Result<String> {
            ensure_foo(self_)?;
            Ok("bar".into())
        }
    }

    impl<F: Send + Sync + 'static> resources::HandlerCounter for Handler<F> {
        async fn new(&self, initial: u32) -> anyhow::Result<ResourceOwn<Counter>> {
            Ok(self.push(initial))
        }

        async fn clone_counter(
            &self,
            self_: ResourceBorrow<Counter>,
        ) -> anyhow::Result<ResourceOwn<Counter>> {
            let count = self.with(self_, |count| *count)?;
            Ok(self.push(count))
        }

        async fn get_count(&self, self_: ResourceBorrow<Counter>) -> anyhow::Result<u32> {
            self.with(self_, |count| *count)
        }

        async fn increment_by(
            &self,
            self_: ResourceBorrow<Counter>,
            num: u32,
        ) -> anyhow::Result<()> {
            self.with(self_, |count| *count = count.wrapping_add(num))
        }

        async fn sum(
            &self,
            a: ResourceBorrow<Counter>,
            b: ResourceBorrow<Counter>,
        ) -> anyhow::Result<u32> {
            let a = self.with(a, |count| *count)?;
            let b = self.with(b, |count| *count)?;
            Ok(a.wrapping_add(b))
        }
    }

    impl<F: Send + Sync + 'static> resources::Handler for Handler<F> {
        async fn bar(&self, v: ResourceBorrow<Foo>) -> anyhow::Result<String> {
            ensure_foo(v)?;
            Ok("bar".into())
        }
    }

    impl<F: Send + Sync + 'static> sync_server::exports::strange::Handler for Handler<F> {
        async fn bar(&self, v: ResourceBorrow<Foo>) -> anyhow::Result<u64> {
            ensure_foo(v)?;
            Ok(42)
        }
    }

    impl<F, Fut> sync_server::Handler for Handler<F>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<u32>> + Send,
    {
        async fn ping(&self) -> anyhow::Result<String> {
            let n = (self.count)().await?;
            ensure!(n == 42);
            Ok("pong".into())
        }
    }

    /// Creates a counter on `clt` and returns its count
    pub async fn count(clt: &mut impl Connection) -> anyhow::Result<u32> {
        use sync_client::wrpc_test::integration::resources::Counter;

        let counter = Counter::new(clt.client(), 42).await?;
        let n = Counter::get_count(clt.client(), &counter.as_borrow()).await?;
        Ok(n)
    }

    impl<F: Send + Sync + 'static> sync::Handler for Handler<F> {
        async fn fallible(&self, ok: bool) -> anyhow::Result<Result<bool, String>> {
            Ok(if ok { Ok(true) } else { Err("test".into()) })
        }

        async fn numbers(
            &self,
        ) -> anyhow::Result<(u8, u16, u32, u64, i8, i16, i32, i64, f32, f64)> {
            Ok((1, 2, 3, 4, 5, 6, 7, 8, 9., 10.))
        }

        async fn with_flags(&self, a: bool, b: bool, c: bool) -> anyhow::Result<Abc> {
            let mut v = Abc::empty();
            v.set(Abc::A, a);
            v.set(Abc::B, b);
            v.set(Abc::C, c);
            Ok(v)
        }

        async fn with_variant_option(&self, ok: bool) -> anyhow::Result<Option<Var>> {
            Ok(ok.then(|| Var::Var(rec("bar"))))
        }

        async fn with_variant_list(&self) -> anyhow::Result<Vec<Var>> {
            Ok(vec![
                Var::Empty,
                Var::Var(rec("foo")),
                Var::Empty,
                Var::Empty,
                Var::Empty,
                Var::Var(rec("bar")),
            ])
        }

        async fn with_record(&self) -> anyhow::Result<Rec> {
            Ok(rec("foo"))
        }

        async fn with_record_list(&self, n: u8) -> anyhow::Result<Vec<Rec>> {
            Ok((0..n).map(|i| rec(i.to_string())).collect())
        }

        async fn with_record_tuple(&self) -> anyhow::Result<(Rec, Rec)> {
            Ok((rec("0"), rec("1")))
        }

        async fn with_enum(&self) -> anyhow::Result<Foobar> {
            Ok(Foobar::Bar)
        }

        async fn identity_char(&self, c: char) -> anyhow::Result<char> {
            Ok(c)
        }

        async fn identity_option_lists(
            &self,
            a: Option<Bytes>,
            b: Option<Vec<String>>,
        ) -> anyhow::Result<(Option<Bytes>, Option<Vec<String>>)> {
            Ok((a, b))
        }
    }

    impl<F: Send + Sync + 'static> sync_server::exports::foo::Handler for Handler<F> {
        async fn f(&self, x: String) -> anyhow::Result<u32> {
            assert_eq!(x, "f");
            Ok(42)
        }

        async fn foo(&self, x: String) -> anyhow::Result<()> {
            assert_eq!(x, "foo");
            Ok(())
        }
    }
}

#[instrument(skip_all, ret)]
async fn assert_bindgen_sync<Fut, CI, CO, CE, I, O, E>(
    connect: impl Fn() -> Fut + Clone + Send + Sync + 'static,
    mut accept: impl AsyncFnMut() -> Result<(O, I), E>,
) -> anyhow::Result<()>
where
    Fut: Future<Output = Result<(CO, CI), CE>> + Send,
    CI: AsyncRead + Send + Unpin + 'static,
    CO: AsyncWrite + Send + Unpin + 'static,
    CE: core::error::Error + Send + Sync + 'static,
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
    E: core::error::Error + Send + Sync + 'static,
{
    let handler = sync_bindgen::Handler {
        counts: Arc::default(),
        count: {
            let connect = connect.clone();
            move || {
                let connect = connect.clone();
                async move {
                    let (tx, rx) = connect().await?;
                    let mut clt = Framed::new(tx, rx);
                    let n = sync_bindgen::count(&mut clt).await?;
                    assert_eof(clt).await?;
                    Ok(n)
                }
            }
        },
    };
    let serve = async |tx, rx| {
        let mut lis = Listener::new(tx, rx);
        while let Some((Header { instance, name }, srv)) = lis.accept().await? {
            let Ok(fut) = sync_bindgen::sync_server::handle(&handler, &instance, &name, srv) else {
                bail!("`{instance}#{name}` is not exported");
            };
            fut.await?;
        }
        anyhow::Ok(())
    };
    try_join!(
        async {
            let (srv_tx, srv_rx) = accept().await.context("failed to accept")?;
            try_join!(serve(srv_tx, srv_rx), async {
                let (srv_tx, srv_rx) = accept().await.context("failed to accept")?;
                serve(srv_tx, srv_rx).await
            })
        },
        async {
            let (tx, rx) = connect().await.context("failed to connect")?;
            let mut clt = Framed::new(tx, rx);
            assert_sync(&mut clt).await?;
            assert_eof(clt).await
        },
    )?;
    Ok(())
}

mod async_bindgen {
    use bytes::Bytes;
    use futures::future::BoxFuture;
    use futures::stream::{self, BoxStream};

    pub mod bindings {
        wit_bindgen_wrpc::generate!({
            world: "async-server",
            path: "tests/wit",
        });
    }

    use bindings::exports::wrpc_test::integration::async_::{self, AsyncCase, Something};

    #[derive(Clone)]
    pub struct Handler;

    impl async_::Handler for Handler {
        async fn with_streams(
            &self,
        ) -> anyhow::Result<(
            BoxStream<'static, Bytes>,
            BoxStream<'static, Vec<Vec<String>>>,
        )> {
            Ok((
                Box::pin(stream::iter([Bytes::from("test")])),
                Box::pin(stream::iter([
                    vec![vec!["foo".into()]],
                    vec![vec!["bar".into(), "baz".into()]],
                ])),
            ))
        }

        async fn with_future(
            &self,
            x: Something,
            s: BoxStream<'static, Bytes>,
        ) -> anyhow::Result<BoxFuture<'static, BoxStream<'static, Bytes>>> {
            assert_eq!(x.foo, "bar");
            Ok(Box::pin(async { s }))
        }

        async fn identity_nested_async(
            &self,
            v: BoxFuture<
                'static,
                BoxFuture<'static, BoxFuture<'static, BoxStream<'static, Vec<String>>>>,
            >,
        ) -> anyhow::Result<
            BoxFuture<
                'static,
                BoxFuture<'static, BoxFuture<'static, BoxStream<'static, Vec<String>>>>,
            >,
        > {
            Ok(v)
        }

        async fn identity_future(
            &self,
            v: BoxFuture<'static, ()>,
        ) -> anyhow::Result<BoxFuture<'static, ()>> {
            Ok(v)
        }

        async fn identity_cases(
            &self,
            o: Option<BoxFuture<'static, ()>>,
            r: Result<BoxStream<'static, Bytes>, BoxFuture<'static, u32>>,
            v: AsyncCase,
        ) -> anyhow::Result<(
            Option<BoxFuture<'static, ()>>,
            Result<BoxStream<'static, Bytes>, BoxFuture<'static, u32>>,
            AsyncCase,
        )> {
            Ok((o, r, v))
        }
    }
}

#[instrument(skip_all, ret)]
async fn assert_bindgen_async(
    tx: impl AsyncWrite + Send + Unpin,
    rx: impl AsyncRead + Send + Unpin,
    accept: impl Future<
        Output = Result<
            (
                impl AsyncWrite + Send + Unpin + 'static,
                impl AsyncRead + Send + Unpin + 'static,
            ),
            impl core::error::Error + Send + Sync + 'static,
        >,
    >,
) -> anyhow::Result<()> {
    try_join!(
        async {
            let (srv_tx, srv_rx) = accept.await.context("failed to accept")?;
            spawn(async move {
                let mut lis = Listener::new(srv_tx, srv_rx);
                while let Some((Header { instance, name }, srv)) = lis.accept().await? {
                    let Ok(fut) = async_bindgen::bindings::handle(
                        &async_bindgen::Handler,
                        &instance,
                        &name,
                        srv,
                    ) else {
                        bail!("`{instance}#{name}` is not exported");
                    };
                    fut.await?;
                }
                anyhow::Ok(())
            })
            .await?
        },
        async {
            let mut clt = Framed::new(tx, rx);
            assert_async(&mut clt).await?;
            assert_eof(clt).await
        },
    )?;
    Ok(())
}

type SyncValues = (
    (bool, u8, u16, u32, u64, i8, i16, i32, i64, f32, f64, char),
    String,
    Vec<Vec<Vec<u8>>>,
    Vec<Option<Vec<Result<Option<String>, ()>>>>,
);

type AsyncValues = (
    BoxStream<'static, Vec<u32>>,
    BoxStream<'static, Vec<String>>,
    Pin<Box<dyn AsyncRead + Send>>,
    BoxFuture<'static, Vec<u64>>,
    BoxStream<'static, Bytes>,
    Vec<BoxFuture<'static, String>>,
);

fn sync_values() -> SyncValues {
    (
        (
            true,
            0xfe,
            0xfeff,
            0xfeff_ffff,
            0xfeff_ffff_ffff_ffff,
            0x7e,
            0x7eff,
            0x7eff_ffff,
            0x7eff_ffff_ffff_ffff,
            0.42,
            0.4242,
            'a',
        ),
        "test".into(),
        vec![vec![b"foo".to_vec()]],
        vec![Some(vec![Ok(Some("bar".into()))])],
    )
}

fn pending<T: Send + 'static>(
    items: impl IntoIterator<Item = T, IntoIter: Send + 'static>,
) -> BoxStream<'static, T> {
    Box::pin(stream::iter(items).then(|v| async move {
        yield_now().await;
        v
    }))
}

fn pending_string(s: &'static str) -> BoxFuture<'static, String> {
    Box::pin(async move {
        yield_now().await;
        s.into()
    })
}

fn async_values() -> AsyncValues {
    (
        pending([vec![0xc0, 0xff], vec![], vec![0xee]]),
        pending([vec!["foo".into()], vec![], vec!["bar".into()]]),
        Box::pin(StreamReader::new(pending([Ok::<_, std::io::Error>(
            Bytes::from("test"),
        )]))),
        Box::pin(async {
            yield_now().await;
            vec![1, 2, 3]
        }),
        pending(
            iter::once(Bytes::default())
                .chain(iter::repeat_n(Bytes::from("abcd".repeat(1 << 13)), 1 << 3)),
        ),
        vec![pending_string("foo"), pending_string("bar")],
    )
}

async fn assert_async_values(
    (a, b, mut c, d, e, f): AsyncValues,
    io: impl Future<Output = std::io::Result<()>>,
) -> anyhow::Result<()> {
    try_join!(
        async {
            let (a, b, c, d, e, f) = join!(
                a.concat(),
                b.concat(),
                async {
                    let mut s = String::default();
                    let res = c.read_to_string(&mut s).await;
                    res.map(|_| s)
                },
                d,
                e.collect::<Vec<_>>(),
                future::join_all(f),
            );
            assert_eq!(a, [0xc0, 0xff, 0xee]);
            assert_eq!(b, ["foo", "bar"]);
            let c = c?;
            assert_eq!(c, "test");
            assert_eq!(d, [1, 2, 3]);
            assert_eq!(e.concat(), b"abcd".repeat(1 << 16));
            assert_eq!(f, ["foo", "bar"]);
            anyhow::Ok(())
        },
        async { io.await.context("failed to complete I/O") },
    )?;
    Ok(())
}

#[instrument(skip_all, ret)]
async fn assert_dynamic<CI, CO, CE, I, O, E>(
    connect: impl AsyncFn() -> Result<(CO, CI), CE>,
    mut accept: impl AsyncFnMut() -> Result<(O, I), E>,
) -> anyhow::Result<()>
where
    CI: AsyncRead + Send + Unpin,
    CO: AsyncWrite + Send + Unpin,
    CE: core::error::Error + Send + Sync + 'static,
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
    E: core::error::Error + Send + Sync + 'static,
{
    try_join!(
        async {
            for spawned in [false, true] {
                let (tx, rx) = accept().await.context("failed to accept")?;
                let mut lis = Listener::new(tx, rx);
                next_invocation(&mut lis, "test", "reset").await?;
                if spawned {
                    spawn(async move { drop(lis) }).await?;
                } else {
                    drop(lis);
                }
            }
            let (tx, rx) = accept().await.context("failed to accept")?;
            let mut lis = Listener::new(tx, rx);
            for _ in 0..2 {
                let srv = next_invocation(&mut lis, "test", "sync").await?;
                serve::<SyncValues, SyncValues, _, _, _>(srv, async |params| {
                    assert_eq!(params, sync_values());
                    anyhow::Ok(sync_values())
                })
                .await?;
            }
            let srv = next_invocation(&mut lis, "test", "async").await?;
            serve::<AsyncValues, AsyncValues, _, _, _>(srv, async |params| {
                assert_async_values(params, future::ready(Ok(()))).await?;
                anyhow::Ok(async_values())
            })
            .await?;
            let next = lis.accept().await?;
            ensure!(next.is_none(), "unexpected invocation");
            Ok(())
        },
        async {
            for _ in 0..2 {
                let (tx, rx) = connect().await?;
                let res = invoke_blocking::<(&str,), (String,), _>(
                    Framed::new(tx, rx),
                    "test",
                    "reset",
                    ("arg",),
                )
                .await;
                res.expect_err("`test.reset` should have failed");
            }

            let mut buf = BytesMut::default();
            let enc = <SyncValues as Encode>::Encoder::default();
            wrpc_transport::pack(enc, sync_values(), &mut buf)?;
            let dec = <SyncValues as Decode>::Decoder::default();
            let unpacked = wrpc_transport::unpack(dec, &mut buf)?;
            let (tx, rx) = connect().await?;
            let mut clt = Framed::new(tx, rx);
            for params in [sync_values(), unpacked] {
                let res =
                    invoke_blocking::<SyncValues, SyncValues, _>(&mut clt, "test", "sync", params)
                        .await?;
                assert_eq!(res, sync_values());
            }

            let (res, io) =
                invoke::<AsyncValues, AsyncValues, _>(&mut clt, "test", "async", async_values())
                    .await?;
            assert_async_values(res, io).await?;
            assert_eof(clt).await
        },
    )?;
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_tcp_bindgen_sync() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |addr, lis| {
        assert_bindgen_sync(
            move || wrpc_test::tcp_streams(addr),
            async || wrpc_test::tcp_accept(&lis).await,
        )
        .await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_tcp_bindgen_async() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |addr, lis| {
        let (tx, rx) = wrpc_test::tcp_streams(addr).await?;
        assert_bindgen_async(tx, rx, wrpc_test::tcp_accept(&lis)).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_tcp_dynamic() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |addr, lis| {
        assert_dynamic(
            move || wrpc_test::tcp_streams(addr),
            async || wrpc_test::tcp_accept(&lis).await,
        )
        .await
    })
    .await
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_uds_bindgen_sync() -> anyhow::Result<()> {
    wrpc_test::serve_unix(async |path, lis| {
        assert_bindgen_sync(
            move || wrpc_test::unix_streams(path.clone()),
            async || wrpc_test::unix_accept(&lis).await,
        )
        .await
    })
    .await
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_uds_bindgen_async() -> anyhow::Result<()> {
    wrpc_test::serve_unix(async |path, lis| {
        let (tx, rx) = wrpc_test::unix_streams(path).await?;
        assert_bindgen_async(tx, rx, wrpc_test::unix_accept(&lis)).await
    })
    .await
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_uds_dynamic() -> anyhow::Result<()> {
    wrpc_test::serve_unix(async |path, lis| {
        assert_dynamic(
            move || wrpc_test::unix_streams(path.clone()),
            async || wrpc_test::unix_accept(&lis).await,
        )
        .await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_quic_bindgen_sync() -> anyhow::Result<()> {
    wrpc_test::with_quic(async |clt, srv| {
        let connect = move || {
            let clt = clt.clone();
            async move { clt.open_bi().await }
        };
        assert_bindgen_sync(connect, async || srv.accept_bi().await).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_quic_bindgen_async() -> anyhow::Result<()> {
    wrpc_test::with_quic(async |clt, srv| {
        let (tx, rx) = clt.open_bi().await?;
        assert_bindgen_async(tx, rx, srv.accept_bi()).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_quic_dynamic() -> anyhow::Result<()> {
    wrpc_test::with_quic(async |clt, srv| {
        assert_dynamic(async || clt.open_bi().await, async || srv.accept_bi().await).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_quic_loopback() -> anyhow::Result<()> {
    wrpc_test::with_quic(async |clt, srv| {
        let connect = async || clt.open_bi().await;
        let accept = async || srv.accept_bi().await;
        let (tx, rx) = connect().await?;
        wrpc_test::assert_single_invocation(tx, rx, accept()).await?;
        wrpc_test::assert_invalid_data_resets(connect, accept).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_quic_stopped_mid_params() -> anyhow::Result<()> {
    wrpc_test::with_quic(async |clt, srv| {
        let param: BoxStream<'static, Bytes> = Box::pin(
            stream::repeat(Bytes::from_static(b"x")).then(|chunk| async {
                yield_now().await;
                chunk
            }),
        );
        let (res, srv) = join!(
            async {
                let (tx, rx) = clt.open_bi().await.expect("failed to open stream");
                invoke_blocking::<(BoxStream<'static, Bytes>,), (u32,), _>(
                    Framed::new(tx, rx),
                    "foo",
                    "bar",
                    (param,),
                )
                .await
            },
            async {
                let (mut stx, mut srx) =
                    srv.accept_bi().await.context("failed to accept stream")?;
                srx.read_exact(&mut [0; 1])
                    .await
                    .context("failed to read invocation")?;
                srx.stop(0u32.into()).context("failed to stop stream")?;
                stx.write_all(b"\0\x01\x2a\0\0")
                    .await
                    .context("failed to write results")?;
                stx.finish().context("failed to finish stream")?;
                anyhow::Ok(())
            }
        );
        srv?;
        res.expect_err("stopping the parameters must fail the invocation");
        Ok(())
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_webtransport_bindgen_sync() -> anyhow::Result<()> {
    wrpc_test::with_webtransport(async |clt, srv| {
        let connect = move || {
            let clt = clt.clone();
            async move { clt.open_bi().await }
        };
        assert_bindgen_sync(connect, async || srv.accept_bi().await).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_webtransport_bindgen_async() -> anyhow::Result<()> {
    wrpc_test::with_webtransport(async |clt, srv| {
        let (tx, rx) = clt.open_bi().await?;
        assert_bindgen_async(tx, rx, srv.accept_bi()).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_webtransport_dynamic() -> anyhow::Result<()> {
    wrpc_test::with_webtransport(async |clt, srv| {
        assert_dynamic(async || clt.open_bi().await, async || srv.accept_bi().await).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_webtransport_loopback() -> anyhow::Result<()> {
    wrpc_test::with_webtransport(async |clt, srv| {
        let connect = async || clt.open_bi().await;
        let accept = async || srv.accept_bi().await;
        let (tx, rx) = connect().await?;
        wrpc_test::assert_single_invocation(tx, rx, accept()).await?;
        wrpc_test::assert_invalid_data_resets(connect, accept).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_webtransport_stopped_mid_params() -> anyhow::Result<()> {
    wrpc_test::with_webtransport(async |clt, srv| {
        let param: BoxStream<'static, Bytes> = Box::pin(
            stream::repeat(Bytes::from_static(b"x")).then(|chunk| async {
                yield_now().await;
                chunk
            }),
        );
        let (res, srv) = join!(
            async {
                let (tx, rx) = clt.open_bi().await.expect("failed to open stream");
                invoke_blocking::<(BoxStream<'static, Bytes>,), (u32,), _>(
                    Framed::new(tx, rx),
                    "foo",
                    "bar",
                    (param,),
                )
                .await
            },
            async {
                let (mut stx, mut srx) =
                    srv.accept_bi().await.context("failed to accept stream")?;
                srx.read_exact(&mut [0; 1])
                    .await
                    .context("failed to read invocation")?;
                srx.stop(0).context("failed to stop stream")?;
                stx.write_all(b"\0\x01\x2a\0\0")
                    .await
                    .context("failed to write results")?;
                stx.finish().context("failed to finish stream")?;
                anyhow::Ok(())
            }
        );
        srv?;
        res.expect_err("stopping the parameters must fail the invocation");
        Ok(())
    })
    .await
}

#[cfg(feature = "http")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_http_bindgen_sync() -> anyhow::Result<()> {
    use std::sync::OnceLock;

    use http::request::Parts;
    use http_body_util::combinators::UnsyncBoxBody;
    use hyper::client::conn::http2::SendRequest;

    let conn: Arc<OnceLock<(SendRequest<UnsyncBoxBody<Bytes, std::io::Error>>, Parts)>> =
        Arc::default();
    let handler = sync_bindgen::Handler {
        counts: Arc::default(),
        count: {
            let conn = Arc::clone(&conn);
            move || {
                let clt = conn.get().cloned();
                async move {
                    let mut clt = clt.context("connection missing")?;
                    sync_bindgen::count(&mut clt).await
                }
            }
        },
    };
    wrpc_test::with_http2(
        async |stream| {
            wrpc_test::serve_http2(stream, move |_, Header { instance, name }, srv| {
                let handler = handler.clone();
                async move {
                    let Ok(fut) =
                        sync_bindgen::sync_server::handle(&handler, &instance, &name, srv)
                    else {
                        bail!("`{instance}#{name}` is not exported");
                    };
                    fut.await
                }
            })
            .await
            .context("failed to serve HTTP/2 connection")
        },
        |parts, sender| async move {
            let mut clt = (sender, parts);
            _ = conn.set(clt.clone());
            assert_sync(&mut clt).await
        },
    )
    .await
}

#[cfg(feature = "http")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_http_bindgen_async() -> anyhow::Result<()> {
    wrpc_test::with_http2(
        async |stream| {
            wrpc_test::serve_http2(stream, |_, Header { instance, name }, srv| async move {
                let Ok(fut) =
                    async_bindgen::bindings::handle(&async_bindgen::Handler, &instance, &name, srv)
                else {
                    bail!("`{instance}#{name}` is not exported");
                };
                fut.await
            })
            .await
            .context("failed to serve HTTP/2 connection")
        },
        |parts, sender| async move { assert_async(&mut (sender, parts)).await },
    )
    .await
}

#[cfg(feature = "http")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_http_dynamic() -> anyhow::Result<()> {
    use http_body_util::BodyExt;
    use hyper::client::conn::http2::handshake;
    use hyper::server::conn::http2::Builder;
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use wrpc_http::{Body, Listener, OutgoingRequest};

    let (clt, srv) = duplex(65536);
    let (mut sender, conn) = handshake(TokioExecutor::default(), TokioIo::new(clt)).await?;
    spawn(conn);
    let mut lis = Listener::new(&Builder::new(TokioExecutor::default()), TokioIo::new(srv));
    try_join!(
        async {
            let mut inv = lis.next().await;
            for func in ["sync", "async"] {
                let (_, Header { instance, name }, srv) = inv.context("missing invocation")??;
                ensure!(
                    instance == "test" && name == func,
                    "unexpected `{instance}.{name}`"
                );
                let (res, next) = join!(
                    async {
                        if func == "sync" {
                            serve::<SyncValues, SyncValues, _, _, _>(srv, async |params| {
                                assert_eq!(params, sync_values());
                                anyhow::Ok(sync_values())
                            })
                            .await
                        } else {
                            serve::<AsyncValues, AsyncValues, _, _, _>(srv, |params| async move {
                                assert_async_values(params, future::ready(Ok(()))).await?;
                                anyhow::Ok(async_values())
                            })
                            .await
                        }
                    },
                    lis.next()
                );
                res?;
                inv = next;
            }
            let (_, Header { instance, name }, ..) = inv.context("missing invocation")??;
            ensure!(
                instance == "test" && name == "reset",
                "unexpected `{instance}.{name}`"
            );
            ensure!(lis.next().await.is_none(), "connection must close cleanly");
            Ok(())
        },
        async move {
            let (parts, ()) = http::Request::new(()).into_parts();
            sender.ready().await?;
            let res = invoke_blocking::<SyncValues, SyncValues, _>(
                OutgoingRequest {
                    parts: parts.clone(),
                    send: |req: http::Request<Body<_>>| {
                        sender.send_request(req.map(BodyExt::boxed_unsync))
                    },
                },
                "test",
                "sync",
                sync_values(),
            )
            .await?;
            assert_eq!(res, sync_values());

            let (res, io) = invoke::<AsyncValues, AsyncValues, _>(
                OutgoingRequest {
                    parts: parts.clone(),
                    send: |req: http::Request<Body<_>>| {
                        sender.send_request(req.map(BodyExt::boxed_unsync))
                    },
                },
                "test",
                "async",
                async_values(),
            )
            .await?;
            assert_async_values(res, io).await?;

            sender.ready().await?;
            let res = invoke_blocking::<(String,), (String,), _>(
                OutgoingRequest {
                    parts,
                    send: |req: http::Request<Body<_>>| {
                        sender.send_request(req.map(BodyExt::boxed_unsync))
                    },
                },
                "test",
                "reset",
                ("arg".into(),),
            )
            .await;
            ensure!(res.is_err(), "`test.reset` should have failed");
            Ok(())
        },
    )?;
    Ok(())
}

#[instrument(skip_all, ret)]
async fn assert_oneshot(
    clt_tx: impl AsyncWrite + Send + Unpin,
    clt_rx: impl AsyncRead + Send + Unpin,
    tx: impl AsyncWrite + Send + Sync + Unpin + 'static,
    rx: impl AsyncRead + Send + Sync + Unpin + 'static,
) -> anyhow::Result<()> {
    join!(
        async {
            let (a,) = invoke_blocking::<(u32,), (String,), _>(
                Framed::new(clt_tx, clt_rx),
                "foo",
                "bar",
                (42,),
            )
            .await
            .expect("failed to invoke `foo.bar`");
            assert_eq!(a, "test");
        },
        async {
            let mut lis = Listener::new(tx, rx);
            let srv = next_invocation(&mut lis, "foo", "bar")
                .await
                .expect("failed to read invocation header");
            serve::<(u32,), (&str,), _, _, _>(srv, async |params| {
                assert_eq!(params, (42,));
                anyhow::Ok(("test",))
            })
            .await
            .expect("failed to serve invocation");
        }
    );
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_oneshot_no_params() -> anyhow::Result<()> {
    let (clt, srv) = duplex(1024);
    let (mut clt_rx, mut clt_tx) = split(clt);
    let (rx, tx) = split(srv);
    let fut = async {
        join!(
            async {
                let mut buf = BytesMut::default();
                HeaderEncoder
                    .encode(
                        HeaderRef {
                            instance: "foo",
                            name: "bar",
                        },
                        &mut buf,
                    )
                    .expect("failed to encode invocation");
                buf.extend_from_slice(b"\0\0");
                clt_tx
                    .write_all(&buf)
                    .await
                    .expect("failed to invoke `foo.bar`");
                let mut res = Vec::default();
                clt_rx
                    .read_to_end(&mut res)
                    .await
                    .expect("failed to read results");
                assert_eq!(res, b"\0\x05\x04test\0\0");
                drop(clt_tx);
            },
            async {
                let mut lis = Listener::new(tx, rx);
                let srv = next_invocation(&mut lis, "foo", "bar")
                    .await
                    .expect("failed to read invocation header");
                serve::<(), (&str,), _, _, _>(srv, async |()| anyhow::Ok(("test",)))
                    .await
                    .expect("failed to serve invocation");
            }
        )
    };
    tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .context("invocation timed out")?;
    Ok(())
}

#[test_log::test(tokio::test)]
async fn rust_reuse_streams() -> anyhow::Result<()> {
    let (clt, srv) = duplex(1024);
    let (clt_rx, clt_tx) = split(clt);
    let (srv_rx, srv_tx) = split(srv);
    try_join!(
        async {
            let mut clt = Framed::new(clt_tx, clt_rx);
            for n in [1u32, 2] {
                let (m,) =
                    invoke_blocking::<(u32,), (u32,), _>(&mut clt, "foo", "bar", (n,)).await?;
                ensure!(m == n + 1);
            }
            for n in [3u32, 4] {
                let enc = <(u32,) as Encode>::Encoder::default();
                HeaderEncoder.encode(
                    HeaderRef {
                        instance: "foo",
                        name: "bar",
                    },
                    clt.tx.write_buffer_mut(),
                )?;
                let tx = transmit(enc, &mut clt.tx, (n,))?;
                tx.await?;
            }
            for n in [3u32, 4] {
                let dec = <(u32,) as Decode>::Decoder::default();
                let ((m,), io) = receive(dec, &mut clt.rx).await?;
                io.await?;
                ensure!(m == n + 1);
            }
            anyhow::Ok(())
        },
        async {
            let mut lis = Listener::new(srv_tx, srv_rx);
            for _ in 0..4 {
                let Some((Header { instance, name }, srv)) = lis.accept().await? else {
                    bail!("stream ended");
                };
                ensure!(instance == "foo" && name == "bar");
                serve::<(u32,), (u32,), _, _, _>(srv, async |(n,)| anyhow::Ok((n + 1,))).await?;
            }
            anyhow::Ok(())
        },
    )?;
    Ok(())
}

#[test_log::test(tokio::test)]
async fn rust_pipelining() -> anyhow::Result<()> {
    const MAX_INVOCATIONS: usize = 2;

    mod bindings {
        wit_bindgen_wrpc::generate!({
            inline: "
                package wrpc-test:pipelining;

                interface handler {
                    f: func(n: u32) -> u32;
                }

                world pipelining {
                    export handler;
                }
            ",
        });
    }

    #[derive(Clone)]
    struct Handler(Arc<Notify>);

    impl bindings::exports::wrpc_test::pipelining::handler::Handler for Handler {
        async fn f(&self, n: u32) -> anyhow::Result<u32> {
            if n == 1 {
                self.0.notified().await;
            } else {
                self.0.notify_one();
            }
            Ok(n)
        }
    }

    let (clt, srv) = duplex(1024);
    let (clt_rx, clt_tx) = split(clt);
    let (srv_rx, srv_tx) = split(srv);
    let fut = async {
        try_join!(
            async {
                let mut clt = Framed::new(clt_tx, clt_rx);
                for n in [1u32, 2] {
                    HeaderEncoder.encode(
                        HeaderRef {
                            instance: "wrpc-test:pipelining/handler",
                            name: "f",
                        },
                        clt.tx.write_buffer_mut(),
                    )?;
                    let tx = transmit(<(u32,) as Encode>::Encoder::default(), &mut clt.tx, (n,))?;
                    tx.await?;
                }
                for n in [1u32, 2] {
                    let dec = <(u32,) as Decode>::Decoder::default();
                    let ((m,), io) = receive(dec, &mut clt.rx).await?;
                    io.await?;
                    ensure!(m == n);
                }
                anyhow::Ok(())
            },
            async {
                let handler = Handler(Arc::default());
                let mut lis = Listener::new(srv_tx, srv_rx);
                let mut invocations = FuturesUnordered::default();
                loop {
                    select! {
                        next = lis.accept(), if invocations.len() < MAX_INVOCATIONS => {
                            let Some((Header { instance, name }, srv)) = next? else {
                                break;
                            };
                            let Ok(fut) = bindings::handle(&handler, &instance, &name, srv) else {
                                bail!("`{instance}#{name}` is not exported");
                            };
                            invocations.push(fut);
                        }
                        Some(res) = invocations.next() => res?,
                    }
                }
                invocations.try_collect::<()>().await
            },
        )
    };
    time::timeout(Duration::from_secs(10), fut)
        .await
        .context("pipelined invocation blocked")??;
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_oneshot_raw() -> anyhow::Result<()> {
    let (mut clt, mut srv) = duplex(1024);

    join!(
        async {
            let (rx, tx) = split(&mut clt);
            let (a,) =
                invoke_blocking::<(u32,), (String,), _>(Framed::new(tx, rx), "foo", "bar", (42,))
                    .await
                    .expect("failed to invoke `foo.bar`");
            assert_eq!(a, "test");
        },
        async {
            let mut rx = FramedRead::new(&mut srv, HeaderDecoder::default());
            let header = rx.try_next().await.expect("failed to read header");
            let header = header.expect("connection closed");
            assert_eq!(header.instance, "foo");
            assert_eq!(header.name, "bar");
            let FramedParts { io, read_buf, .. } = rx.into_parts();
            let mut req = [0; 5];
            read_buf
                .as_ref()
                .chain(io)
                .read_exact(&mut req)
                .await
                .expect("failed to read request");
            assert_eq!(&req, b"\0\x01\x2a\0\0");

            let mut data = bytes::BytesMut::default();
            let enc = <(&str,) as Encode>::Encoder::default();
            wrpc_transport::pack(enc, ("test",), &mut data).expect("failed to encode");

            let mut res = bytes::BytesMut::default();
            FrameEncoder
                .encode(
                    FrameRef {
                        path: &[],
                        data: &data,
                    },
                    &mut res,
                )
                .expect("failed to encode response frame");
            eprintln!("response: `{res:?}`");

            srv.write_all(&res).await.expect("failed to write response");
            srv.write_all(b"\0\0")
                .await
                .expect("failed to close response root");
            drop(srv);
        }
    );
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_oneshot_raw_strings_invalid_utf8() -> anyhow::Result<()> {
    mod bindings {
        wit_bindgen_wrpc::generate!({
            inline: "
                package wrpc-test:raw-strings;

                interface strings {
                    bar: func() -> string;
                }

                world client {
                    import strings;
                }
            ",
            raw_strings,
        });
    }

    let (clt, srv) = duplex(1024);
    let (clt_rx, clt_tx) = split(clt);
    let (rx, tx) = split(srv);
    join!(
        async {
            bindings::wrpc_test::raw_strings::strings::bar(Framed::new(clt_tx, clt_rx))
                .await
                .expect_err("invalid UTF-8 must be rejected");
        },
        async {
            let mut lis = Listener::new(tx, rx);
            let srv = next_invocation(&mut lis, "wrpc-test:raw-strings/strings", "bar")
                .await
                .expect("failed to read invocation header");
            serve::<(), (Bytes,), _, _, _>(srv, async |()| {
                anyhow::Ok((Bytes::from_static(b"\xff"),))
            })
            .await
            .expect("failed to serve invocation");
        }
    );
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_oneshot_raw_strings_invalid_utf8_result() -> anyhow::Result<()> {
    mod bindings {
        wit_bindgen_wrpc::generate!({
            inline: "
                package wrpc-test:raw-strings;

                interface strings {
                    record rec {
                        s: string,
                        l: list<string>,
                        o: option<string>,
                        t: tuple<string, u8>,
                        r: result<string, string>,
                    }
                    variant var { s(string), l(list<string>) }
                    bar: func() -> string;
                    baz: func() -> rec;
                    qux: func() -> var;
                }

                world server {
                    export strings;
                }
            ",
            raw_strings,
        });
    }
    use bindings::exports::wrpc_test::raw_strings::strings::{Rec, Var};

    #[derive(Clone, Copy)]
    struct Handler;

    impl bindings::exports::wrpc_test::raw_strings::strings::Handler for Handler {
        async fn bar(&self) -> anyhow::Result<Vec<u8>> {
            Ok(b"\xff".to_vec())
        }

        async fn baz(&self) -> anyhow::Result<Rec> {
            Ok(Rec {
                s: b"\xff".to_vec(),
                l: Vec::default(),
                o: None,
                t: (Vec::default(), 0),
                r: Ok(Vec::default()),
            })
        }

        async fn qux(&self) -> anyhow::Result<Var> {
            Ok(Var::S(b"\xff".to_vec()))
        }
    }

    for func in ["bar", "baz", "qux"] {
        let (clt, srv) = duplex(1024);
        let (clt_rx, clt_tx) = split(clt);
        let (rx, tx) = split(srv);
        join!(
            async {
                invoke_blocking::<(), (String,), _>(
                    Framed::new(clt_tx, clt_rx),
                    "wrpc-test:raw-strings/strings",
                    func,
                    (),
                )
                .await
                .expect_err("invalid UTF-8 must not be transmitted");
            },
            async {
                let mut lis = Listener::new(tx, rx);
                let next = lis
                    .accept()
                    .await
                    .expect("failed to read invocation header");
                let (header, srv) = next.expect("connection closed");
                let Ok(fut) = bindings::handle(&Handler, &header.instance, &header.name, srv)
                else {
                    panic!("`{func}` is not exported");
                };
                fut.await
                    .expect_err("invalid UTF-8 must not be transmitted");
            }
        );
    }
    Ok(())
}

#[test_log::test(tokio::test)]
async fn rust_oneshot_borrowed() -> anyhow::Result<()> {
    mod bindings {
        wit_bindgen_wrpc::generate!({
            inline: "
                package wrpc-test:borrowed;

                world server {
                    export greet: func(name: string) -> string;
                }
            ",
        });
    }

    #[derive(Clone, Copy)]
    struct Handler;

    impl bindings::Handler for Handler {
        async fn greet(&self, name: String) -> anyhow::Result<String> {
            Ok(format!("Hello, {name}"))
        }
    }

    let mut req = BytesMut::default();
    HeaderEncoder.encode(
        HeaderRef {
            instance: "",
            name: "greet",
        },
        &mut req,
    )?;
    FrameEncoder.encode(
        FrameRef {
            path: &[],
            data: b"\x05world",
        },
        &mut req,
    )?;
    FrameEncoder.encode(
        FrameRef {
            path: &[],
            data: &[],
        },
        &mut req,
    )?;
    let mut res = Vec::default();
    let mut lis = Listener::new(&mut res, req.as_ref());
    let next = lis.accept().await?;
    let (Header { instance, name }, srv) = next.context("connection closed")?;
    let Ok(fut) = bindings::handle(&Handler, &instance, &name, srv) else {
        bail!("`{instance}#{name}` is not exported");
    };
    fut.await?;
    drop(lis);
    let (greeting,) = invoke_blocking::<(&str,), (String,), _>(
        Framed::new(sink(), res.as_slice()),
        "",
        "greet",
        ("world",),
    )
    .await?;
    assert_eq!(greeting, "Hello, world");
    Ok(())
}

#[test_log::test(tokio::test)]
async fn rust_bindgen_not_send() -> anyhow::Result<()> {
    mod bindings {
        wit_bindgen_wrpc::generate!({
            inline: "
                package wrpc-test:not-send;

                interface greeter {
                    greet: func(name: future<string>) -> string;
                }

                world greeter-world {
                    import greeter;
                    export greeter;
                }
            ",
        });
    }

    #[derive(Clone, Copy)]
    struct Handler;

    impl bindings::exports::wrpc_test::not_send::greeter::Handler for Handler {
        async fn greet(&self, name: BoxFuture<'static, String>) -> anyhow::Result<String> {
            let name = name.await;
            Ok(format!("Hello, {name}"))
        }
    }

    let (clt, srv) = duplex(1024);
    let (clt_rx, clt_tx) = split(clt);
    let clt_rx: Box<dyn AsyncRead + Unpin> = Box::new(clt_rx);
    let clt_tx: Box<dyn AsyncWrite + Unpin> = Box::new(clt_tx);
    let clt = Framed::new(clt_tx, clt_rx);
    let (rx, tx) = split(srv);
    let (greeting, ()) = try_join!(
        async {
            let (greeting, io) = bindings::wrpc_test::not_send::greeter::greet(
                clt,
                Box::pin(async { "world".into() }),
            )
            .await?;
            io.await?;
            anyhow::Ok(greeting)
        },
        async {
            let mut lis = Listener::new(tx, rx);
            while let Some((Header { instance, name }, srv)) = lis.accept().await? {
                let Ok(fut) = bindings::handle(&Handler, &instance, &name, srv) else {
                    bail!("`{instance}#{name}` is not exported");
                };
                fut.await?;
            }
            anyhow::Ok(())
        },
    )?;
    assert_eq!(greeting, "Hello, world");
    Ok(())
}

#[test_log::test(tokio::test)]
async fn rust_bindgen_instance_names() -> anyhow::Result<()> {
    mod bindings {
        wit_bindgen_wrpc::generate!({
            inline: "
                package wrpc-test:instance-names@1.2.3+build;

                interface iface {
                    f: func();
                }

                world instance-names {
                    import iface;
                    import foo: interface {
                        f: func();
                    }
                    export iface;
                    export foo: interface {
                        f: func();
                    }
                }
            ",
        });
    }

    #[derive(Clone, Copy)]
    struct Handler;

    impl bindings::exports::wrpc_test::instance_names::iface::Handler for Handler {
        async fn f(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    impl bindings::exports::foo::Handler for Handler {
        async fn f(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    let (clt, srv) = duplex(1024);
    let (clt_rx, clt_tx) = split(clt);
    let (rx, tx) = split(srv);
    let ((), instances) = try_join!(
        async {
            let mut clt = Framed::new(clt_tx, clt_rx);
            bindings::wrpc_test::instance_names::iface::f(&mut clt).await?;
            bindings::foo::f(&mut clt).await?;
            anyhow::Ok(())
        },
        async {
            let mut lis = Listener::new(tx, rx);
            let mut instances = Vec::default();
            while let Some((Header { instance, name }, srv)) = lis.accept().await? {
                let Ok(fut) = bindings::handle(&Handler, &instance, &name, srv) else {
                    bail!("`{instance}#{name}` is not exported");
                };
                fut.await?;
                instances.push(instance);
            }
            anyhow::Ok(instances)
        },
    )?;
    assert_eq!(instances, ["wrpc-test:instance-names/iface@1", "foo"]);
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_oneshot_duplex() -> anyhow::Result<()> {
    let (clt, srv) = duplex(1024);
    let (clt_rx, clt_tx) = split(clt);
    let (rx, tx) = split(srv);
    assert_oneshot(clt_tx, clt_rx, tx, rx).await
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_oneshot_uds() -> anyhow::Result<()> {
    let (mut clt, srv) = tokio::net::UnixStream::pair()?;
    let (clt_rx, clt_tx) = clt.split();
    let (rx, tx) = srv.into_split();
    assert_oneshot(clt_tx, clt_rx, tx, rx).await
}

#[test_log::test(tokio::test)]
async fn rust_stream_of_futures() -> anyhow::Result<()> {
    fn pending(rx: oneshot::Receiver<u8>) -> BoxFuture<'static, u8> {
        Box::pin(async { rx.await.unwrap() })
    }

    async fn next<const N: usize>(
        s: &mut BoxStream<'static, Vec<BoxFuture<'static, u8>>>,
    ) -> anyhow::Result<[BoxFuture<'static, u8>; N]> {
        let chunk = time::timeout(Duration::from_secs(5), s.next()).await?;
        let chunk = chunk.context("stream ended")?;
        let Ok(chunk) = chunk.try_into() else {
            bail!("unexpected chunk length");
        };
        Ok(chunk)
    }

    async fn resolve(f: BoxFuture<'static, u8>) -> anyhow::Result<u8> {
        let v = time::timeout(Duration::from_secs(5), f).await?;
        Ok(v)
    }

    let (clt_tx, srv_rx) = duplex(1024);
    let (srv_tx, clt_rx) = duplex(1024);
    let (chunks_tx, mut chunks_rx) = mpsc::unbounded_channel();
    let s: BoxStream<'static, Vec<BoxFuture<'static, u8>>> =
        Box::pin(stream::poll_fn(move |cx| chunks_rx.poll_recv(cx)));
    let srv = async move {
        let mut srv_rx = FramedRead::new(srv_rx, HeaderDecoder::default());
        srv_rx.try_next().await?;
        let srv_rx = srv_rx.map_decoder(|_| FrameDecoder::default());
        let dec =
            <(BoxStream<'static, Vec<BoxFuture<'static, u8>>>,) as Decode>::Decoder::default();
        let ((s,), io) = receive(dec, srv_rx).await?;
        let enc = <() as Encode>::Encoder::default();
        let tx = transmit(enc, FramedWrite::new(srv_tx, FrameEncoder), ())?;
        tx.await?;
        anyhow::Ok((s, io))
    };
    let clt = invoke::<(BoxStream<'static, Vec<BoxFuture<'static, u8>>>,), (), _>(
        Framed::new(clt_tx, clt_rx),
        "test",
        "f",
        (s,),
    );
    let ((mut s, srv_io), ((), clt_io)) = try_join!(srv, async {
        clt.await.context("failed to invoke `test.f`")
    })?;
    let srv_io = spawn(srv_io);
    let clt_io = spawn(clt_io);

    let (p0_tx, p0_rx) = oneshot::channel();
    let Ok(()) = chunks_tx.send(vec![pending(p0_rx), Box::pin(future::ready(1))]) else {
        bail!("stream dropped");
    };
    let [f0, f1] = next(&mut s).await?;
    let f1 = resolve(f1).await?;
    assert_eq!(f1, 1);
    _ = p0_tx.send(10);
    let f0 = resolve(f0).await?;
    assert_eq!(f0, 10);

    let (p2_tx, p2_rx) = oneshot::channel();
    let Ok(()) = chunks_tx.send(vec![pending(p2_rx), Box::pin(future::ready(3))]) else {
        bail!("stream dropped");
    };
    let [f2, f3] = next(&mut s).await?;
    let f3 = resolve(f3).await?;
    assert_eq!(f3, 3);
    _ = p2_tx.send(12);
    let f2 = resolve(f2).await?;
    assert_eq!(f2, 12);

    let Ok(()) = chunks_tx.send(vec![Box::pin(future::ready(4))]) else {
        bail!("stream dropped");
    };
    let [f4] = next(&mut s).await?;
    let f4 = resolve(f4).await?;
    assert_eq!(f4, 4);
    drop(chunks_tx);
    assert!(s.next().await.is_none());
    srv_io.await??;
    clt_io.await??;
    Ok(())
}
