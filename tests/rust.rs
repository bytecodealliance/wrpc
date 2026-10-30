#![allow(clippy::disallowed_names, clippy::type_complexity)]

pub mod common;

use core::future::poll_fn;
use core::iter;
use core::net::SocketAddr;
use core::pin::{Pin, pin};
use core::task::Poll;
use core::time::Duration;

use std::sync::{Arc, Mutex};

use anyhow::{Context as _, bail, ensure};
use bytes::{Bytes, BytesMut};
use common::{assert_async, assert_sync};
use futures::future::{BoxFuture, OptionFuture};
use futures::stream::{BoxStream, FuturesUnordered};
use futures::{Stream, StreamExt as _, TryStreamExt as _, future, stream};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, BufReader, split};
use tokio::task::yield_now;
use tokio::{join, try_join};
use tokio_util::io::StreamReader;
use tracing::instrument;
use wrpc_transport::frame::{Oneshot, tcp};
use wrpc_transport::{InvokeExt as _, ResourceBorrow, ResourceOwn, ServeExt as _, TupleDecode};

async fn with_tcp(
    f: impl AsyncFnOnce(
        &tcp::Client<SocketAddr>,
        &wrpc_transport::frame::Server<(), BufReader<tcp::ReadHalf>, tcp::WriteHalf>,
    ) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let lis = wrpc_test::tcp_bind().await?;
    let addr = lis.local_addr()?;
    let clt = tcp::Client::from(addr);
    let srv = wrpc_transport::frame::Server::default();
    wrpc_test::accept_until(f(&clt, &srv), async || {
        wrpc_test::accept_tcp(&srv, &lis).await
    })
    .await
}

#[cfg(unix)]
async fn with_uds(
    f: impl AsyncFnOnce(
        &wrpc_transport::frame::unix::Client<std::path::PathBuf>,
        &wrpc_transport::frame::Server<
            (),
            BufReader<wrpc_transport::frame::unix::ReadHalf>,
            wrpc_transport::frame::unix::WriteHalf,
        >,
    ) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("sock");
    let lis = tokio::net::UnixListener::bind(&path)?;
    let srv = wrpc_transport::frame::Server::default();
    let clt = wrpc_transport::frame::unix::Client::from(path);
    wrpc_test::accept_until(f(&clt, &srv), async || {
        wrpc_test::accept_unix(&srv, &lis).await
    })
    .await
}

#[cfg(feature = "quic")]
async fn with_quic(
    f: impl AsyncFnOnce(&wrpc_quic::Client, &wrpc_quic::Server) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    wrpc_test::with_quic(|cc, sc| async move {
        let srv = wrpc_quic::Server::default();
        let clt = wrpc_quic::Client::from(cc);
        wrpc_test::accept_until(f(&clt, &srv), async || {
            wrpc_test::accept_quic(&srv, &sc).await
        })
        .await
    })
    .await
}

#[cfg(feature = "webtransport")]
async fn with_webtransport(
    f: impl AsyncFnOnce(&wrpc_webtransport::Client, &wrpc_webtransport::Server) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    wrpc_test::with_webtransport(|cc, sc| async move {
        let srv = wrpc_webtransport::Server::default();
        let clt = wrpc_webtransport::Client::from(cc);
        wrpc_test::accept_until(f(&clt, &srv), async || {
            wrpc_test::accept_webtransport(&srv, &sc).await
        })
        .await
    })
    .await
}

#[cfg(feature = "websockets")]
async fn with_websockets(
    f: impl AsyncFnOnce(
        &wrpc_websockets::Client<'static>,
        &wrpc_transport::frame::Server<
            (),
            StreamReader<wrpc_websockets::Incoming<tokio::net::TcpStream>, Bytes>,
            tokio_util::io::SinkWriter<wrpc_websockets::Outgoing<tokio::net::TcpStream>>,
        >,
    ) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    wrpc_test::with_websockets(|clt, sb, lis| async move {
        let srv = wrpc_transport::frame::Server::default();
        let clt = wrpc_websockets::Client::from(clt);
        wrpc_test::accept_until(f(&clt, &srv), async || {
            wrpc_test::accept_websocket(&srv, &lis, &sb, wrpc_websockets::split).await
        })
        .await
    })
    .await
}

#[cfg(feature = "http")]
async fn with_http(
    f: impl AsyncFnOnce(
        http::request::Parts,
        &wrpc::transport::http::Client<
            hyper::client::conn::http2::SendRequest<wrpc_http::OutgoingBody>,
        >,
        &wrpc_transport::frame::Server<
            http::request::Parts,
            wrpc_http::DynIncomingBodyDataReader,
            wrpc_http::OutgoingBodyDataWriter,
        >,
    ) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    use wrpc::transport::http::{Client, Server};

    let srv = Arc::new(wrpc_transport::frame::Server::default());
    wrpc_test::with_http2(Server::from(Arc::clone(&srv)), |parts, sender| async move {
        f(parts, &Client::from(sender), &srv).await
    })
    .await
}

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
async fn assert_bindgen_sync<C: Clone + Send + Sync>(
    cx: C,
    clt: &impl wrpc::Invoke<Context = C>,
    srv: &impl wrpc::Serve,
) -> anyhow::Result<()> {
    mod resources_client {
        wit_bindgen_wrpc::generate!({
            world: "resources-client",
            path: "tests/wit",
        });
    }

    mod resources_server {
        wit_bindgen_wrpc::generate!({
            world: "resources-server",
            path: "tests/wit",
        });
    }

    mod root_client {
        wit_bindgen_wrpc::generate!({
            world: "root-client",
            path: "tests/wit",
        });
    }

    mod root_server {
        wit_bindgen_wrpc::generate!({
            world: "root-server",
            path: "tests/wit",
        });
    }

    mod sync_server {
        wit_bindgen_wrpc::generate!({
            world: "sync-server",
            path: "tests/wit",
        });
    }

    use resources_server::exports::wrpc_test::integration::resources::{self, Counter, Foo};
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

    #[derive(Clone, Default)]
    struct Handler(Arc<Mutex<Vec<u32>>>);

    impl Handler {
        fn push(&self, count: u32) -> ResourceOwn<Counter> {
            let mut counts = self.0.lock().unwrap();
            counts.push(count);
            ResourceOwn::from(Bytes::copy_from_slice(&(counts.len() - 1).to_ne_bytes()))
        }

        fn with<T>(
            &self,
            handle: ResourceBorrow<Counter>,
            f: impl FnOnce(&mut u32) -> T,
        ) -> anyhow::Result<T> {
            let handle = Bytes::from(handle);
            let i = handle.as_ref().try_into()?;
            let i = usize::from_ne_bytes(i);
            let mut counts = self.0.lock().unwrap();
            let count = counts.get_mut(i).context("unknown counter")?;
            Ok(f(count))
        }
    }

    impl<C: Send + Sync> resources::HandlerFoo<C> for Handler {
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

    impl<C: Send + Sync> resources::HandlerCounter<C> for Handler {
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
            self.with(self_, |count| *count += num)
        }

        async fn sum(
            &self,
            _cx: C,
            a: ResourceBorrow<Counter>,
            b: ResourceBorrow<Counter>,
        ) -> anyhow::Result<u32> {
            let a = self.with(a, |count| *count)?;
            let b = self.with(b, |count| *count)?;
            Ok(a + b)
        }
    }

    impl<C: Send + Sync> resources::Handler<C> for Handler {
        async fn bar(&self, _cx: C, v: ResourceBorrow<Foo>) -> anyhow::Result<String> {
            ensure_foo(v)?;
            Ok("bar".into())
        }
    }

    impl<C: Send + Sync> resources_server::exports::strange::Handler<C> for Handler {
        async fn bar(&self, _cx: C, v: ResourceBorrow<Foo>) -> anyhow::Result<u64> {
            ensure_foo(v)?;
            Ok(42)
        }
    }

    impl<C: Send + Sync> root_server::Handler<C> for Handler {
        async fn ping(&self, _cx: C) -> anyhow::Result<String> {
            Ok("pong".into())
        }
    }

    impl<C: Send + Sync> sync::Handler<C> for Handler {
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

    impl<C: Send + Sync> sync_server::exports::foo::Handler<C> for Handler {
        async fn f(&self, _cx: C, x: String) -> anyhow::Result<u32> {
            assert_eq!(x, "f");
            Ok(42)
        }

        async fn foo(&self, _cx: C, x: String) -> anyhow::Result<()> {
            assert_eq!(x, "foo");
            Ok(())
        }
    }

    let mut invocations = root_server::serve(srv, Handler::default()).await?;
    assert!(
        invocations
            .iter()
            .any(|(instance, name, ..)| instance.is_empty() && *name == "ping")
    );
    let resources = resources_server::serve(srv, Handler::default()).await?;
    invocations.extend(resources);
    let sync = sync_server::serve(srv, Handler::default()).await?;
    invocations.extend(sync);
    serve_bindgen(invocations, async {
        use resources_client::strange;
        use resources_client::wrpc_test::integration::resources::{self, Counter, Foo};

        assert_sync(cx.clone(), clt).await?;
        let pong = root_client::ping(clt, cx.clone()).await?;
        assert_eq!(pong, "pong");

        let foo = Foo::new(clt, cx.clone()).await?;
        let v = Foo::bar(clt, cx.clone(), &foo.as_borrow()).await?;
        assert_eq!(v, "bar");
        let v = resources::bar(clt, cx.clone(), &foo.as_borrow()).await?;
        assert_eq!(v, "bar");
        let v = strange::bar(clt, cx.clone(), &foo.as_borrow()).await?;
        assert_eq!(v, 42);
        let v = Foo::foo(clt, cx.clone(), &foo).await?;
        assert_eq!(v, "foo");

        let a = Counter::new(clt, cx.clone(), 1).await?;
        Counter::increment_by(clt, cx.clone(), &a.as_borrow(), 2).await?;
        let n = Counter::get_count(clt, cx.clone(), &a.as_borrow()).await?;
        assert_eq!(n, 3);
        let b = Counter::clone_counter(clt, cx.clone(), &a.as_borrow()).await?;
        Counter::increment_by(clt, cx.clone(), &b.as_borrow(), 1).await?;
        let n = Counter::sum(clt, cx, &a.as_borrow(), &b.as_borrow()).await?;
        assert_eq!(n, 7);
        Ok(())
    })
    .await
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

    async fn assert_async_values((a, b, mut c, d, e, f): AsyncValues) {
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
        assert_eq!(c.unwrap(), "test");
        assert_eq!(d, [1, 2, 3]);
        assert_eq!(e.concat(), b"abcd".repeat(1 << 16));
        assert_eq!(f, ["foo", "bar"]);
    }

    let paths: Arc<[Box<[Option<usize>]>]> = Arc::from([
        Box::from([Some(0)]),
        Box::from([Some(1)]),
        Box::from([Some(2)]),
        Box::from([Some(3)]),
        Box::from([Some(4)]),
        Box::from([Some(5), None]),
    ]);
    let sync = srv
        .serve_values::<SyncValues, SyncValues>("test", "sync", Arc::default())
        .await?;
    let async_ = srv
        .serve_values::<AsyncValues, AsyncValues>("test", "async", Arc::clone(&paths))
        .await?;
    let (mut sync, mut async_) = (pin!(sync), pin!(async_));
    try_join!(
        async {
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
                let ((), io) = join!(assert_async_values(params), OptionFuture::from(io));
                io.transpose()?;
                tx(async_values()).await?;
            }
            Ok(())
        },
        async {
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
                let ((), io) = join!(assert_async_values(res), OptionFuture::from(io));
                io.transpose()?;
            }
            anyhow::Ok(())
        },
    )?;
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_tcp_bindgen_sync() -> anyhow::Result<()> {
    with_tcp(async |clt, srv| assert_bindgen_sync((), clt, srv).await).await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_tcp_bindgen_async() -> anyhow::Result<()> {
    with_tcp(async |clt, srv| assert_bindgen_async((), clt, srv).await).await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_tcp_dynamic() -> anyhow::Result<()> {
    with_tcp(async |clt, srv| assert_dynamic((), clt, srv).await).await
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_uds_bindgen_sync() -> anyhow::Result<()> {
    with_uds(async |clt, srv| assert_bindgen_sync((), clt, srv).await).await
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_uds_bindgen_async() -> anyhow::Result<()> {
    with_uds(async |clt, srv| assert_bindgen_async((), clt, srv).await).await
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_uds_dynamic() -> anyhow::Result<()> {
    with_uds(async |clt, srv| assert_dynamic((), clt, srv).await).await
}

#[cfg(feature = "quic")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_quic_bindgen_sync() -> anyhow::Result<()> {
    with_quic(async |clt, srv| assert_bindgen_sync((), clt, srv).await).await
}

#[cfg(feature = "quic")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_quic_bindgen_async() -> anyhow::Result<()> {
    with_quic(async |clt, srv| assert_bindgen_async((), clt, srv).await).await
}

#[cfg(feature = "quic")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_quic_dynamic() -> anyhow::Result<()> {
    with_quic(async |clt, srv| assert_dynamic((), clt, srv).await).await
}

#[cfg(feature = "webtransport")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_webtransport_bindgen_sync() -> anyhow::Result<()> {
    with_webtransport(async |clt, srv| assert_bindgen_sync((), clt, srv).await).await
}

#[cfg(feature = "webtransport")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_webtransport_bindgen_async() -> anyhow::Result<()> {
    with_webtransport(async |clt, srv| assert_bindgen_async((), clt, srv).await).await
}

#[cfg(feature = "webtransport")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_webtransport_dynamic() -> anyhow::Result<()> {
    with_webtransport(async |clt, srv| assert_dynamic((), clt, srv).await).await
}

#[cfg(feature = "websockets")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_websockets_bindgen_sync() -> anyhow::Result<()> {
    with_websockets(async |clt, srv| assert_bindgen_sync((), clt, srv).await).await
}

#[cfg(feature = "websockets")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_websockets_bindgen_async() -> anyhow::Result<()> {
    with_websockets(async |clt, srv| assert_bindgen_async((), clt, srv).await).await
}

#[cfg(feature = "websockets")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_websockets_dynamic() -> anyhow::Result<()> {
    with_websockets(async |clt, srv| assert_dynamic((), clt, srv).await).await
}

#[cfg(feature = "http")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_http_bindgen_sync() -> anyhow::Result<()> {
    with_http(async |cx, clt, srv| assert_bindgen_sync(cx, clt, srv).await).await
}

#[cfg(feature = "http")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_http_bindgen_async() -> anyhow::Result<()> {
    with_http(async |cx, clt, srv| assert_bindgen_async(cx, clt, srv).await).await
}

#[cfg(feature = "http")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_http_dynamic() -> anyhow::Result<()> {
    with_http(async |cx, clt, srv| assert_dynamic(cx, clt, srv).await).await
}

async fn assert_oneshot<P, R, W>(
    clt: impl wrpc::Invoke<Context = ()>,
    rx: R,
    tx: W,
    params: &'static [u8],
    expected: P,
) -> anyhow::Result<()>
where
    P: TupleDecode + PartialEq + core::fmt::Debug + Send + 'static,
    <P::Decoder as tokio_util::codec::Decoder>::Error: core::error::Error + Send + Sync + 'static,
    R: AsyncRead + Send + Sync + Unpin + 'static,
    W: AsyncWrite + Send + Sync + Unpin + 'static,
{
    let srv = wrpc_transport::frame::Server::default();
    let invocations = srv
        .serve_values::<P, (&str,)>("foo", "bar", Arc::default())
        .await?;
    let fut = async {
        try_join!(
            async {
                let (_tx, mut rx) = clt
                    .invoke((), "foo", "bar", Bytes::from_static(params), &[[]; 0])
                    .await?;
                let mut res = Vec::default();
                rx.read_to_end(&mut res).await?;
                assert_eq!(res, b"\x04test");
                anyhow::Ok(())
            },
            async {
                srv.accept((), tx, BufReader::new(rx))
                    .await
                    .expect("failed to accept invocation");
                let Some(((), params, rx, tx)) = pin!(invocations).try_next().await? else {
                    bail!("invocation missing");
                };
                assert_eq!(params, expected);
                assert!(rx.is_none());
                tx(("test",)).await
            }
        )
    };
    tokio::time::timeout(Duration::from_secs(10), fut).await??;
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_oneshot_duplex() -> anyhow::Result<()> {
    let (clt, srv) = Oneshot::duplex(1024);
    let (rx, tx) = split(srv);
    assert_oneshot(clt, rx, tx, b"", ()).await
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_oneshot_uds() -> anyhow::Result<()> {
    let (clt, srv) = Oneshot::unix_pair()?;
    let (rx, tx) = wrpc_transport::unix::split(srv);
    assert_oneshot(clt, rx, tx, b"\x2a", (42_u32,)).await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_oneshot_raw() -> anyhow::Result<()> {
    let (clt, mut srv) = Oneshot::duplex(1024);
    let (res, ()) = join!(
        clt.invoke_values_blocking::<_, _, (String,)>((), "foo", "bar", (42_u32,), &[[]; 0]),
        async {
            let mut req = Vec::default();
            srv.read_to_end(&mut req).await.unwrap();
            assert_eq!(
                req,
                concat!("\0", "\x03foo", "\x03bar", concat!("\0", "\x01\x2a")).as_bytes()
            );
            srv.write_all(concat!("\0", "\x05\x04test").as_bytes())
                .await
                .unwrap();
        }
    );
    let res = res?;
    assert_eq!(res, ("test".to_string(),));
    Ok(())
}
