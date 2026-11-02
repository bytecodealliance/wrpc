#![allow(clippy::disallowed_names, clippy::type_complexity)]

pub mod common;

use core::future::poll_fn;
use core::iter;
use core::pin::{Pin, pin};
use core::task::Poll;
use core::time::Duration;

use std::sync::{Arc, Mutex};

use anyhow::{Context as _, bail, ensure};
use bytes::{Bytes, BytesMut};
use common::{assert_async, assert_sync};
use futures::future::BoxFuture;
use futures::stream::{BoxStream, FuturesUnordered};
use futures::{Stream, StreamExt as _, TryStreamExt as _, future, stream};
use tokio::io::{AsyncRead, AsyncReadExt as _, BufReader, split};
use tokio::task::yield_now;
use tokio::{join, spawn, try_join};
use tokio_util::io::StreamReader;
use tracing::instrument;
use wrpc_transport::frame::Oneshot;
use wrpc_transport::{InvokeExt as _, ResourceBorrow, ResourceOwn, ServeExt as _};

async fn serve_bindgen<S, F>(
    invocations: Vec<(&'static str, &'static str, S)>,
    client: impl Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<()>
where
    S: Stream<Item = anyhow::Result<F>> + Unpin,
    F: Future<Output = anyhow::Result<()>>,
{
    let mut invocations = stream::select_all(invocations.into_iter().map(|(.., s)| s));
    let mut served = FuturesUnordered::default();
    let mut client = pin!(client);
    poll_fn(|cx| {
        while let Poll::Ready(Some(invocation)) = invocations.poll_next_unpin(cx) {
            let invocation = invocation?;
            served.push(invocation);
        }
        while let Poll::Ready(Some(res)) = served.poll_next_unpin(cx) {
            res?;
        }
        client.as_mut().poll(cx)
    })
    .await?;
    served.try_collect().await
}

#[instrument(skip_all, ret)]
async fn assert_bindgen_sync<C, I>(cx: C, clt: &I, srv: &impl wrpc::Serve) -> anyhow::Result<()>
where
    C: Clone + Send + Sync + 'static,
    I: wrpc::Invoke<Context = C> + Clone + 'static,
{
    mod sync_client {
        wit_bindgen_wrpc::generate!({
            world: "sync-client",
            path: "tests/wit",
        });
    }

    mod sync_server {
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
    struct Handler<I: wrpc::Invoke> {
        counts: Arc<Mutex<Vec<u32>>>,
        clt: I,
        cx: I::Context,
    }

    impl<I: wrpc::Invoke> Handler<I> {
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

    impl<C: Send + Sync, I: wrpc::Invoke + 'static> resources::HandlerFoo<C> for Handler<I> {
        async fn new(&self, _cx: C) -> anyhow::Result<ResourceOwn<Foo>> {
            Ok(ResourceOwn::from(Bytes::from_static(b"foo")))
        }

        async fn foo(&self, _cx: C, v: ResourceOwn<Foo>) -> anyhow::Result<String> {
            ensure_foo(v)?;
            Ok("foo".into())
        }

        async fn bar(&self, _cx: C, self_: ResourceBorrow<Foo>) -> anyhow::Result<String> {
            ensure_foo(self_)?;
            Ok("bar".into())
        }
    }

    impl<C: Send + Sync, I: wrpc::Invoke + 'static> resources::HandlerCounter<C> for Handler<I> {
        async fn new(&self, _cx: C, initial: u32) -> anyhow::Result<ResourceOwn<Counter>> {
            Ok(self.push(initial))
        }

        async fn clone_counter(
            &self,
            _cx: C,
            self_: ResourceBorrow<Counter>,
        ) -> anyhow::Result<ResourceOwn<Counter>> {
            let count = self.with(self_, |count| *count)?;
            Ok(self.push(count))
        }

        async fn get_count(&self, _cx: C, self_: ResourceBorrow<Counter>) -> anyhow::Result<u32> {
            self.with(self_, |count| *count)
        }

        async fn increment_by(
            &self,
            _cx: C,
            self_: ResourceBorrow<Counter>,
            num: u32,
        ) -> anyhow::Result<()> {
            self.with(self_, |count| *count = count.wrapping_add(num))
        }

        async fn sum(
            &self,
            _cx: C,
            a: ResourceBorrow<Counter>,
            b: ResourceBorrow<Counter>,
        ) -> anyhow::Result<u32> {
            let a = self.with(a, |count| *count)?;
            let b = self.with(b, |count| *count)?;
            Ok(a.wrapping_add(b))
        }
    }

    impl<C: Send + Sync, I: wrpc::Invoke + 'static> resources::Handler<C> for Handler<I> {
        async fn bar(&self, _cx: C, v: ResourceBorrow<Foo>) -> anyhow::Result<String> {
            ensure_foo(v)?;
            Ok("bar".into())
        }
    }

    impl<C: Send + Sync, I: wrpc::Invoke + 'static> sync_server::exports::strange::Handler<C>
        for Handler<I>
    {
        async fn bar(&self, _cx: C, v: ResourceBorrow<Foo>) -> anyhow::Result<u64> {
            ensure_foo(v)?;
            Ok(42)
        }
    }

    impl<C, I> sync_server::Handler<C> for Handler<I>
    where
        C: Send + Sync,
        I: wrpc::Invoke + 'static,
        I::Context: Clone,
    {
        async fn ping(&self, _cx: C) -> anyhow::Result<String> {
            use sync_client::wrpc_test::integration::resources::Counter;

            let counter = Counter::new(&self.clt, self.cx.clone(), 42).await?;
            let n = Counter::get_count(&self.clt, self.cx.clone(), &counter.as_borrow()).await?;
            ensure!(n == 42);
            Ok("pong".into())
        }
    }

    impl<C: Send + Sync, I: wrpc::Invoke + 'static> sync::Handler<C> for Handler<I> {
        async fn fallible(&self, _cx: C, ok: bool) -> anyhow::Result<Result<bool, String>> {
            Ok(if ok { Ok(true) } else { Err("test".into()) })
        }

        async fn numbers(
            &self,
            _cx: C,
        ) -> anyhow::Result<(u8, u16, u32, u64, i8, i16, i32, i64, f32, f64)> {
            Ok((1, 2, 3, 4, 5, 6, 7, 8, 9., 10.))
        }

        async fn with_flags(&self, _cx: C, a: bool, b: bool, c: bool) -> anyhow::Result<Abc> {
            let mut v = Abc::empty();
            v.set(Abc::A, a);
            v.set(Abc::B, b);
            v.set(Abc::C, c);
            Ok(v)
        }

        async fn with_variant_option(&self, _cx: C, ok: bool) -> anyhow::Result<Option<Var>> {
            Ok(ok.then(|| Var::Var(rec("bar"))))
        }

        async fn with_variant_list(&self, _cx: C) -> anyhow::Result<Vec<Var>> {
            Ok(vec![
                Var::Empty,
                Var::Var(rec("foo")),
                Var::Empty,
                Var::Empty,
                Var::Empty,
                Var::Var(rec("bar")),
            ])
        }

        async fn with_record(&self, _cx: C) -> anyhow::Result<Rec> {
            Ok(rec("foo"))
        }

        async fn with_record_list(&self, _cx: C, n: u8) -> anyhow::Result<Vec<Rec>> {
            Ok((0..n).map(|i| rec(i.to_string())).collect())
        }

        async fn with_record_tuple(&self, _cx: C) -> anyhow::Result<(Rec, Rec)> {
            Ok((rec("0"), rec("1")))
        }

        async fn with_enum(&self, _cx: C) -> anyhow::Result<Foobar> {
            Ok(Foobar::Bar)
        }

        async fn identity_char(&self, _cx: C, c: char) -> anyhow::Result<char> {
            Ok(c)
        }

        async fn identity_option_lists(
            &self,
            _cx: C,
            a: Option<Bytes>,
            b: Option<Vec<String>>,
        ) -> anyhow::Result<(Option<Bytes>, Option<Vec<String>>)> {
            Ok((a, b))
        }
    }

    impl<C: Send + Sync, I: wrpc::Invoke + 'static> sync_server::exports::foo::Handler<C>
        for Handler<I>
    {
        async fn f(&self, _cx: C, x: String) -> anyhow::Result<u32> {
            assert_eq!(x, "f");
            Ok(42)
        }

        async fn foo(&self, _cx: C, x: String) -> anyhow::Result<()> {
            assert_eq!(x, "foo");
            Ok(())
        }
    }

    let handler = Handler {
        counts: Arc::default(),
        clt: clt.clone(),
        cx: cx.clone(),
    };
    let invocations = sync_server::serve(srv, handler).await?;
    assert!(
        invocations
            .iter()
            .any(|(instance, name, ..)| instance.is_empty() && *name == "ping")
    );
    serve_bindgen(invocations, assert_sync(cx, clt)).await
}

#[instrument(skip_all, ret)]
async fn assert_bindgen_async<C: Clone + Send + Sync>(
    cx: C,
    clt: &impl wrpc::Invoke<Context = C>,
    srv: &impl wrpc::Serve,
) -> anyhow::Result<()> {
    mod bindings {
        wit_bindgen_wrpc::generate!({
            world: "async-server",
            path: "tests/wit",
        });
    }

    use bindings::exports::wrpc_test::integration::async_::{self, AsyncCase, Something};

    #[derive(Clone)]
    struct Handler;

    impl<C: Send + Sync> async_::Handler<C> for Handler {
        async fn with_streams(
            &self,
            _cx: C,
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
            _cx: C,
            x: Something,
            s: BoxStream<'static, Bytes>,
        ) -> anyhow::Result<BoxFuture<'static, BoxStream<'static, Bytes>>> {
            assert_eq!(x.foo, "bar");
            Ok(Box::pin(async { s }))
        }

        async fn identity_nested_async(
            &self,
            _cx: C,
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
            _cx: C,
            v: BoxFuture<'static, ()>,
        ) -> anyhow::Result<BoxFuture<'static, ()>> {
            Ok(v)
        }

        async fn identity_cases(
            &self,
            _cx: C,
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

    let invocations = bindings::serve(srv, Handler).await?;
    serve_bindgen(invocations, assert_async(cx, clt)).await
}

#[instrument(skip_all, ret)]
async fn assert_dynamic<C: Clone + Send + Sync + 'static>(
    cx: C,
    clt: &impl wrpc::Invoke<Context = C>,
    srv: &impl wrpc::Serve,
) -> anyhow::Result<()> {
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

    async fn assert_async_values<E>(
        (a, b, mut c, d, e, f): AsyncValues,
        io: Option<impl Future<Output = Result<(), E>>>,
    ) -> anyhow::Result<()>
    where
        anyhow::Error: From<E>,
    {
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
            async {
                if let Some(io) = io {
                    io.await?;
                }
                anyhow::Ok(())
            },
        )?;
        Ok(())
    }

    let paths: Arc<[Box<[Option<usize>]>]> = Arc::from([
        Box::from([Some(0)]),
        Box::from([Some(1)]),
        Box::from([Some(2)]),
        Box::from([Some(3)]),
        Box::from([Some(4)]),
        Box::from([Some(5), None]),
    ]);
    let reset = srv
        .serve_values::<(String,), (String,)>("test", "reset", Arc::default())
        .await?;
    let sync = srv
        .serve_values::<SyncValues, SyncValues>("test", "sync", Arc::default())
        .await?;
    let async_ = srv
        .serve_values::<AsyncValues, AsyncValues>("test", "async", Arc::clone(&paths))
        .await?;
    let (mut reset, mut sync, mut async_) = (pin!(reset), pin!(sync), pin!(async_));
    try_join!(
        async {
            for spawned in [false, true] {
                let Some(inv) = reset.try_next().await? else {
                    bail!("`test.reset` missing");
                };
                if spawned {
                    spawn(async move { drop(inv) }).await?;
                } else {
                    drop(inv);
                }
            }
            for _ in 0..2 {
                let Some((.., params, io, tx)) = sync.try_next().await? else {
                    bail!("`test.sync` missing");
                };
                assert!(io.is_none());
                assert_eq!(params, sync_values());
                tx(sync_values()).await?;
            }
            for _ in 0..2 {
                let Some((.., params, io, tx)) = async_.try_next().await? else {
                    bail!("`test.async` missing");
                };
                assert_async_values(params, io).await?;
                tx(async_values()).await?;
            }
            Ok(())
        },
        async {
            for _ in 0..2 {
                let res = clt
                    .invoke_values_blocking::<_, _, (String,)>(
                        cx.clone(),
                        "test",
                        "reset",
                        ("arg",),
                        &[[]; 0],
                    )
                    .await;
                res.expect_err("`test.reset` should have failed");
            }

            let mut buf = BytesMut::default();
            wrpc_transport::pack(sync_values(), &mut buf).await?;
            let unpacked = wrpc_transport::unpack(&mut buf)?;
            for params in [sync_values(), unpacked] {
                let res: SyncValues = clt
                    .invoke_values_blocking(cx.clone(), "test", "sync", params, &[[]; 0])
                    .await?;
                assert_eq!(res, sync_values());
            }

            wrpc_transport::pack(async_values(), &mut buf).await?;
            let unpacked = wrpc_transport::unpack(&mut buf)?;
            for params in [async_values(), unpacked] {
                let (res, io) = clt
                    .invoke_values(cx.clone(), "test", "async", params, &paths)
                    .await?;
                assert_async_values(res, io).await?;
            }
            anyhow::Ok(())
        },
    )?;
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_tcp_bindgen_sync() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |clt, srv| assert_bindgen_sync((), &clt, srv).await).await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_tcp_bindgen_async() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |clt, srv| assert_bindgen_async((), &clt, srv).await).await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_tcp_dynamic() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |clt, srv| assert_dynamic((), &clt, srv).await).await
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_uds_bindgen_sync() -> anyhow::Result<()> {
    wrpc_test::serve_unix(async |clt, srv| assert_bindgen_sync((), &clt, srv).await).await
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_uds_bindgen_async() -> anyhow::Result<()> {
    wrpc_test::serve_unix(async |clt, srv| assert_bindgen_async((), &clt, srv).await).await
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_uds_dynamic() -> anyhow::Result<()> {
    wrpc_test::serve_unix(async |clt, srv| assert_dynamic((), &clt, srv).await).await
}

#[cfg(feature = "quic")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_quic_bindgen_sync() -> anyhow::Result<()> {
    wrpc_test::serve_quic(async |clt, srv| assert_bindgen_sync((), &clt, srv).await).await
}

#[cfg(feature = "quic")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_quic_bindgen_async() -> anyhow::Result<()> {
    wrpc_test::serve_quic(async |clt, srv| assert_bindgen_async((), &clt, srv).await).await
}

#[cfg(feature = "quic")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_quic_dynamic() -> anyhow::Result<()> {
    wrpc_test::serve_quic(async |clt, srv| assert_dynamic((), &clt, srv).await).await
}

#[cfg(feature = "webtransport")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_webtransport_bindgen_sync() -> anyhow::Result<()> {
    wrpc_test::serve_webtransport(async |clt, srv| assert_bindgen_sync((), &clt, srv).await).await
}

#[cfg(feature = "webtransport")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_webtransport_bindgen_async() -> anyhow::Result<()> {
    wrpc_test::serve_webtransport(async |clt, srv| assert_bindgen_async((), &clt, srv).await).await
}

#[cfg(feature = "webtransport")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_webtransport_dynamic() -> anyhow::Result<()> {
    wrpc_test::serve_webtransport(async |clt, srv| assert_dynamic((), &clt, srv).await).await
}

#[cfg(feature = "http")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_http_bindgen_sync() -> anyhow::Result<()> {
    wrpc_test::serve_http2(async |cx, clt, srv| assert_bindgen_sync(cx, &clt, srv).await).await
}

#[cfg(feature = "http")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_http_bindgen_async() -> anyhow::Result<()> {
    wrpc_test::serve_http2(async |cx, clt, srv| assert_bindgen_async(cx, &clt, srv).await).await
}

#[cfg(feature = "http")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_http_dynamic() -> anyhow::Result<()> {
    wrpc_test::serve_http2(async |cx, clt, srv| assert_dynamic(cx, &clt, srv).await).await
}

#[instrument(skip_all, ret)]
async fn assert_oneshot<T, R, W>(clt: T, rx: R, tx: W) -> anyhow::Result<()>
where
    T: wrpc::Invoke<Context = ()> + 'static,
    R: tokio::io::AsyncRead + Send + Sync + Unpin + 'static,
    W: tokio::io::AsyncWrite + Send + Sync + Unpin + 'static,
{
    let srv = Arc::new(wrpc_transport::frame::Server::default());
    let invocations = srv
        .serve_values::<(u32,), (&str,)>("foo", "bar", Arc::default())
        .await?;
    join!(
        async {
            let (a,) = clt
                .invoke_values_blocking::<_, _, (String,)>((), "foo", "bar", (42,), &[[]; 0])
                .await
                .expect("failed to invoke `foo.bar`");
            assert_eq!(a, "test");
        },
        async {
            srv.accept((), tx, BufReader::new(rx))
                .await
                .expect("failed to accept connection");
            let ((), params, rx, tx) = pin!(invocations)
                .try_next()
                .await
                .expect("failed to accept invocation")
                .expect("unexpected end of stream");
            assert!(rx.is_none());
            assert_eq!(params, (42,));
            tx(("test",)).await.expect("failed to transmit response");
        }
    );
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_oneshot_no_params_open_outgoing() -> anyhow::Result<()> {
    use wrpc::Invoke as _;

    let (clt, srv) = Oneshot::duplex(1024);
    let (rx, tx) = split(srv);
    let srv = Arc::new(wrpc_transport::frame::Server::default());
    let invocations = srv
        .serve_values::<(), (&str,)>("foo", "bar", Arc::default())
        .await?;
    let fut = async {
        join!(
            async {
                let (_tx, mut rx) = clt
                    .invoke((), "foo", "bar", Bytes::default(), &[[]; 0])
                    .await
                    .expect("failed to invoke `foo.bar`");
                let mut res = Vec::default();
                rx.read_to_end(&mut res)
                    .await
                    .expect("failed to read results");
                assert_eq!(res, b"\x04test");
            },
            async {
                srv.accept((), tx, BufReader::new(rx))
                    .await
                    .expect("failed to accept connection");
                let ((), (), rx, tx) = pin!(invocations)
                    .try_next()
                    .await
                    .expect("failed to accept invocation")
                    .expect("unexpected end of stream");
                assert!(rx.is_none());
                tx(("test",)).await.expect("failed to transmit response");
            }
        )
    };
    tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .context("invocation timed out")?;
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_oneshot_raw() -> anyhow::Result<()> {
    let (clt, mut srv) = tokio::io::duplex(1024);

    join!(
        async {
            let (a,) = Oneshot::from(clt)
                .invoke_values_blocking::<_, _, (String,)>((), "foo", "bar", (42,), &[[]; 0])
                .await
                .expect("failed to invoke `foo.bar`");
            assert_eq!(a, "test");
        },
        async {
            use tokio::io::AsyncWriteExt as _;
            use tokio_util::codec::Encoder as _;
            use wrpc_transport::Encode as _;

            let mut req = Vec::default();
            _ = srv.read_to_end(&mut req).await;
            eprintln!("request: `{:?}`", Bytes::from(req));

            let mut data = bytes::BytesMut::default();
            let deferred: Option<wrpc_transport::DeferredFn<wrpc_transport::frame::Outgoing>> =
                ("test",)
                    .encode(&mut Default::default(), &mut data)
                    .expect("failed to encode");
            assert!(deferred.is_none());

            let mut res = bytes::BytesMut::default();
            wrpc_transport::FrameEncoder
                .encode(
                    &wrpc_transport::Frame {
                        path: Arc::default(),
                        data: data.freeze(),
                    },
                    &mut res,
                )
                .expect("failed to encode response frame");
            eprintln!("response: `{res:?}`");

            srv.write_all(&res).await.expect("failed to write response");
        }
    );
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_oneshot_duplex() -> anyhow::Result<()> {
    let (clt, srv) = Oneshot::duplex(1024);
    let (rx, tx) = split(srv);
    assert_oneshot(clt, rx, tx).await
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_oneshot_uds() -> anyhow::Result<()> {
    let (clt, srv) = Oneshot::unix_pair()?;
    let (rx, tx) = wrpc_transport::unix::split(srv);
    assert_oneshot(clt, rx, tx).await
}
