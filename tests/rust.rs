#![allow(clippy::type_complexity)]

mod common;

use core::net::Ipv6Addr;
use core::pin::pin;
use core::str;
use core::time::Duration;

use std::sync::Arc;

use anyhow::Context;
use bytes::Bytes;
use futures::{stream, FutureExt as _, Stream, StreamExt as _, TryStreamExt as _};
use tokio::sync::{oneshot, RwLock};
use tokio::time::sleep;
use tokio::{join, select, spawn, try_join};
use tracing::{info, info_span, instrument, Instrument};
use wrpc_transport::{Invoke as _, ResourceBorrow, ResourceOwn, Serve as _};

use common::{with_nats, with_quic};

// TODO: Migrate
//#[instrument(skip(client, ty, params, results))]
//async fn loopback_dynamic(
//    client: &impl wrpc_transport_legacy::Client,
//    name: &str,
//    ty: DynamicFunctionType,
//    params: Vec<Value>,
//    results: Vec<Value>,
//) -> anyhow::Result<(Vec<Value>, Vec<Value>)> {
//    match ty {
//        DynamicFunctionType::Method {
//            receiver: _,
//            params: _,
//            results: _,
//        } => todo!("methods not supported yet"),
//        DynamicFunctionType::Static {
//            params: params_ty,
//            results: results_ty,
//        } => {
//            info!("serve function");
//            let invocations = client
//                .serve_dynamic("wrpc:wrpc/test-dynamic", name, params_ty)
//                .await
//                .context("failed to serve static function")?;
//            let (params, results) = try_join!(
//                async {
//                    info!("await invocation");
//                    let AcceptedInvocation {
//                        params,
//                        result_subject,
//                        transmitter,
//                        ..
//                    } = pin!(invocations)
//                        .try_next()
//                        .await
//                        .with_context(|| "unexpected end of invocation stream".to_string())?
//                        .with_context(|| "failed to decode parameters".to_string())?;
//                    info!("transmit response to invocation");
//                    transmitter
//                        .transmit_tuple_dynamic(result_subject, results)
//                        .await
//                        .context("failed to transmit result tuple")?;
//                    info!("finished serving invocation");
//                    anyhow::Ok(params)
//                },
//                async {
//                    info!("invoke function");
//                    let (results, params_tx) = client
//                        .invoke_dynamic(
//                            "wrpc:wrpc/test-dynamic",
//                            name,
//                            DynamicTuple(params),
//                            &results_ty,
//                        )
//                        .await
//                        .with_context(|| "failed to invoke static function".to_string())?;
//                    info!("transmit async parameters");
//                    params_tx
//                        .await
//                        .context("failed to transmit parameter tuple")?;
//                    info!("finished invocation");
//                    Ok(results)
//                },
//            )?;
//            Ok((params, results))
//        }
//    }
//}

async fn assert_bindgen<C, I, S>(i: Arc<I>, s: Arc<S>) -> anyhow::Result<()>
where
    C: Send + Sync + Default,
    I: wrpc::Invoke<Context = C>,
    S: wrpc::Serve<Context = C>,
{
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let shutdown_rx = async move { shutdown_rx.await.expect("shutdown sender dropped") }.shared();
    try_join!(
        async {
            wrpc::generate!({
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

            use exports::wrpc_test::integration::shared::Counter;

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

            impl<C: Send + Sync> Handler<C> for Component {
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

            serve(s.as_ref(), Component::default(), shutdown_rx.clone())
                .await
                .context("failed to serve `wrpc-test:integration/test`")
        },
        async {
            wrpc::generate!({
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
                    use wrpc_test::integration::shared::Abc;

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
                    let v = wrpc_test::integration::shared::fallible(self.0.as_ref(), C::default())
                        .await
                        .context("failed to call `wrpc-test:integration/shared.fallible`")?;
                    assert_eq!(v, Ok(true));

                    info!("calling `wrpc-test:integration/shared.numbers`");
                    let v = wrpc_test::integration::shared::numbers(self.0.as_ref(), C::default())
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
                        wrpc_test::integration::shared::with_flags(self.0.as_ref(), C::default())
                            .await
                            .context("failed to call `wrpc-test:integration/shared.with-flags`")?;
                    assert_eq!(v, Abc::A | Abc::C);

                    let counter = wrpc_test::integration::shared::Counter::new(
                        self.0.as_ref(),
                        C::default(),
                        0,
                    )
                    .await
                    .context(
                        "failed to call `wrpc-test:integration/shared.[constructor]counter`",
                    )?;
                    let counter_borrow = counter.as_borrow();

                    wrpc_test::integration::shared::Counter::increment_by(self.0.as_ref(), C::default(), &counter_borrow, 1)
                            .await
                            .context("failed to call `wrpc-test:integration/shared.[method]counter-increment-by`")?;

                    let count = wrpc_test::integration::shared::Counter::get_count(
                        self.0.as_ref(),
                        C::default(),
                        &counter_borrow,
                    )
                    .await
                    .context(
                        "failed to call `wrpc-test:integration/shared.[method]counter-get-count`",
                    )?;
                    assert_eq!(count, 1);

                    wrpc_test::integration::shared::Counter::increment_by(self.0.as_ref(), C::default(), &counter_borrow, 2)
                            .await
                            .context("failed to call `wrpc-test:integration/shared.[method]counter-increment-by`")?;

                    let count = wrpc_test::integration::shared::Counter::get_count(
                        self.0.as_ref(),
                        C::default(),
                        &counter_borrow,
                    )
                    .await
                    .context(
                        "failed to call `wrpc-test:integration/shared.[method]counter-get-count`",
                    )?;
                    assert_eq!(count, 3);

                    let second_counter = wrpc_test::integration::shared::Counter::clone_counter(self.0.as_ref(), C::default(), &counter_borrow)
                            .await
                            .context("failed to call `wrpc-test:integration/shared.[method]counter-clone-counter`")?;

                    let second_counter_borrow = second_counter.as_borrow();
                    let sum = wrpc_test::integration::shared::Counter::sum(
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

            serve(s.as_ref(), Component(Arc::clone(&i)), shutdown_rx.clone())
                .await
                .context("failed to serve `wrpc-test:integration/test`")
        },
        async {
            wrpc::generate!({
                inline: "
                        package wrpc-test:integration;

                        world test {
                            import bar: interface {
                                bar: func() -> string;
                            }
                        }"
            });

            // TODO: Remove the need for this
            sleep(Duration::from_secs(2)).await;

            let v = bar::bar(i.as_ref(), C::default())
                .await
                .context("failed to call `wrpc-test:integration/test.bar.bar`")?;
            assert_eq!(v, "bar");
            shutdown_tx.send(()).expect("failed to send shutdown");
            Ok(())
        },
    )?;
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_bindgen_nats() -> anyhow::Result<()> {
    with_nats(|_, nats_client| async {
        let client = wrpc_transport_nats::Client::new(nats_client, "test-prefix".to_string());
        let client = Arc::new(client);
        assert_bindgen(Arc::clone(&client), client).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_bindgen_quic() -> anyhow::Result<()> {
    with_quic(
        &[
            "*.wrpc-test_integration__bar",
            "*.wrpc-test_integration__foo",
            "*.wrpc-test_integration__shared",
            "*.wrpc-test_integration__test",
            "*.counter.wrpc-test_integration__shared",
        ],
        |port, clt_ep, srv_ep| async move {
            let clt = wrpc_transport_quic::Client::new(clt_ep, (Ipv6Addr::LOCALHOST, port));
            let srv = wrpc_transport_quic::Server::default();

            let srv = Arc::new(srv);
            let mut fut = pin!(assert_bindgen(Arc::new(clt), Arc::clone(&srv)));
            loop {
                select! {
                    res = &mut fut => {
                        return res
                    }
                    res = srv.accept(&srv_ep) => {
                        let ok = res.expect("failed to accept connection");
                        assert!(ok);
                        continue
                    }
                }
            }
        },
    )
    .await
}

#[instrument(ret)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn rust_dynamic() -> anyhow::Result<()> {
    with_quic(
        &["sync.test", "async.test"],
        |port, clt_ep, srv_ep| async move {
            let clt = wrpc_transport_quic::Client::new(clt_ep, (Ipv6Addr::LOCALHOST, port));
            let srv = wrpc_transport_quic::Server::default();

            let async_inv = srv
                .serve_values("test", "async", [[Some(0)], [Some(1)]])
                .await
                .context("failed to serve `test.async`")?;
            let sync_inv = srv
                .serve_values("test", "sync", [[]])
                .await
                .context("failed to serve `test.sync`")?;
            let mut async_inv = pin!(async_inv);
            let mut sync_inv = pin!(sync_inv);

            join!(
                async {
                    info!("accepting `test.sync` connection");
                    let ok = srv
                        .accept(&srv_ep)
                        .await
                        .expect("failed to accept client connection");
                    assert!(ok);
                    info!("receiving `test.sync` parameters");
                    let ((), params, rx, tx) = sync_inv
                        .try_next()
                        .await
                        .expect("failed to accept invocation")
                        .expect("unexpected end of stream");
                    let ((a, b, c, d, e, f, g, h, i, j, k, l), m, n): (
                        (bool, u8, u16, u32, u64, i8, i16, i32, i64, f32, f64, char),
                        String,
                        Vec<Vec<Vec<u8>>>,
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
                    info!("transmitting `test.sync` returns");
                    tx((
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
                    ))
                    .await
                    .expect("failed to send response");
                }
                .instrument(info_span!("server")),
                async {
                    info!("invoking `test.sync`");
                    let returns = clt
                        .invoke_values_blocking(
                            (),
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
                            ),
                            &[[]],
                        )
                        .await
                        .expect("failed to invoke `test.sync`");
                    let ((a, b, c, d, e, f, g, h, i, j, k, l), m, n): (
                        (bool, u8, u16, u32, u64, i8, i16, i32, i64, f32, f64, char),
                        String,
                        Vec<Vec<Vec<u8>>>,
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
                    info!("finishing `test.sync` session");
                }
                .instrument(info_span!("client")),
            );

            join!(
                async {
                    info!("accepting `test.async` connection");
                    let ok = srv
                        .accept(&srv_ep)
                        .await
                        .expect("failed to accept client connection");
                    assert!(ok);
                    info!("receiving `test.async` parameters");
                    let ((), params, rx, tx) = async_inv
                        .try_next()
                        .await
                        .expect("failed to accept invocation")
                        .expect("unexpected end of stream");
                    let (a, b): (
                        Pin<Box<dyn Stream<Item = u32> + Send + Sync>>,
                        Pin<Box<dyn Stream<Item = String> + Send + Sync>>,
                    ) = params;
                    let rx = rx.map(Instrument::in_current_span).map(spawn);
                    assert_eq!(a.collect::<Vec<_>>().await, [0xc0, 0xff, 0xee]);
                    assert_eq!(b.collect::<Vec<_>>().await, ["foo", "bar"]);
                    let a: Pin<Box<dyn Stream<Item = u32> + Send + Sync>> =
                        Box::pin(stream::iter([0xc0, 0xff, 0xee]));

                    let b: Pin<Box<dyn Stream<Item = &str> + Send + Sync>> =
                        Box::pin(stream::iter(["foo", "bar"]));
                    if let Some(rx) = rx {
                        rx.await
                            .expect("async receiver panicked")
                            .expect("failed to perform async I/O");
                    }
                    info!("transmitting `test.async` returns");
                    tx((a, b)).await.expect("failed to send response");
                }
                .instrument(info_span!("server")),
                async {
                    let a: Pin<Box<dyn Stream<Item = u32> + Send + Sync>> =
                        Box::pin(stream::iter([0xc0, 0xff, 0xee]));
                    let b: Pin<Box<dyn Stream<Item = &str> + Send + Sync>> =
                        Box::pin(stream::iter(["foo", "bar"]));
                    info!("invoking `test.async`");
                    let (returns, io) = clt
                        .invoke_values((), "test", "async", (a, b), &[[Some(0)], [Some(1)]])
                        .await
                        .expect("failed to invoke `test.async`");
                    let (a, b): (
                        Pin<Box<dyn Stream<Item = u32> + Send + Sync>>,
                        Pin<Box<dyn Stream<Item = String> + Send + Sync>>,
                    ) = returns;
                    info!("receiving `test.async` async values");
                    join!(
                        async { assert_eq!(a.collect::<Vec<_>>().await, [0xc0, 0xff, 0xee]) },
                        async {
                            assert_eq!(b.collect::<Vec<_>>().await, ["foo", "bar"]);
                        },
                        async {
                            if let Some(io) = io {
                                io.await.expect("failed to complete async I/O");
                            }
                        }
                    );
                }
                .instrument(info_span!("client")),
            );
            Ok(())
        },
    )
    .await
    // TODO: migrate the whole test suite
    //    with_nats(|_, nats_client| async {
    //        let client = wrpc_transport_nats_legacy::Client::new(nats_client, "test-prefix".to_string());
    //        let client = Arc::new(client);
    //
    //        let (params, results) = loopback_dynamic(
    //            client.as_ref(),
    //            "unit_unit",
    //            DynamicFunctionType::Static {
    //                params: vec![].into(),
    //                results: vec![].into(),
    //            },
    //            vec![],
    //            vec![],
    //        )
    //        .await
    //        .context("failed to invoke `unit_unit`")?;
    //        assert!(params.is_empty());
    //        assert!(results.is_empty());
    //
    //        let flat_types = vec![
    //            Type::Bool,
    //            Type::U8,
    //            Type::U16,
    //            Type::U32,
    //            Type::U64,
    //            Type::S8,
    //            Type::S16,
    //            Type::S32,
    //            Type::S64,
    //            Type::F32,
    //            Type::F64,
    //            Type::Char,
    //            Type::String,
    //            Type::Enum,
    //            Type::Flags,
    //        ]
    //        .into();
    //
    //        let flat_variant_type = vec![None, Some(Type::Bool)].into();
    //
    //        let sync_tuple_type = vec![
    //            Type::Bool,
    //            Type::U8,
    //            Type::U16,
    //            Type::U32,
    //            Type::U64,
    //            Type::S8,
    //            Type::S16,
    //            Type::S32,
    //            Type::S64,
    //            Type::F32,
    //            Type::F64,
    //            Type::Char,
    //            Type::String,
    //            Type::List(Arc::new(Type::U64)),
    //            Type::List(Arc::new(Type::Bool)),
    //            Type::Record(Arc::clone(&flat_types)),
    //            Type::Tuple(Arc::clone(&flat_types)),
    //            Type::Variant(Arc::clone(&flat_variant_type)),
    //            Type::Variant(Arc::clone(&flat_variant_type)),
    //            Type::Enum,
    //            Type::Option(Arc::new(Type::Bool)),
    //            Type::Result {
    //                ok: Some(Arc::new(Type::Bool)),
    //                err: Some(Arc::new(Type::Bool)),
    //            },
    //            Type::Flags,
    //        ]
    //        .into();
    //
    //        let (params, results) = loopback_dynamic(
    //            client.as_ref(),
    //            "sync",
    //            DynamicFunctionType::Static {
    //                params: Arc::clone(&sync_tuple_type),
    //                results: sync_tuple_type,
    //            },
    //            vec![
    //                Value::Bool(true),
    //                Value::U8(0xfe),
    //                Value::U16(0xfeff),
    //                Value::U32(0xfeff_ffff),
    //                Value::U64(0xfeff_ffff_ffff_ffff),
    //                Value::S8(0x7e),
    //                Value::S16(0x7eff),
    //                Value::S32(0x7eff_ffff),
    //                Value::S64(0x7eff_ffff_ffff_ffff),
    //                Value::F32(0.42),
    //                Value::F64(0.4242),
    //                Value::Char('a'),
    //                Value::String("test".into()),
    //                Value::List(vec![]),
    //                Value::List(vec![
    //                    Value::Bool(true),
    //                    Value::Bool(false),
    //                    Value::Bool(true),
    //                ]),
    //                Value::Record(vec![
    //                    Value::Bool(true),
    //                    Value::U8(0xfe),
    //                    Value::U16(0xfeff),
    //                    Value::U32(0xfeff_ffff),
    //                    Value::U64(0xfeff_ffff_ffff_ffff),
    //                    Value::S8(0x7e),
    //                    Value::S16(0x7eff),
    //                    Value::S32(0x7eff_ffff),
    //                    Value::S64(0x7eff_ffff_ffff_ffff),
    //                    Value::F32(0.42),
    //                    Value::F64(0.4242),
    //                    Value::Char('a'),
    //                    Value::String("test".into()),
    //                    Value::Enum(0xfeff),
    //                    Value::Flags(0xdead_beef),
    //                ]),
    //                Value::Tuple(vec![
    //                    Value::Bool(true),
    //                    Value::U8(0xfe),
    //                    Value::U16(0xfeff),
    //                    Value::U32(0xfeff_ffff),
    //                    Value::U64(0xfeff_ffff_ffff_ffff),
    //                    Value::S8(0x7e),
    //                    Value::S16(0x7eff),
    //                    Value::S32(0x7eff_ffff),
    //                    Value::S64(0x7eff_ffff_ffff_ffff),
    //                    Value::F32(0.42),
    //                    Value::F64(0.4242),
    //                    Value::Char('a'),
    //                    Value::String("test".into()),
    //                    Value::Enum(0xfeff),
    //                    Value::Flags(0xdead_beef),
    //                ]),
    //                Value::Variant {
    //                    discriminant: 0,
    //                    nested: None,
    //                },
    //                Value::Variant {
    //                    discriminant: 1,
    //                    nested: Some(Box::new(Value::Bool(true))),
    //                },
    //                Value::Enum(0xfeff),
    //                Value::Option(Some(Box::new(Value::Bool(true)))),
    //                Value::Result(Ok(Some(Box::new(Value::Bool(true))))),
    //                Value::Flags(0xdead_beef),
    //            ],
    //            vec![
    //                Value::Bool(true),
    //                Value::U8(0xfe),
    //                Value::U16(0xfeff),
    //                Value::U32(0xfeff_ffff),
    //                Value::U64(0xfeff_ffff_ffff_ffff),
    //                Value::S8(0x7e),
    //                Value::S16(0x7eff),
    //                Value::S32(0x7eff_ffff),
    //                Value::S64(0x7eff_ffff_ffff_ffff),
    //                Value::F32(0.42),
    //                Value::F64(0.4242),
    //                Value::Char('a'),
    //                Value::String("test".into()),
    //                Value::List(vec![]),
    //                Value::List(vec![
    //                    Value::Bool(true),
    //                    Value::Bool(false),
    //                    Value::Bool(true),
    //                ]),
    //                Value::Record(vec![
    //                    Value::Bool(true),
    //                    Value::U8(0xfe),
    //                    Value::U16(0xfeff),
    //                    Value::U32(0xfeff_ffff),
    //                    Value::U64(0xfeff_ffff_ffff_ffff),
    //                    Value::S8(0x7e),
    //                    Value::S16(0x7eff),
    //                    Value::S32(0x7eff_ffff),
    //                    Value::S64(0x7eff_ffff_ffff_ffff),
    //                    Value::F32(0.42),
    //                    Value::F64(0.4242),
    //                    Value::Char('a'),
    //                    Value::String("test".into()),
    //                    Value::Enum(0xfeff),
    //                    Value::Flags(0xdead_beef),
    //                ]),
    //                Value::Tuple(vec![
    //                    Value::Bool(true),
    //                    Value::U8(0xfe),
    //                    Value::U16(0xfeff),
    //                    Value::U32(0xfeff_ffff),
    //                    Value::U64(0xfeff_ffff_ffff_ffff),
    //                    Value::S8(0x7e),
    //                    Value::S16(0x7eff),
    //                    Value::S32(0x7eff_ffff),
    //                    Value::S64(0x7eff_ffff_ffff_ffff),
    //                    Value::F32(0.42),
    //                    Value::F64(0.4242),
    //                    Value::Char('a'),
    //                    Value::String("test".into()),
    //                    Value::Enum(0xfeff),
    //                    Value::Flags(0xdead_beef),
    //                ]),
    //                Value::Variant {
    //                    discriminant: 0,
    //                    nested: None,
    //                },
    //                Value::Variant {
    //                    discriminant: 1,
    //                    nested: Some(Box::new(Value::Bool(true))),
    //                },
    //                Value::Enum(0xfeff),
    //                Value::Option(Some(Box::new(Value::Bool(true)))),
    //                Value::Result(Ok(Some(Box::new(Value::Bool(true))))),
    //                Value::Flags(0xdead_beef),
    //            ],
    //        )
    //        .await
    //        .context("failed to invoke `sync`")?;
    //        let mut values = zip(params, results);
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::Bool(true), Value::Bool(true))
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::U8(0xfe), Value::U8(0xfe))
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::U16(0xfeff), Value::U16(0xfeff))
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::U32(0xfeff_ffff), Value::U32(0xfeff_ffff))
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (
    //                Value::U64(0xfeff_ffff_ffff_ffff),
    //                Value::U64(0xfeff_ffff_ffff_ffff)
    //            )
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::S8(0x7e), Value::S8(0x7e))
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::S16(0x7eff), Value::S16(0x7eff))
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::S32(0x7eff_ffff), Value::S32(0x7eff_ffff))
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (
    //                Value::S64(0x7eff_ffff_ffff_ffff),
    //                Value::S64(0x7eff_ffff_ffff_ffff)
    //            )
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::F32(p), Value::F32(r)) if p == 0.42 && r == 0.42
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::F64(p), Value::F64(r)) if p == 0.4242 && r == 0.4242
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::Char('a'), Value::Char('a'))
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::String(p), Value::String(r)) if p == "test" && r == "test"
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::List(p), Value::List(r)) if p.is_empty() && r.is_empty()
    //        ));
    //        let (Value::List(p), Value::List(r)) = values.next().unwrap() else {
    //            bail!("list type mismatch")
    //        };
    //        {
    //            let mut values = zip(p, r);
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::Bool(true), Value::Bool(true))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::Bool(false), Value::Bool(false))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::Bool(true), Value::Bool(true))
    //            ));
    //        }
    //        let (Value::Record(p), Value::Record(r)) = values.next().unwrap() else {
    //            bail!("record type mismatch")
    //        };
    //        {
    //            let mut values = zip(p, r);
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::Bool(true), Value::Bool(true))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::U8(0xfe), Value::U8(0xfe))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::U16(0xfeff), Value::U16(0xfeff))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::U32(0xfeff_ffff), Value::U32(0xfeff_ffff))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (
    //                    Value::U64(0xfeff_ffff_ffff_ffff),
    //                    Value::U64(0xfeff_ffff_ffff_ffff)
    //                )
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::S8(0x7e), Value::S8(0x7e))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::S16(0x7eff), Value::S16(0x7eff))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::S32(0x7eff_ffff), Value::S32(0x7eff_ffff))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (
    //                    Value::S64(0x7eff_ffff_ffff_ffff),
    //                    Value::S64(0x7eff_ffff_ffff_ffff)
    //                )
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::F32(p), Value::F32(r)) if p == 0.42 && r == 0.42
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::F64(p), Value::F64(r)) if p == 0.4242 && r == 0.4242
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::Char('a'), Value::Char('a'))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::String(p), Value::String(r)) if p == "test" && r == "test"
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::Enum(0xfeff), Value::Enum(0xfeff))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::Flags(0xdead_beef), Value::Flags(0xdead_beef))
    //            ));
    //        }
    //        let (Value::Tuple(p), Value::Tuple(r)) = values.next().unwrap() else {
    //            bail!("tuple type mismatch")
    //        };
    //        {
    //            let mut values = zip(p, r);
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::Bool(true), Value::Bool(true))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::U8(0xfe), Value::U8(0xfe))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::U16(0xfeff), Value::U16(0xfeff))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::U32(0xfeff_ffff), Value::U32(0xfeff_ffff))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (
    //                    Value::U64(0xfeff_ffff_ffff_ffff),
    //                    Value::U64(0xfeff_ffff_ffff_ffff)
    //                )
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::S8(0x7e), Value::S8(0x7e))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::S16(0x7eff), Value::S16(0x7eff))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::S32(0x7eff_ffff), Value::S32(0x7eff_ffff))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (
    //                    Value::S64(0x7eff_ffff_ffff_ffff),
    //                    Value::S64(0x7eff_ffff_ffff_ffff)
    //                )
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::F32(p), Value::F32(r)) if p == 0.42 && r == 0.42
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::F64(p), Value::F64(r)) if p == 0.4242 && r == 0.4242
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::Char('a'), Value::Char('a'))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::String(p), Value::String(r)) if p == "test" && r == "test"
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::Enum(0xfeff), Value::Enum(0xfeff))
    //            ));
    //            assert!(matches!(
    //                values.next().unwrap(),
    //                (Value::Flags(0xdead_beef), Value::Flags(0xdead_beef))
    //            ));
    //        }
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (
    //                Value::Variant {
    //                    discriminant: 0,
    //                    nested: None,
    //                },
    //                Value::Variant {
    //                    discriminant: 0,
    //                    nested: None,
    //                }
    //            )
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (
    //                Value::Variant {
    //                    discriminant: 1,
    //                    nested: p,
    //                },
    //                Value::Variant {
    //                    discriminant: 1,
    //                    nested: r,
    //                }
    //            ) if matches!((p.as_deref(), r.as_deref()), (Some(Value::Bool(true)), Some(Value::Bool(true))))
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::Enum(0xfeff), Value::Enum(0xfeff))
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::Option(p), Value::Option(r)) if matches!((p.as_deref(), r.as_deref()), (Some(Value::Bool(true)), Some(Value::Bool(true))))
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::Result(Ok(p)), Value::Result(Ok(r))) if matches!((p.as_deref(), r.as_deref()), (Some(Value::Bool(true)), Some(Value::Bool(true))))
    //        ));
    //        assert!(matches!(
    //            values.next().unwrap(),
    //            (Value::Flags(0xdead_beef), Value::Flags(0xdead_beef))
    //        ));
    //
    //        let async_tuple_type = vec![
    //            Type::Future(None),
    //            Type::Future(Some(Arc::new(Type::Bool))),
    //            Type::Future(Some(Arc::new(Type::Future(None)))),
    //            Type::Future(Some(Arc::new(Type::Future(Some(Arc::new(Type::Future(
    //                Some(Arc::new(Type::Bool)),
    //            ))))))),
    //            Type::Future(Some(Arc::new(Type::Stream(Some(Arc::new(Type::U8)))))),
    //            Type::Stream(None),
    //            Type::Stream(Some(Arc::new(Type::U8))),
    //            Type::Resource(ResourceType::Pollable),
    //            Type::Resource(ResourceType::InputStream),
    //            Type::Tuple(
    //                vec![
    //                    Type::Future(None),
    //                    Type::Future(Some(Arc::new(Type::Bool))),
    //                    Type::Future(Some(Arc::new(Type::Future(None)))),
    //                    Type::Resource(ResourceType::InputStream),
    //                ]
    //                .into(),
    //            ),
    //            Type::Record(
    //                vec![
    //                    Type::Future(None),
    //                    Type::Future(Some(Arc::new(Type::Bool))),
    //                    Type::Future(Some(Arc::new(Type::Future(None)))),
    //                    Type::Resource(ResourceType::InputStream),
    //                ]
    //                .into(),
    //            ),
    //        ]
    //        .into();
    //
    //        let (params, results) = loopback_dynamic(
    //            client.as_ref(),
    //            "async",
    //            DynamicFunctionType::Static {
    //                params: Arc::clone(&async_tuple_type),
    //                results: async_tuple_type,
    //            },
    //            vec![
    //                Value::Future(Box::pin(async { Ok(None) })),
    //                Value::Future(Box::pin(async {
    //                    sleep(Duration::from_nanos(42)).await;
    //                    Ok(Some(Value::Bool(true)))
    //                })),
    //                Value::Future(Box::pin(async {
    //                    Ok(Some(Value::Future(Box::pin(async { Ok(None) }))))
    //                })),
    //                Value::Future(Box::pin(async {
    //                    sleep(Duration::from_nanos(42)).await;
    //                    Ok(Some(Value::Future(Box::pin(async {
    //                        sleep(Duration::from_nanos(42)).await;
    //                        Ok(Some(Value::Future(Box::pin(async {
    //                            sleep(Duration::from_nanos(42)).await;
    //                            Ok(Some(Value::Bool(true)))
    //                        }))))
    //                    }))))
    //                })),
    //                Value::Future(Box::pin(async {
    //                    sleep(Duration::from_nanos(42)).await;
    //                    Ok(Some(Value::Stream(Box::pin(stream::iter([
    //                        Ok(vec![Some(Value::U8(0x42))]),
    //                        Ok(vec![Some(Value::U8(0xff))]),
    //                    ])))))
    //                })),
    //                Value::Stream(Box::pin(stream::iter([Ok(vec![None, None, None, None])]))),
    //                Value::Stream(Box::pin(stream::iter([Ok(vec![
    //                    Some(Value::U8(0x42)),
    //                    Some(Value::U8(0xff)),
    //                ])]))),
    //                Value::Future(Box::pin(async { Ok(None) })),
    //                Value::Stream(Box::pin(
    //                    stream::iter([
    //                        Ok(vec![Some(Value::U8(0x42))]),
    //                        Ok(vec![Some(Value::U8(0xff))]),
    //                    ])
    //                    .then(|item| async {
    //                        sleep(Duration::from_nanos(42)).await;
    //                        item
    //                    }),
    //                )),
    //                Value::Tuple(vec![
    //                    Value::Future(Box::pin(async { Ok(None) })),
    //                    Value::Future(Box::pin(async {
    //                        sleep(Duration::from_nanos(42)).await;
    //                        Ok(Some(Value::Bool(true)))
    //                    })),
    //                    Value::Future(Box::pin(async {
    //                        Ok(Some(Value::Future(Box::pin(async { Ok(None) }))))
    //                    })),
    //                    Value::Stream(Box::pin(
    //                        stream::iter([
    //                            Ok(vec![Some(Value::U8(0x42))]),
    //                            Ok(vec![Some(Value::U8(0xff))]),
    //                        ])
    //                        .then(|item| async {
    //                            sleep(Duration::from_nanos(42)).await;
    //                            item
    //                        }),
    //                    )),
    //                ]),
    //                Value::Record(vec![
    //                    Value::Future(Box::pin(async { Ok(None) })),
    //                    Value::Future(Box::pin(async {
    //                        sleep(Duration::from_nanos(42)).await;
    //                        Ok(Some(Value::Bool(true)))
    //                    })),
    //                    Value::Future(Box::pin(async {
    //                        Ok(Some(Value::Future(Box::pin(async { Ok(None) }))))
    //                    })),
    //                    Value::Stream(Box::pin(
    //                        stream::iter([
    //                            Ok(vec![Some(Value::U8(0x42))]),
    //                            Ok(vec![Some(Value::U8(0xff))]),
    //                        ])
    //                        .then(|item| async {
    //                            sleep(Duration::from_nanos(42)).await;
    //                            item
    //                        }),
    //                    )),
    //                ]),
    //            ],
    //            vec![
    //                Value::Future(Box::pin(async { Ok(None) })),
    //                Value::Future(Box::pin(async {
    //                    sleep(Duration::from_nanos(42)).await;
    //                    Ok(Some(Value::Bool(true)))
    //                })),
    //                Value::Future(Box::pin(async {
    //                    Ok(Some(Value::Future(Box::pin(async { Ok(None) }))))
    //                })),
    //                Value::Future(Box::pin(async {
    //                    sleep(Duration::from_nanos(42)).await;
    //                    Ok(Some(Value::Future(Box::pin(async {
    //                        sleep(Duration::from_nanos(42)).await;
    //                        Ok(Some(Value::Future(Box::pin(async {
    //                            sleep(Duration::from_nanos(42)).await;
    //                            Ok(Some(Value::Bool(true)))
    //                        }))))
    //                    }))))
    //                })),
    //                Value::Future(Box::pin(async {
    //                    sleep(Duration::from_nanos(42)).await;
    //                    Ok(Some(Value::Stream(Box::pin(stream::iter([
    //                        Ok(vec![Some(Value::U8(0x42))]),
    //                        Ok(vec![Some(Value::U8(0xff))]),
    //                    ])))))
    //                })),
    //                Value::Stream(Box::pin(stream::iter([Ok(vec![None, None, None, None])]))),
    //                Value::Stream(Box::pin(stream::iter([Ok(vec![
    //                    Some(Value::U8(0x42)),
    //                    Some(Value::U8(0xff)),
    //                ])]))),
    //                Value::Future(Box::pin(async { Ok(None) })),
    //                Value::Stream(Box::pin(
    //                    stream::iter([
    //                        Ok(vec![Some(Value::U8(0x42))]),
    //                        Ok(vec![Some(Value::U8(0xff))]),
    //                    ])
    //                    .then(|item| async {
    //                        sleep(Duration::from_nanos(42)).await;
    //                        item
    //                    }),
    //                )),
    //                Value::Tuple(vec![
    //                    Value::Future(Box::pin(async { Ok(None) })),
    //                    Value::Future(Box::pin(async {
    //                        sleep(Duration::from_nanos(42)).await;
    //                        Ok(Some(Value::Bool(true)))
    //                    })),
    //                    Value::Future(Box::pin(async {
    //                        Ok(Some(Value::Future(Box::pin(async { Ok(None) }))))
    //                    })),
    //                    Value::Stream(Box::pin(
    //                        stream::iter([
    //                            Ok(vec![Some(Value::U8(0x42))]),
    //                            Ok(vec![Some(Value::U8(0xff))]),
    //                        ])
    //                        .then(|item| async {
    //                            sleep(Duration::from_nanos(42)).await;
    //                            item
    //                        }),
    //                    )),
    //                ]),
    //                Value::Record(vec![
    //                    Value::Future(Box::pin(async { Ok(None) })),
    //                    Value::Future(Box::pin(async {
    //                        sleep(Duration::from_nanos(42)).await;
    //                        Ok(Some(Value::Bool(true)))
    //                    })),
    //                    Value::Future(Box::pin(async {
    //                        Ok(Some(Value::Future(Box::pin(async { Ok(None) }))))
    //                    })),
    //                    Value::Stream(Box::pin(
    //                        stream::iter([
    //                            Ok(vec![Some(Value::U8(0x42))]),
    //                            Ok(vec![Some(Value::U8(0xff))]),
    //                        ])
    //                        .then(|item| async {
    //                            sleep(Duration::from_nanos(42)).await;
    //                            item
    //                        }),
    //                    )),
    //                ]),
    //            ],
    //        )
    //        .await
    //        .context("failed to invoke `async`")?;
    //
    //        let mut values = zip(params, results);
    //        let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
    //            bail!("future type mismatch")
    //        };
    //        assert!(matches!((p.await, r.await), (Ok(None), Ok(None))));
    //
    //        let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
    //            bail!("future type mismatch")
    //        };
    //        assert!(matches!(
    //            (p.await, r.await),
    //            (Ok(Some(Value::Bool(true))), Ok(Some(Value::Bool(true))))
    //        ));
    //
    //        let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
    //            bail!("future type mismatch")
    //        };
    //        let (Ok(Some(Value::Future(p))), Ok(Some(Value::Future(r)))) = (p.await, r.await) else {
    //            bail!("future type mismatch")
    //        };
    //        assert!(matches!((p.await, r.await), (Ok(None), Ok(None))));
    //
    //        let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
    //            bail!("future type mismatch")
    //        };
    //        let (Ok(Some(Value::Future(p))), Ok(Some(Value::Future(r)))) = (p.await, r.await) else {
    //            bail!("future type mismatch")
    //        };
    //        let (Ok(Some(Value::Future(p))), Ok(Some(Value::Future(r)))) = (p.await, r.await) else {
    //            bail!("future type mismatch")
    //        };
    //        assert!(matches!(
    //            (p.await, r.await),
    //            (Ok(Some(Value::Bool(true))), Ok(Some(Value::Bool(true))))
    //        ));
    //
    //        let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
    //            bail!("future type mismatch")
    //        };
    //        let (Ok(Some(Value::Stream(mut p))), Ok(Some(Value::Stream(mut r)))) = (p.await, r.await)
    //        else {
    //            bail!("stream type mismatch")
    //        };
    //        assert!(matches!(
    //            (
    //                p.try_next().await.unwrap().as_deref().unwrap(),
    //                r.try_next().await.unwrap().as_deref().unwrap(),
    //            ),
    //            ([Some(Value::U8(0x42))], [Some(Value::U8(0x42))])
    //        ));
    //        assert!(matches!(
    //            (
    //                p.try_next().await.unwrap().as_deref().unwrap(),
    //                r.try_next().await.unwrap().as_deref().unwrap(),
    //            ),
    //            ([Some(Value::U8(0xff))], [Some(Value::U8(0xff))])
    //        ));
    //        assert!(matches!(
    //            (p.try_next().await.unwrap(), r.try_next().await.unwrap()),
    //            (None, None)
    //        ));
    //
    //        let (Value::Stream(mut p), Value::Stream(mut r)) = values.next().unwrap() else {
    //            bail!("stream type mismatch")
    //        };
    //        assert!(matches!(
    //            (
    //                p.try_next().await.unwrap().as_deref().unwrap(),
    //                r.try_next().await.unwrap().as_deref().unwrap(),
    //            ),
    //            ([None, None, None, None], [None, None, None, None])
    //        ));
    //        assert!(matches!(
    //            (p.try_next().await.unwrap(), r.try_next().await.unwrap()),
    //            (None, None)
    //        ));
    //
    //        let (Value::Stream(mut p), Value::Stream(mut r)) = values.next().unwrap() else {
    //            bail!("stream type mismatch")
    //        };
    //        assert!(matches!(
    //            (
    //                p.try_next().await.unwrap().as_deref().unwrap(),
    //                r.try_next().await.unwrap().as_deref().unwrap(),
    //            ),
    //            (
    //                [Some(Value::U8(0x42)), Some(Value::U8(0xff))],
    //                [Some(Value::U8(0x42)), Some(Value::U8(0xff))]
    //            )
    //        ));
    //        assert!(matches!(
    //            (p.try_next().await.unwrap(), r.try_next().await.unwrap()),
    //            (None, None)
    //        ));
    //
    //        let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
    //            bail!("future type mismatch")
    //        };
    //        assert!(matches!((p.await, r.await), (Ok(None), Ok(None))));
    //
    //        let (Value::Stream(mut p), Value::Stream(mut r)) = values.next().unwrap() else {
    //            bail!("stream type mismatch")
    //        };
    //        assert!(matches!(
    //            (
    //                p.try_next().await.unwrap().as_deref().unwrap(),
    //                r.try_next().await.unwrap().as_deref().unwrap(),
    //            ),
    //            ([Some(Value::U8(0x42))], [Some(Value::U8(0x42))])
    //        ));
    //        assert!(matches!(
    //            (
    //                p.try_next().await.unwrap().as_deref().unwrap(),
    //                r.try_next().await.unwrap().as_deref().unwrap(),
    //            ),
    //            ([Some(Value::U8(0xff))], [Some(Value::U8(0xff))])
    //        ));
    //        assert!(matches!(
    //            (p.try_next().await.unwrap(), r.try_next().await.unwrap()),
    //            (None, None)
    //        ));
    //
    //        let (Value::Tuple(p), Value::Tuple(r)) = values.next().unwrap() else {
    //            bail!("tuple type mismatch")
    //        };
    //        {
    //            let mut values = zip(p, r);
    //            let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
    //                bail!("future type mismatch")
    //            };
    //            assert!(matches!((p.await, r.await), (Ok(None), Ok(None))));
    //
    //            let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
    //                bail!("future type mismatch")
    //            };
    //            assert!(matches!(
    //                (p.await, r.await),
    //                (Ok(Some(Value::Bool(true))), Ok(Some(Value::Bool(true))))
    //            ));
    //
    //            let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
    //                bail!("future type mismatch")
    //            };
    //            let (Ok(Some(Value::Future(p))), Ok(Some(Value::Future(r)))) = (p.await, r.await) else {
    //                bail!("future type mismatch")
    //            };
    //            assert!(matches!((p.await, r.await), (Ok(None), Ok(None))));
    //
    //            let (Value::Stream(mut p), Value::Stream(mut r)) = values.next().unwrap() else {
    //                bail!("stream type mismatch")
    //            };
    //            assert!(matches!(
    //                (
    //                    p.try_next().await.unwrap().as_deref().unwrap(),
    //                    r.try_next().await.unwrap().as_deref().unwrap(),
    //                ),
    //                ([Some(Value::U8(0x42))], [Some(Value::U8(0x42))])
    //            ));
    //            assert!(matches!(
    //                (
    //                    p.try_next().await.unwrap().as_deref().unwrap(),
    //                    r.try_next().await.unwrap().as_deref().unwrap(),
    //                ),
    //                ([Some(Value::U8(0xff))], [Some(Value::U8(0xff))])
    //            ));
    //            assert!(matches!(
    //                (p.try_next().await.unwrap(), r.try_next().await.unwrap()),
    //                (None, None)
    //            ));
    //        }
    //
    //        let (Value::Record(p), Value::Record(r)) = values.next().unwrap() else {
    //            bail!("record type mismatch")
    //        };
    //        {
    //            let mut values = zip(p, r);
    //            let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
    //                bail!("future type mismatch")
    //            };
    //            assert!(matches!((p.await, r.await), (Ok(None), Ok(None))));
    //
    //            let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
    //                bail!("future type mismatch")
    //            };
    //            assert!(matches!(
    //                (p.await, r.await),
    //                (Ok(Some(Value::Bool(true))), Ok(Some(Value::Bool(true))))
    //            ));
    //
    //            let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
    //                bail!("future type mismatch")
    //            };
    //            let (Ok(Some(Value::Future(p))), Ok(Some(Value::Future(r)))) = (p.await, r.await) else {
    //                bail!("future type mismatch")
    //            };
    //            assert!(matches!((p.await, r.await), (Ok(None), Ok(None))));
    //
    //            let (Value::Stream(mut p), Value::Stream(mut r)) = values.next().unwrap() else {
    //                bail!("stream type mismatch")
    //            };
    //            assert!(matches!(
    //                (
    //                    p.try_next().await.unwrap().as_deref().unwrap(),
    //                    r.try_next().await.unwrap().as_deref().unwrap(),
    //                ),
    //                ([Some(Value::U8(0x42))], [Some(Value::U8(0x42))])
    //            ));
    //            assert!(matches!(
    //                (
    //                    p.try_next().await.unwrap().as_deref().unwrap(),
    //                    r.try_next().await.unwrap().as_deref().unwrap(),
    //                ),
    //                ([Some(Value::U8(0xff))], [Some(Value::U8(0xff))])
    //            ));
    //            assert!(matches!(
    //                (p.try_next().await.unwrap(), r.try_next().await.unwrap()),
    //                (None, None)
    //            ));
    //        }
    //
    //        let unit_invocations = client
    //            .serve_static::<()>("wrpc:wrpc/test-static", "unit_unit")
    //            .await
    //            .context("failed to serve")?;
    //        try_join!(
    //            async {
    //                let AcceptedInvocation {
    //                    params: (),
    //                    result_subject,
    //                    transmitter,
    //                    ..
    //                } = pin!(unit_invocations)
    //                    .try_next()
    //                    .await
    //                    .context("failed to receive invocation")?
    //                    .context("unexpected end of stream")?;
    //                transmitter
    //                    .transmit_static(result_subject, ())
    //                    .await
    //                    .context("failed to transmit response")?;
    //                anyhow::Ok(())
    //            },
    //            async {
    //                let ((), tx) = client
    //                    .invoke_static("wrpc:wrpc/test-static", "unit_unit", ())
    //                    .await
    //                    .context("failed to invoke")?;
    //                tx.await.context("failed to transmit parameters")?;
    //                Ok(())
    //            }
    //        )?;
    //        Ok(())
    //    }).await
}
