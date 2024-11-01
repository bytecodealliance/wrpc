#![allow(clippy::type_complexity)]
#![allow(unused)]

mod common;

use core::str;
use core::time::Duration;

use std::sync::Arc;
use std::thread;

use anyhow::Context;
use bytes::Bytes;
use common::assert_async;
use futures::{stream, FutureExt as _, Stream, StreamExt as _, TryStreamExt as _};
use tokio::sync::{oneshot, RwLock};
use tokio::time::sleep;
use tokio::{join, select, spawn, try_join};
use tracing::{info, info_span, instrument, Instrument, Span};
use wrpc_transport::{
    Invoke as _, InvokeExt as _, ResourceBorrow, ResourceOwn, Serve as _, ServeExt as _,
};

#[instrument(skip_all, ret)]
async fn assert_bindgen_async<C, I, S>(clt: Arc<I>, srv: Arc<S>) -> anyhow::Result<()>
where
    C: Send + Sync + Default,
    I: wrpc::Invoke<Context = C> + 'static,
    S: wrpc::Serve<Context = C> + Send + 'static,
{
    let span = Span::current();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let shutdown_rx = async move { shutdown_rx.await.expect("shutdown sender dropped") }.shared();
    try_join!(
        async {
            wit_bindgen_wrpc::generate!({
                world: "async-server",
                path: "tests/wit",
            });

            #[derive(Clone, Default)]
            struct Component {}

            impl<C: Send + Sync> exports::wrpc_test::integration::async_::Handler<C> for Component {
                async fn with_streams(
                    &self,
                    _cx: C,
                ) -> anyhow::Result<(
                    Pin<Box<dyn Stream<Item = Bytes> + Send>>,
                    Pin<Box<dyn Stream<Item = Vec<Vec<String>>> + Send>>,
                )> {
                    Ok((
                        Box::pin(stream::iter([Bytes::from("test")])),
                        Box::pin(stream::iter([
                            vec![vec!["foo".to_string()]],
                            vec![vec!["bar".to_string(), "baz".to_string()]],
                        ])),
                    ))
                }

                async fn with_future(
                    &self,
                    _cx: C,
                    x: exports::wrpc_test::integration::async_::Something,
                    s: Pin<Box<dyn Stream<Item = Bytes> + Send>>,
                ) -> anyhow::Result<
                    Pin<Box<dyn Future<Output = Pin<Box<dyn Stream<Item = Bytes> + Send>>> + Send>>,
                > {
                    assert_eq!(x.foo, "bar");
                    Ok(Box::pin(async { s }))
                }

                async fn identity_nested_async(
                    &self,
                    _cx: C,
                    v: Pin<
                        Box<
                            (dyn Future<
                                Output = Pin<
                                    Box<
                                        (dyn Future<
                                            Output = Pin<
                                                Box<
                                                    (dyn Future<
                                                        Output = Pin<
                                                            Box<
                                                                (dyn Stream<Item = Vec<String>>
                                                                     + Send
                                                                     + 'static),
                                                            >,
                                                        >,
                                                    > + Send
                                                         + 'static),
                                                >,
                                            >,
                                        > + Send
                                             + 'static),
                                    >,
                                >,
                            > + Send
                                 + 'static),
                        >,
                    >,
                ) -> anyhow::Result<
                    Pin<
                        Box<
                            (dyn Future<
                                Output = Pin<
                                    Box<
                                        (dyn Future<
                                            Output = Pin<
                                                Box<
                                                    (dyn Future<
                                                        Output = Pin<
                                                            Box<
                                                                (dyn Stream<Item = Vec<String>>
                                                                     + Send
                                                                     + 'static),
                                                            >,
                                                        >,
                                                    > + Send
                                                         + 'static),
                                                >,
                                            >,
                                        > + Send
                                             + 'static),
                                    >,
                                >,
                            > + Send
                                 + 'static),
                        >,
                    >,
                > {
                    Ok(v)
                }
            }

            let srv = Arc::clone(&srv);
            let shutdown_rx = shutdown_rx.clone();
            tokio::spawn(async move {
                let mut invocations = serve(srv.as_ref(), Component::default())
                    .await
                    .context("failed to serve `wrpc-test:integration/async`")?;
                let mut invocations = stream::select_all(invocations.into_iter().map(
                    |(instance, name, invocations)| {
                        invocations.map(move |res| (instance, name, res))
                    },
                ));
                loop {
                    let shutdown_rx = shutdown_rx.clone();
                    select! {
                        Some((instance, name, invocation)) = invocations.next() => {
                            info!(instance, name, "serving invocation");
                            invocation
                                .unwrap_or_else(|err| panic!("failed to accept `{instance}#{name}` invocation: {err:?}"))
                                .await
                                .expect("failed to serve invocation");
                        }
                        () = shutdown_rx => {
                            info!("shutting down");
                            return anyhow::Ok(())
                        }
                    }
                }
            }.instrument(span.clone()))
            .await?
        },
        async {
            wit_bindgen_wrpc::generate!({
                world: "async-client",
                path: "tests/wit",
            });

            // TODO: Remove the need for this
            sleep(Duration::from_secs(1)).await;

            assert_async(clt.as_ref()).await?;

            shutdown_tx.send(()).expect("failed to send shutdown");
            Ok(())
        },
    )?;
    Ok(())
}

#[instrument(skip_all, ret)]
async fn assert_bindgen_sync<C, I, S>(clt: Arc<I>, srv: Arc<S>) -> anyhow::Result<()>
where
    C: Send + Sync + Default,
    I: wrpc::Invoke<Context = C> + 'static,
    S: wrpc::Serve<Context = C> + Send + 'static,
{
    let span = Span::current();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let shutdown_rx = async move { shutdown_rx.await.expect("shutdown sender dropped") }.shared();
    try_join!(
        async {
            mod bindings {
                wit_bindgen_wrpc::generate!({
                    inline: "
                       package wrpc-test:integration;

                       interface shared {
                           flags abc {
                               a,
                               b,
                               c,
                           }

                           fallible: func() -> result<bool, string>;
                           numbers: func() -> tuple<u8, u16, u32, u64, s8, s16, s32, s64, f32, f64>;
                           with-flags: func() -> abc;

                           resource counter {
                               constructor(initial: u32);
                               clone-counter: func() -> counter;

                               get-count: func() -> u32;
                               increment-by: func(num: u32);

                               sum: static func(a: borrow<counter>, b: borrow<counter>) -> u32;
                           }
                       }

                       world test {
                           export shared;

                           export f: func(x: string) -> u32;
                           export foo: interface {
                               foo: func(x: string);
                           }
                       }"
                });
            }

            use bindings::exports;
            use bindings::exports::wrpc_test::integration::shared::Counter;

            #[derive(Clone, Default)]
            struct Component {
                inner: Arc<RwLock<Option<String>>>,
                counts: Arc<RwLock<Vec<u32>>>,
            }

            impl<C: Send + Sync> exports::wrpc_test::integration::shared::HandlerCounter<C> for Component {
                async fn new(&self, _cx: C, initial: u32) -> anyhow::Result<ResourceOwn<Counter>> {
                    let mut counts = self.counts.write().await;
                    counts.push(initial);

                    let index = counts.len() - 1;
                    let handle_blob: Bytes = index.to_ne_bytes().to_vec().into();
                    let handle = ResourceOwn::from(handle_blob);

                    Ok(handle)
                }

                async fn clone_counter(
                    &self,
                    _cx: C,
                    self_: ResourceBorrow<Counter>,
                ) -> anyhow::Result<ResourceOwn<Counter>> {
                    let handle_blob: Bytes = self_.into();

                    let index_bytes: [u8; 8] = handle_blob[0..8]
                        .try_into()
                        .context("failed to decode counter resource hanlde")?;
                    let index = usize::from_ne_bytes(index_bytes);

                    let mut counts = self.counts.write().await;
                    let count = *counts
                        .get(index)
                        .context("counter resource entry not found")?;

                    counts.push(count);
                    let index = counts.len() - 1;
                    let handle_blob: Bytes = index.to_ne_bytes().to_vec().into();
                    let handle = ResourceOwn::from(handle_blob);

                    Ok(handle)
                }

                async fn get_count(
                    &self,
                    _cx: C,
                    self_: ResourceBorrow<Counter>,
                ) -> anyhow::Result<u32> {
                    let handle_blob: Bytes = self_.into();

                    let index_bytes: [u8; 8] = handle_blob[0..8]
                        .try_into()
                        .context("failed to decode counter resource hanlde")?;
                    let index = usize::from_ne_bytes(index_bytes);

                    let counts = self.counts.read().await;
                    let count = counts
                        .get(index)
                        .context("counter resource entry not found")?;

                    Ok(*count)
                }

                async fn increment_by(
                    &self,
                    _cx: C,
                    self_: ResourceBorrow<Counter>,
                    num: u32,
                ) -> anyhow::Result<()> {
                    let handle_blob: Bytes = self_.into();

                    let index_bytes: [u8; 8] = handle_blob[0..8]
                        .try_into()
                        .context("failed to decode counter resource handle")?;
                    let index = usize::from_ne_bytes(index_bytes);

                    let mut counts = self.counts.write().await;
                    let count = counts
                        .get_mut(index)
                        .context("counter resource entry not found")?;

                    *count += num;

                    Ok(())
                }

                async fn sum(
                    &self,
                    _cx: C,
                    a: ResourceBorrow<Counter>,
                    b: ResourceBorrow<Counter>,
                ) -> anyhow::Result<u32> {
                    let a_handle_blob: Bytes = a.into();
                    let b_handle_blob: Bytes = b.into();

                    let a_index_bytes: [u8; 8] = a_handle_blob[0..8]
                        .try_into()
                        .context("failed to decode counter resource handle")?;
                    let b_index_bytes: [u8; 8] = b_handle_blob[0..8]
                        .try_into()
                        .context("failed to decode counter resource handle")?;

                    let a_index = usize::from_ne_bytes(a_index_bytes);
                    let b_index = usize::from_ne_bytes(b_index_bytes);

                    let counts = self.counts.write().await;
                    let a_count = counts
                        .get(a_index)
                        .context("counter resource entry not found")?;
                    let b_count = counts
                        .get(b_index)
                        .context("counter resource entry not found")?;

                    Ok(*a_count + *b_count)
                }
            }

            impl<C: Send + Sync> bindings::Handler<C> for Component {
                async fn f(&self, _cx: C, x: String) -> anyhow::Result<u32> {
                    let stored = self.inner.read().await.as_ref().unwrap().to_string();
                    assert_eq!(stored, x);
                    Ok(42)
                }
            }

            impl<C: Send + Sync> exports::wrpc_test::integration::shared::Handler<C> for Component {
                async fn fallible(&self, _cx: C) -> anyhow::Result<Result<bool, String>> {
                    Ok(Ok(true))
                }

                async fn numbers(
                    &self,
                    _cx: C,
                ) -> anyhow::Result<(u8, u16, u32, u64, i8, i16, i32, i64, f32, f64)>
                {
                    Ok((
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
                    ))
                }

                async fn with_flags(
                    &self,
                    _cx: C,
                ) -> anyhow::Result<exports::wrpc_test::integration::shared::Abc> {
                    use exports::wrpc_test::integration::shared::Abc;
                    Ok(Abc::A | Abc::C)
                }
            }

            impl<C: Send + Sync> exports::foo::Handler<C> for Component {
                async fn foo(&self, _cx: C, x: String) -> anyhow::Result<()> {
                    let old = self.inner.write().await.replace(x);
                    assert!(old.is_none());
                    Ok(())
                }
            }

            let srv = Arc::clone(&srv);
            let shutdown_rx = shutdown_rx.clone();
            tokio::spawn(async move {
                let mut invocations = bindings::serve(srv.as_ref(), Component::default())
                    .await
                    .context("failed to serve `wrpc-test:integration/test`")?;
                let mut invocations = stream::select_all(invocations.into_iter().map(
                    |(instance, name, invocations)| {
                        invocations.map(move |res| (instance, name, res))
                    },
                ));
                loop {
                    let shutdown_rx = shutdown_rx.clone();
                    select! {
                        Some((instance, name, invocation)) = invocations.next() => {
                            info!(instance, name, "serving invocation");
                            invocation
                                .unwrap_or_else(|err| panic!("failed to accept `{instance}#{name}` invocation: {err:?}"))
                                .await
                                .expect("failed to serve invocation");
                        }
                        () = shutdown_rx => {
                            info!("shutting down");
                            return anyhow::Ok(())
                        }
                    }
                }
            })
            .await?
        }.instrument(span.clone()),
        async {
            mod bindings {
                wit_bindgen_wrpc::generate!({
                    inline: "
                       package wrpc-test:integration;

                       interface shared {
                           flags abc {
                               a,
                               b,
                               c,
                           }

                           fallible: func() -> result<bool, string>;
                           numbers: func() -> tuple<u8, u16, u32, u64, s8, s16, s32, s64, f32, f64>;
                           with-flags: func() -> abc;

                           resource counter {
                               constructor(initial: u32);
                               clone-counter: func() -> counter;

                               get-count: func() -> u32;
                               increment-by: func(num: u32);

                               sum: static func(a: borrow<counter>, b: borrow<counter>) -> u32;
                           }
                       }

                       world test {
                           import shared;

                           import f: func(x: string) -> u32;
                           import foo: interface {
                               foo: func(x: string);
                           }
                           export bar: interface {
                               bar: func() -> string;
                           }
                       }"
                });
            }
            use bindings::{exports, foo, f};
            use bindings::wrpc_test::integration::shared;
            use bindings::wrpc_test::integration::shared::Counter;

            struct Component<T>(Arc<T>);

            impl<T> Clone for Component<T> {
                fn clone(&self) -> Self {
                    Self(Arc::clone(&self.0))
                }
            }

            // TODO: Remove the need for this
            sleep(Duration::from_secs(1)).await;

            impl<C, T> exports::bar::Handler<C> for Component<T>
            where
                C: Send + Sync + Default,
                T: wrpc::Invoke<Context = C>,
            {
                async fn bar(&self, _cx: C) -> anyhow::Result<String> {
                    use shared::Abc;

                    info!("calling `wrpc-test:integration/test.foo.f`");
                    foo::foo(self.0.as_ref(), C::default(), "foo")
                        .await
                        .context("failed to call `wrpc-test:integration/test.foo.foo`")?;

                    info!("calling `wrpc-test:integration/test.f`");
                    let v = f(self.0.as_ref(), C::default(), "foo")
                        .await
                        .context("failed to call `wrpc-test:integration/test.f`")?;
                    assert_eq!(v, 42);

                    info!("calling `wrpc-test:integration/shared.fallible`");
                    let v = shared::fallible(self.0.as_ref(), C::default())
                        .await
                        .context("failed to call `wrpc-test:integration/shared.fallible`")?;
                    assert_eq!(v, Ok(true));

                    info!("calling `wrpc-test:integration/shared.numbers`");
                    let v = shared::numbers(self.0.as_ref(), C::default())
                        .await
                        .context("failed to call `wrpc-test:integration/shared.numbers`")?;
                    assert_eq!(
                        v,
                        (
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
                        )
                    );

                    info!("calling `wrpc-test:integration/shared.with-flags`");
                    let v =
                        shared::with_flags(self.0.as_ref(), C::default())
                            .await
                            .context("failed to call `wrpc-test:integration/shared.with-flags`")?;
                    assert_eq!(v, Abc::A | Abc::C);

                    let counter = Counter::new(
                        self.0.as_ref(),
                        C::default(),
                        0,
                    )
                    .await
                    .context(
                        "failed to call `wrpc-test:integration/shared.[constructor]counter`",
                    )?;
                    let counter_borrow = counter.as_borrow();

                    Counter::increment_by(self.0.as_ref(), C::default(), &counter_borrow, 1)
                            .await
                            .context("failed to call `wrpc-test:integration/shared.[method]counter-increment-by`")?;

                    let count = Counter::get_count(
                        self.0.as_ref(),
                        C::default(),
                        &counter_borrow,
                    )
                    .await
                    .context(
                        "failed to call `wrpc-test:integration/shared.[method]counter-get-count`",
                    )?;
                    assert_eq!(count, 1);

                    Counter::increment_by(self.0.as_ref(), C::default(), &counter_borrow, 2)
                            .await
                            .context("failed to call `wrpc-test:integration/shared.[method]counter-increment-by`")?;

                    let count = Counter::get_count(
                        self.0.as_ref(),
                        C::default(),
                        &counter_borrow,
                    )
                    .await
                    .context(
                        "failed to call `wrpc-test:integration/shared.[method]counter-get-count`",
                    )?;
                    assert_eq!(count, 3);

                    let second_counter = Counter::clone_counter(self.0.as_ref(), C::default(), &counter_borrow)
                            .await
                            .context("failed to call `wrpc-test:integration/shared.[method]counter-clone-counter`")?;

                    let second_counter_borrow = second_counter.as_borrow();
                    let sum = Counter::sum(
                        self.0.as_ref(),
                        C::default(),
                        &counter_borrow,
                        &second_counter_borrow,
                    )
                    .await
                    .context("failed to call `wrpc-test:integration/shared.[static]counter-sum")?;
                    assert_eq!(sum, 6);

                    Ok("bar".to_string())
                }
            }

            let mut invocations = bindings::serve(srv.as_ref(), Component(Arc::clone(&clt)))
                .await
                .context("failed to serve `wrpc-test:integration/test`")?;
            let mut invocations = stream::select_all(invocations.into_iter().map(
                |(instance, name, invocations)| invocations.map(move |res| (instance, name, res)),
            ));
            let shutdown_rx = shutdown_rx.clone();
            tokio::spawn(async move {
                loop {
                    let shutdown_rx = shutdown_rx.clone();
                    select! {
                        Some((instance, name, invocation)) = invocations.next() => {
                            info!(instance, name, "serving invocation");
                            invocation
                                .unwrap_or_else(|err| panic!("failed to accept `{instance}#{name}` invocation: {err:?}"))
                                .await
                                .expect("failed to serve invocation");
                        }
                        () = shutdown_rx => {
                            info!("shutting down");
                            return anyhow::Ok(())
                        }
                    }
                }
            }.instrument(span.clone()))
            .await?
        }.instrument(span.clone()),
        async {
            mod bindings {
                wit_bindgen_wrpc::generate!({
                    inline: "
                       package wrpc-test:integration;

                       world test {
                           import bar: interface {
                               bar: func() -> string;
                           }
                       }"
                });
            }
            use bindings::bar;

            // TODO: Remove the need for this
            sleep(Duration::from_secs(2)).await;

            let v = bar::bar(clt.as_ref(), C::default())
                .await
                .context("failed to call `wrpc-test:integration/test.bar.bar`")?;
            assert_eq!(v, "bar");
            shutdown_tx.send(()).expect("failed to send shutdown");
            Ok(())
        }.instrument(span.clone()),
    )?;
    Ok(())
}

#[instrument(skip_all, ret)]
async fn assert_dynamic<C, I, S>(clt: Arc<I>, srv: Arc<S>) -> anyhow::Result<()>
where
    C: Send + Sync + Default + 'static,
    I: wrpc::Invoke<Context = C>,
    S: wrpc::Serve<Context = C>,
{
    use core::pin::pin;

    use tokio::io::{AsyncRead, AsyncReadExt as _};

    let async_inv = srv
        .serve_values(
            "test",
            "async",
            [
                Box::from([Some(0)]),
                Box::from([Some(1)]),
                Box::from([Some(2)]),
                Box::from([Some(3)]),
                Box::from([Some(4)]),
            ],
        )
        .await
        .context("failed to serve `test.async`")?;
    let reset_inv = srv
        .serve_values::<(String,), (String,)>("test", "reset", [Box::default(); 0])
        .await
        .context("failed to serve `test.reset`")?;
    let sync_inv = srv
        .serve_values("test", "sync", [Box::default(); 0])
        .await
        .context("failed to serve `test.sync`")?;

    let mut async_inv = pin!(async_inv);
    let mut reset_inv = pin!(reset_inv);
    let mut sync_inv = pin!(sync_inv);

    join!(
        async {
            info!("receiving `test.reset` parameters");
            reset_inv
                .try_next()
                .await
                .expect("failed to accept invocation")
                .expect("unexpected end of stream");
            info!("receiving `test.reset` parameters");
            reset_inv
                .try_next()
                .await
                .expect("failed to accept invocation")
                .expect("unexpected end of stream");
            let inv = reset_inv
                .try_next()
                .await
                .expect("failed to accept invocation")
                .expect("unexpected end of stream");
            thread::spawn(|| inv);
            anyhow::Ok(())
        }
        .instrument(info_span!("server")),
        async {
            info!("invoking `test.reset`");
            clt.invoke_values_blocking::<_, _, (String,)>(
                C::default(),
                "test",
                "reset",
                ("arg",),
                &[[]; 0],
            )
            .await
            .expect_err("`test.reset` should have failed");
            info!("invoking `test.reset`");
            clt.invoke_values_blocking::<_, _, (String,)>(
                C::default(),
                "test",
                "reset",
                ("arg",),
                &[[]; 0],
            )
            .await
            .expect_err("`test.reset` should have failed");
            info!("invoking `test.reset`");
            clt.invoke_values_blocking::<_, _, (String,)>(
                C::default(),
                "test",
                "reset",
                ("arg",),
                &[[]; 0],
            )
            .await
            .expect_err("`test.reset` should have failed");
        }
        .instrument(info_span!("client")),
    );

    join!(
        async {
            info!("receiving `test.sync` parameters");
            let (_, params, rx, tx) = sync_inv
                .try_next()
                .await
                .expect("failed to accept invocation")
                .expect("unexpected end of stream");
            let (a, b, c, d, e, f, g, h, i, j, k, l, m, n, o): (
                bool,
                u8,
                u16,
                u32,
                u64,
                i8,
                i16,
                i32,
                i64,
                f32,
                f64,
                char,
                String,
                Vec<Vec<Vec<u8>>>,
                Vec<Option<Vec<Result<Option<String>, ()>>>>,
            ) = params;
            assert!(rx.is_none());
            assert!(a);
            assert_eq!(b, 0xfe);
            assert_eq!(c, 0xfeff);
            assert_eq!(d, 0xfeff_ffff);
            assert_eq!(e, 0xfeff_ffff_ffff_ffff);
            assert_eq!(f, 0x7e);
            assert_eq!(g, 0x7eff);
            assert_eq!(h, 0x7eff_ffff);
            assert_eq!(i, 0x7eff_ffff_ffff_ffff);
            assert_eq!(j, 0.42);
            assert_eq!(k, 0.4242);
            assert_eq!(l, 'a');
            assert_eq!(m, "test");
            assert_eq!(n, [[b"foo"]]);
            assert_eq!(o, [Some(vec![Ok(Some(String::from("bar")))])]);
            info!("transmitting `test.sync` returns");
            tx((
                true,
                0xfe_u8,
                0xfeff_u16,
                0xfeff_ffff_u32,
                0xfeff_ffff_ffff_ffff_u64,
                0x7e_i8,
                0x7eff_i16,
                0x7eff_ffff_i32,
                0x7eff_ffff_ffff_ffff_i64,
                0.42_f32,
                0.4242_f64,
                'a',
                "test",
                vec![vec!["foo".as_bytes()]],
                vec![Some(vec![Ok::<_, ()>(Some(String::from("bar")))])],
            ))
            .await
            .expect("failed to send response");
        }
        .instrument(info_span!("server")),
        async {
            info!("invoking `test.sync`");
            let returns = clt
                .invoke_values_blocking(
                    C::default(),
                    "test",
                    "sync",
                    (
                        (
                            true,
                            0xfe_u8,
                            0xfeff_u16,
                            0xfeff_ffff_u32,
                            0xfeff_ffff_ffff_ffff_u64,
                            0x7e_i8,
                            0x7eff_i16,
                            0x7eff_ffff_i32,
                            0x7eff_ffff_ffff_ffff_i64,
                            0.42_f32,
                            0.4242_f64,
                            'a',
                        ),
                        "test",
                        vec![vec!["foo".as_bytes()]],
                        vec![Some(vec![Ok::<_, ()>(Some(String::from("bar")))])],
                    ),
                    &[[]; 0],
                )
                .await
                .expect("failed to invoke `test.sync`");
            let (a, b, c, d, e, f, g, h, i, j, k, l, m, n, o): (
                bool,
                u8,
                u16,
                u32,
                u64,
                i8,
                i16,
                i32,
                i64,
                f32,
                f64,
                char,
                String,
                Vec<Vec<Vec<u8>>>,
                Vec<Option<Vec<Result<Option<String>, ()>>>>,
            ) = returns;
            assert!(a);
            assert_eq!(b, 0xfe);
            assert_eq!(c, 0xfeff);
            assert_eq!(d, 0xfeff_ffff);
            assert_eq!(e, 0xfeff_ffff_ffff_ffff);
            assert_eq!(f, 0x7e);
            assert_eq!(g, 0x7eff);
            assert_eq!(h, 0x7eff_ffff);
            assert_eq!(i, 0x7eff_ffff_ffff_ffff);
            assert_eq!(j, 0.42);
            assert_eq!(k, 0.4242);
            assert_eq!(l, 'a');
            assert_eq!(m, "test");
            assert_eq!(n, [[b"foo"]]);
            assert_eq!(o, [Some(vec![Ok(Some(String::from("bar")))])]);
        }
        .instrument(info_span!("client")),
    );

    join!(
        async {
            info!("receiving `test.async` parameters");
            let (_, params, rx, tx) = async_inv
                .try_next()
                .await
                .expect("failed to accept invocation")
                .expect("unexpected end of stream");
            let (a, b, mut c, d, e): (
                Pin<Box<dyn Stream<Item = Vec<u32>> + Send>>,
                Pin<Box<dyn Stream<Item = Vec<String>> + Send>>,
                Pin<Box<dyn AsyncRead + Send>>,
                Pin<Box<dyn Future<Output = Vec<u64>> + Send>>,
                Pin<Box<dyn Stream<Item = Bytes> + Send>>,
            ) = params;
            let io = rx.map(Instrument::in_current_span).map(spawn);
            join!(
                async {
                    info!("receiving `a`");
                    assert_eq!(a.collect::<Vec<_>>().await.concat(), [0xc0, 0xff, 0xee]);
                },
                async {
                    info!("receiving `b`");
                    assert_eq!(b.collect::<Vec<_>>().await.concat(), ["foo", "bar"]);
                },
                async {
                    info!("receiving `c`");
                    let mut buf = String::new();
                    c.read_to_string(&mut buf)
                        .await
                        .expect("failed to read string from stream");
                    assert_eq!(buf, "test");
                },
                async {
                    info!("receiving `d`");
                    assert_eq!(d.await, [1, 2, 3]);
                },
                async {
                    info!("receiving `e`");
                    assert_eq!(
                        e.collect::<Vec<_>>().await.concat(),
                        b"abcd".repeat(1 << 16)
                    );
                },
                async {
                    if let Some(io) = io {
                        info!("performing I/O");
                        io.await.expect("failed to complete async I/O");
                    }
                }
            );
            let a: Pin<Box<dyn Stream<Item = Vec<u32>> + Send>> =
                Box::pin(stream::iter([vec![0xc0, 0xff], vec![0xee]]));

            let b: Pin<Box<dyn Stream<Item = Vec<&str>> + Send>> =
                Box::pin(stream::iter([vec!["foo"], vec!["bar"]]));
            let c: Pin<Box<dyn AsyncRead + Send>> = Box::pin(b"test".as_slice());
            let d: Pin<Box<dyn Future<Output = Vec<u64>> + Send>> =
                Box::pin(async { vec![1, 2, 3] });
            let e: Pin<Box<dyn Stream<Item = Bytes> + Send>> =
                Box::pin(stream::repeat(Bytes::from("abcd".repeat(1 << 13))).take(1 << 3));
            info!("transmitting `test.async` returns");
            tx((a, b, c, d, e)).await.expect("failed to send response");
        }
        .instrument(info_span!("server")),
        async {
            let a: Pin<Box<dyn Stream<Item = Vec<u32>> + Send>> =
                Box::pin(stream::iter([vec![0xc0, 0xff, 0xee]]));
            let b: Pin<Box<dyn Stream<Item = Vec<&str>> + Send>> =
                Box::pin(stream::iter([vec!["foo", "bar"]]));
            let c: Pin<Box<dyn AsyncRead + Send>> = Box::pin(b"test".as_slice());
            let d: Pin<Box<dyn Future<Output = Vec<u64>> + Send>> =
                Box::pin(async { vec![1, 2, 3] });
            let e: Pin<Box<dyn Stream<Item = Bytes> + Send>> =
                Box::pin(stream::repeat(Bytes::from("abcd".repeat(1 << 13))).take(1 << 3));
            info!("invoking `test.async`");
            let (returns, io) = clt
                .invoke_values(
                    C::default(),
                    "test",
                    "async",
                    (a, b, c, d, e),
                    &[[Some(0)], [Some(1)], [Some(2)], [Some(3)], [Some(4)]],
                )
                .await
                .expect("failed to invoke `test.async`");
            let (a, b, mut c, d, e): (
                Pin<Box<dyn Stream<Item = Vec<u32>> + Send>>,
                Pin<Box<dyn Stream<Item = Vec<String>> + Send>>,
                Pin<Box<dyn AsyncRead + Send>>,
                Pin<Box<dyn Future<Output = Vec<u64>> + Send>>,
                Pin<Box<dyn Stream<Item = Bytes> + Send>>,
            ) = returns;
            info!("receiving `test.async` async values");
            join!(
                async {
                    info!("receiving `a`");
                    assert_eq!(a.collect::<Vec<_>>().await.concat(), [0xc0, 0xff, 0xee]);
                },
                async {
                    info!("receiving `b`");
                    assert_eq!(b.collect::<Vec<_>>().await.concat(), ["foo", "bar"]);
                },
                async {
                    info!("receiving `c`");
                    let mut buf = String::new();
                    c.read_to_string(&mut buf)
                        .await
                        .expect("failed to read string from stream");
                    assert_eq!(buf, "test");
                },
                async {
                    info!("receiving `d`");
                    assert_eq!(d.await, [1, 2, 3]);
                },
                async {
                    info!("receiving `e`");
                    assert_eq!(
                        e.collect::<Vec<_>>().await.concat(),
                        b"abcd".repeat(1 << 16)
                    );
                },
                async {
                    if let Some(io) = io {
                        info!("performing I/O");
                        io.await.expect("failed to complete async I/O");
                    }
                }
            );
        }
        .instrument(info_span!("client")),
    );

    Ok(())
}

#[cfg(feature = "nats")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_bindgen_nats_sync() -> anyhow::Result<()> {
    wrpc_test::with_nats(|_, nats_client| async {
        let clt = wrpc_transport_nats::Client::new(
            nats_client,
            "rust-bindgen-sync",
            Some("rust-bindgen-sync".into()),
        )
        .await
        .context("failed to construct client")?;
        let clt = Arc::new(clt);
        assert_bindgen_sync(Arc::clone(&clt), clt).await
    })
    .await
}

#[cfg(feature = "nats")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_bindgen_nats_async() -> anyhow::Result<()> {
    wrpc_test::with_nats(|_, nats_client| {
        async {
            let clt = wrpc_transport_nats::Client::new(
                nats_client,
                "rust-bindgen-async",
                Some("rust-bindgen-async".into()),
            )
            .await
            .context("failed to construct client")?;
            let clt = Arc::new(clt);
            assert_bindgen_async(Arc::clone(&clt), clt).await
        }
        .in_current_span()
    })
    .await
}

#[cfg(feature = "nats")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_dynamic_nats() -> anyhow::Result<()> {
    wrpc_test::with_nats(|_, nats_client| async {
        let clt = wrpc_transport_nats::Client::new(nats_client, "rust-dynamic", None)
            .await
            .context("failed to construct client")?;
        let clt = Arc::new(clt);
        assert_dynamic(Arc::clone(&clt), clt).await
    })
    .await
}

#[cfg(feature = "quic")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_bindgen_quic_sync() -> anyhow::Result<()> {
    use core::net::Ipv6Addr;
    use core::pin::pin;

    wrpc_test::with_quic(|clt, srv| {
        async move {
            let clt = wrpc_transport_quic::Client::from(clt);
            let srv_conn = wrpc_transport_quic::Client::from(srv);
            let srv = Arc::new(wrpc_transport::frame::Server::default());

            let mut fut = pin!(async {
                let clt = Arc::new(clt);
                assert_bindgen_sync(Arc::clone(&clt), Arc::clone(&srv)).await
            });
            loop {
                select! {
                    res = &mut fut => {
                        return res
                    }
                    res = srv.accept(&srv_conn) => {
                        res.expect("failed to accept connection");
                        continue
                    }
                }
            }
        }
        .in_current_span()
    })
    .await
}

#[cfg(feature = "quic")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_bindgen_quic_async() -> anyhow::Result<()> {
    use core::net::Ipv6Addr;
    use core::pin::pin;

    wrpc_test::with_quic(|clt, srv| async move {
        let clt = wrpc_transport_quic::Client::from(clt);
        let srv_conn = wrpc_transport_quic::Client::from(srv);
        let srv = Arc::new(wrpc_transport::frame::Server::default());

        let mut fut = pin!(async {
            let clt = Arc::new(clt);
            assert_bindgen_async(Arc::clone(&clt), Arc::clone(&srv)).await
        }
        .in_current_span());
        loop {
            select! {
                res = &mut fut => {
                    return res
                }
                res = srv.accept(&srv_conn) => {
                    res.expect("failed to accept connection");
                    continue
                }
            }
        }
    })
    .await
}

#[cfg(feature = "quic")]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_dynamic_quic() -> anyhow::Result<()> {
    use core::net::Ipv6Addr;
    use core::pin::pin;

    use tracing::Span;

    let span = Span::current();
    wrpc_test::with_quic(|clt, srv| {
        async move {
            let clt = wrpc_transport_quic::Client::from(clt);
            let srv_conn = wrpc_transport_quic::Client::from(srv);
            let srv = Arc::new(wrpc_transport::frame::Server::default());

            let mut fut = pin!(assert_dynamic(Arc::new(clt), Arc::clone(&srv)));
            loop {
                select! {
                    res = &mut fut => {
                        return res
                    }
                    res = srv.accept(&srv_conn) => {
                        res.expect("failed to accept connection");
                        continue
                    }
                }
            }
            Ok(())
        }
        .instrument(span)
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_bindgen_tcp_sync() -> anyhow::Result<()> {
    use core::net::Ipv6Addr;
    use core::pin::pin;

    use tracing::Span;
    use wrpc_transport::frame::AcceptExt as _;

    let lis = tokio::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
        .await
        .context("failed to start TCP listener")?;
    let lis = lis.map_context(|addr| assert!(addr.ip().is_loopback()));
    let addr = lis.local_addr().context("failed to get server address")?;

    let srv = Arc::new(wrpc_transport::frame::Server::default());
    let clt = wrpc_transport::frame::tcp::Client::from(addr);
    let span = Span::current();
    let mut fut = pin!(
        async { assert_bindgen_sync(Arc::new(clt), Arc::clone(&srv),).await }
            .instrument(span.clone())
    );
    loop {
        select! {
            res = &mut fut => {
                return res
            }
            res = srv.accept(&lis).instrument(span.clone()) => {
                res.expect("failed to accept connection");
                continue
            }
        }
    }
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_bindgen_tcp_async() -> anyhow::Result<()> {
    use core::net::Ipv6Addr;
    use core::pin::pin;

    use tracing::Span;
    use wrpc_transport::frame::AcceptExt as _;

    let lis = tokio::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
        .await
        .context("failed to start TCP listener")?;
    let lis = lis.map_context(|addr| assert!(addr.ip().is_loopback()));
    let addr = lis.local_addr().context("failed to get server address")?;

    let srv = Arc::new(wrpc_transport::frame::Server::default());
    let clt = wrpc_transport::frame::tcp::Client::from(addr);
    let span = Span::current();
    let mut fut = pin!(
        async { assert_bindgen_async(Arc::new(clt), Arc::clone(&srv),).await }
            .instrument(span.clone())
    );
    loop {
        select! {
            res = &mut fut => {
                return res
            }
            res = srv.accept(&lis).instrument(span.clone()) => {
                res.expect("failed to accept connection");
                continue
            }
        }
    }
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_bindgen_uds_sync() -> anyhow::Result<()> {
    use core::net::Ipv6Addr;
    use core::pin::pin;

    use std::path::PathBuf;

    use tempfile::NamedTempFile;
    use tracing::Span;
    use wrpc_transport::frame::AcceptExt as _;

    let tmp = NamedTempFile::new().context("failed to create temporary file")?;
    let path = PathBuf::from(&tmp.into_temp_path());

    let lis = tokio::net::UnixListener::bind(&path).context("failed to bind Unix listener")?;
    let lis = lis.map_context(|addr| assert!(addr.is_unnamed()));

    let srv = Arc::new(wrpc_transport::frame::Server::default());
    let clt = wrpc_transport::frame::unix::Client::from(path);
    let span = Span::current();
    let mut fut = pin!(
        async { assert_bindgen_sync(Arc::new(clt), Arc::clone(&srv),).await }
            .instrument(span.clone())
    );
    loop {
        select! {
            res = &mut fut => {
                return res
            }
            res = srv.accept(&lis).instrument(span.clone()) => {
                res.expect("failed to accept connection");
                continue
            }
        }
    }
}

#[cfg(unix)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
#[instrument(ret)]
async fn rust_bindgen_uds_async() -> anyhow::Result<()> {
    use core::net::Ipv6Addr;
    use core::pin::pin;

    use std::path::PathBuf;

    use tempfile::NamedTempFile;
    use tracing::Span;
    use wrpc_transport::frame::AcceptExt as _;

    let tmp = NamedTempFile::new().context("failed to create temporary file")?;
    let path = PathBuf::from(&tmp.into_temp_path());

    let lis = tokio::net::UnixListener::bind(&path).context("failed to bind Unix listener")?;
    let lis = lis.map_context(|addr| assert!(addr.is_unnamed()));

    let srv = Arc::new(wrpc_transport::frame::Server::default());
    let clt = wrpc_transport::frame::unix::Client::from(path);
    let span = Span::current();
    let mut fut = pin!(
        async { assert_bindgen_async(Arc::new(clt), Arc::clone(&srv),).await }
            .instrument(span.clone())
    );
    loop {
        select! {
            res = &mut fut => {
                return res
            }
            res = srv.accept(&lis).instrument(span.clone()) => {
                res.expect("failed to accept connection");
                continue
            }
        }
    }
}
