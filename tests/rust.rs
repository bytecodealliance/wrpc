#![allow(clippy::disallowed_names, clippy::type_complexity)]
#![recursion_limit = "256"]

pub mod common;

use core::future::poll_fn;
use core::iter;
use core::pin::{Pin, pin};
use core::task::Poll;
use core::time::Duration;

use std::sync::{Arc, Mutex};

use anyhow::{Context as _, ensure};
use bytes::{Bytes, BytesMut};
use common::{assert_async, assert_sync};
use futures::future::BoxFuture;
use futures::stream::{BoxStream, FuturesUnordered};
use futures::{Stream, StreamExt as _, TryStreamExt as _, future, stream};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, split};
use tokio::task::yield_now;
use tokio::{join, spawn, try_join};
use tokio_util::codec::Encoder as _;
use tokio_util::io::StreamReader;
use tracing::instrument;
use wrpc_transport::{
    Connect as _, HeaderEncoder, HeaderRef, Oneshot, ResourceBorrow, ResourceOwn, accept,
    invoke_values, invoke_values_blocking, serve_values, tcp,
};

async fn serve_bindgen<F, E>(
    invocations: impl Stream<Item = Result<F, E>>,
    client: impl Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<()>
where
    F: Future<Output = anyhow::Result<()>>,
    E: core::error::Error + Send + Sync + 'static,
{
    let mut invocations = pin!(invocations);
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
async fn assert_bindgen_sync<T, C, I, O, E>(
    cx: T::Context,
    clt: &T,
    srv: impl Stream<Item = Result<(C, O, I), E>>,
) -> anyhow::Result<()>
where
    T: wrpc::Connect + Clone + 'static,
    T::Context: Clone + Send + Sync + 'static,
    C: Send + Sync + 'static,
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
    E: core::error::Error + Send + Sync + 'static,
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
    struct Handler<I: wrpc::Connect> {
        counts: Arc<Mutex<Vec<u32>>>,
        clt: I,
        cx: I::Context,
    }

    impl<I: wrpc::Connect> Handler<I> {
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

    impl<C: Send + Sync, I: wrpc::Connect + 'static> resources::HandlerFoo<C> for Handler<I> {
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

    impl<C: Send + Sync, I: wrpc::Connect + 'static> resources::HandlerCounter<C> for Handler<I> {
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

    impl<C: Send + Sync, I: wrpc::Connect + 'static> resources::Handler<C> for Handler<I> {
        async fn bar(&self, _cx: C, v: ResourceBorrow<Foo>) -> anyhow::Result<String> {
            ensure_foo(v)?;
            Ok("bar".into())
        }
    }

    impl<C: Send + Sync, I: wrpc::Connect + 'static> sync_server::exports::strange::Handler<C>
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
        I: wrpc::Connect + 'static,
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

    impl<C: Send + Sync, I: wrpc::Connect + 'static> sync::Handler<C> for Handler<I> {
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

    impl<C: Send + Sync, I: wrpc::Connect + 'static> sync_server::exports::foo::Handler<C>
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
    serve_bindgen(sync_server::serve(srv, handler), assert_sync(cx, clt)).await
}

#[instrument(skip_all, ret)]
async fn assert_bindgen_async<T, C, I, O, E>(
    cx: T::Context,
    clt: &T,
    srv: impl Stream<Item = Result<(C, O, I), E>>,
) -> anyhow::Result<()>
where
    T: wrpc::Connect,
    T::Context: Clone + Send + Sync,
    C: Send + Sync + 'static,
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
    E: core::error::Error + Send + Sync + 'static,
{
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

    serve_bindgen(bindings::serve(srv, Handler), assert_async(cx, clt)).await
}

#[instrument(skip_all, ret)]
async fn assert_dynamic<T, C, I, O, E>(
    cx: T::Context,
    clt: &T,
    srv: impl Stream<Item = Result<(C, O, I), E>>,
) -> anyhow::Result<()>
where
    T: wrpc::Connect,
    T::Context: Clone + Send + Sync + 'static,
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
    E: core::error::Error + Send + Sync + 'static,
{
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
        io: impl Future<Output = anyhow::Result<()>>,
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
            io,
        )?;
        Ok(())
    }

    async fn accept<C, I, O, E>(
        srv: &mut (impl Stream<Item = Result<(C, O, I), E>> + Unpin),
        func: &str,
    ) -> anyhow::Result<(O, I)>
    where
        I: AsyncRead + Unpin,
        E: core::error::Error + Send + Sync + 'static,
    {
        let res = srv.next().await.context("invocation stream ended")?;
        let (.., tx, mut rx) = res.context("failed to accept invocation")?;
        let header = wrpc_transport::accept(&mut rx)
            .await
            .context("failed to read invocation header")?;
        ensure!(header.instance == "test" && header.name == func);
        Ok((tx, rx))
    }

    let mut srv = pin!(srv);
    try_join!(
        async {
            for spawned in [false, true] {
                let (tx, rx) = accept(&mut srv, "reset").await?;
                let inv = serve_values::<(String,), (String,)>(tx, rx);
                if spawned {
                    spawn(async move { drop(inv) }).await?;
                } else {
                    drop(inv);
                }
            }
            for _ in 0..2 {
                let (tx, rx) = accept(&mut srv, "sync").await?;
                let (params, tx) = serve_values::<SyncValues, SyncValues>(tx, rx);
                let (params, io) = params.await?;
                assert_eq!(params, sync_values());
                tx(sync_values()).await?;
                io.await?;
            }
            for _ in 0..2 {
                let (tx, rx) = accept(&mut srv, "async").await?;
                let (params, tx) = serve_values::<AsyncValues, AsyncValues>(tx, rx);
                let (params, io) = params.await?;
                try_join!(assert_async_values(params, io), tx(async_values()))?;
            }
            Ok(())
        },
        async {
            for _ in 0..2 {
                let (tx, rx) = clt.connect(cx.clone()).await?;
                let res =
                    invoke_values_blocking::<_, (String,)>(tx, rx, "test", "reset", ("arg",)).await;
                res.expect_err("`test.reset` should have failed");
            }

            let mut buf = BytesMut::default();
            wrpc_transport::pack(sync_values(), &mut buf).await?;
            let unpacked = wrpc_transport::unpack(&mut buf)?;
            for params in [sync_values(), unpacked] {
                let (tx, rx) = clt.connect(cx.clone()).await?;
                let res: SyncValues =
                    invoke_values_blocking(tx, rx, "test", "sync", params).await?;
                assert_eq!(res, sync_values());
            }

            wrpc_transport::pack(async_values(), &mut buf).await?;
            let unpacked = wrpc_transport::unpack(&mut buf)?;
            for params in [async_values(), unpacked] {
                let (tx, rx) = clt.connect(cx.clone()).await?;
                let (res, io) = invoke_values(tx, rx, "test", "async", params).await?;
                assert_async_values(res, io).await?;
            }
            anyhow::Ok(())
        },
    )?;
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_tcp_bindgen_sync() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |clt, srv| assert_bindgen_sync((), &clt, tcp::incoming(srv)).await)
        .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_tcp_bindgen_async() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |clt, srv| assert_bindgen_async((), &clt, tcp::incoming(srv)).await)
        .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_tcp_dynamic() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |clt, srv| assert_dynamic((), &clt, tcp::incoming(srv)).await).await
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_uds_bindgen_sync() -> anyhow::Result<()> {
    wrpc_test::serve_unix(async |clt, srv| {
        assert_bindgen_sync((), &clt, wrpc_transport::unix::incoming(srv)).await
    })
    .await
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_uds_bindgen_async() -> anyhow::Result<()> {
    wrpc_test::serve_unix(async |clt, srv| {
        assert_bindgen_async((), &clt, wrpc_transport::unix::incoming(srv)).await
    })
    .await
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_uds_dynamic() -> anyhow::Result<()> {
    wrpc_test::serve_unix(async |clt, srv| {
        assert_dynamic((), &clt, wrpc_transport::unix::incoming(srv)).await
    })
    .await
}

#[cfg(feature = "quic")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_quic_bindgen_sync() -> anyhow::Result<()> {
    wrpc_test::serve_quic(async |clt, srv| assert_bindgen_sync((), &clt, srv.incoming()).await)
        .await
}

#[cfg(feature = "quic")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_quic_bindgen_async() -> anyhow::Result<()> {
    wrpc_test::serve_quic(async |clt, srv| assert_bindgen_async((), &clt, srv.incoming()).await)
        .await
}

#[cfg(feature = "quic")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_quic_dynamic() -> anyhow::Result<()> {
    wrpc_test::serve_quic(async |clt, srv| assert_dynamic((), &clt, srv.incoming()).await).await
}

#[cfg(feature = "webtransport")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_webtransport_bindgen_sync() -> anyhow::Result<()> {
    wrpc_test::serve_webtransport(async |clt, srv| {
        assert_bindgen_sync((), &clt, srv.incoming()).await
    })
    .await
}

#[cfg(feature = "webtransport")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_webtransport_bindgen_async() -> anyhow::Result<()> {
    wrpc_test::serve_webtransport(async |clt, srv| {
        assert_bindgen_async((), &clt, srv.incoming()).await
    })
    .await
}

#[cfg(feature = "webtransport")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_webtransport_dynamic() -> anyhow::Result<()> {
    wrpc_test::serve_webtransport(async |clt, srv| assert_dynamic((), &clt, srv.incoming()).await)
        .await
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
async fn assert_oneshot<T, R, W>(clt: T, mut rx: R, tx: W) -> anyhow::Result<()>
where
    T: wrpc::Connect<Context = ()> + 'static,
    R: tokio::io::AsyncRead + Send + Sync + Unpin + 'static,
    W: tokio::io::AsyncWrite + Send + Sync + Unpin + 'static,
{
    join!(
        async {
            let (tx, rx) = clt.connect(()).await.expect("failed to connect");
            let (a,) = invoke_values_blocking::<_, (String,)>(tx, rx, "foo", "bar", (42,))
                .await
                .expect("failed to invoke `foo.bar`");
            assert_eq!(a, "test");
        },
        async {
            let header = accept(&mut rx)
                .await
                .expect("failed to read invocation header");
            assert_eq!(header.instance, "foo");
            assert_eq!(header.name, "bar");
            let (params, tx) = serve_values::<(u32,), (&str,)>(tx, rx);
            let (params, io) = params.await.expect("failed to receive parameters");
            assert_eq!(params, (42,));
            tx(("test",)).await.expect("failed to transmit response");
            io.await.expect("invocation failed");
        }
    );
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_oneshot_no_params() -> anyhow::Result<()> {
    let (clt, srv) = Oneshot::duplex(1024);
    let (mut rx, tx) = split(srv);
    let fut = async {
        join!(
            async {
                let (mut tx, mut rx) = clt.connect(()).await.expect("failed to connect");
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
                tx.write_all(&buf)
                    .await
                    .expect("failed to invoke `foo.bar`");
                let mut res = Vec::default();
                rx.read_to_end(&mut res)
                    .await
                    .expect("failed to read results");
                assert_eq!(res, b"\0\x05\x04test\0\0");
                drop(tx);
            },
            async {
                let header = accept(&mut rx)
                    .await
                    .expect("failed to read invocation header");
                assert_eq!(header.instance, "foo");
                assert_eq!(header.name, "bar");
                let (params, tx) = serve_values::<(), (&str,)>(tx, rx);
                let ((), io) = params.await.expect("failed to receive parameters");
                tx(("test",)).await.expect("failed to transmit response");
                io.await.expect("invocation failed");
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
            let (tx, rx) = Oneshot::from(clt)
                .connect(())
                .await
                .expect("failed to connect");
            let (a,) = invoke_values_blocking::<_, (String,)>(tx, rx, "foo", "bar", (42,))
                .await
                .expect("failed to invoke `foo.bar`");
            assert_eq!(a, "test");
        },
        async {
            use wrpc_transport::Encode as _;

            let mut req = Vec::default();
            _ = srv.read_to_end(&mut req).await;
            eprintln!("request: `{:?}`", Bytes::from(req));

            let mut data = bytes::BytesMut::default();
            let deferred: Option<wrpc_transport::DeferredFn<wrpc_transport::Packer>> = ("test",)
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

    let (clt, srv) = Oneshot::duplex(1024);
    let (mut rx, tx) = split(srv);
    join!(
        async {
            bindings::wrpc_test::raw_strings::strings::bar(&clt, ())
                .await
                .expect_err("invalid UTF-8 must be rejected");
        },
        async {
            let header = accept(&mut rx)
                .await
                .expect("failed to read invocation header");
            assert_eq!(header.instance, "wrpc-test:raw-strings/strings");
            assert_eq!(header.name, "bar");
            let (params, tx) = serve_values::<(), (Bytes,)>(tx, rx);
            let ((), io) = params.await.expect("failed to receive parameters");
            tx((Bytes::from_static(b"\xff"),))
                .await
                .expect("failed to transmit response");
            io.await.expect("invocation failed");
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

    impl<C: Send + Sync> bindings::exports::wrpc_test::raw_strings::strings::Handler<C> for Handler {
        async fn bar(&self, _cx: C) -> anyhow::Result<Vec<u8>> {
            Ok(b"\xff".to_vec())
        }

        async fn baz(&self, _cx: C) -> anyhow::Result<Rec> {
            Ok(Rec {
                s: b"\xff".to_vec(),
                l: Vec::default(),
                o: None,
                t: (Vec::default(), 0),
                r: Ok(Vec::default()),
            })
        }

        async fn qux(&self, _cx: C) -> anyhow::Result<Var> {
            Ok(Var::S(b"\xff".to_vec()))
        }
    }

    for func in ["bar", "baz", "qux"] {
        let (clt, conn) = Oneshot::duplex(1024);
        let (mut rx, tx) = split(conn);
        join!(
            async {
                let (tx, rx) = clt.connect(()).await.expect("failed to connect");
                invoke_values_blocking::<_, (String,)>(
                    tx,
                    rx,
                    "wrpc-test:raw-strings/strings",
                    func,
                    (),
                )
                .await
                .expect_err("invalid UTF-8 must not be transmitted");
            },
            async {
                let header = accept(&mut rx)
                    .await
                    .expect("failed to read invocation header");
                let Ok(fut) =
                    bindings::handle(&Handler, (), &header.instance, &header.name, tx, rx)
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
    let (rx, tx) = srv.into_split();
    assert_oneshot(clt, rx, tx).await
}
