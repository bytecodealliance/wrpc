use core::iter::zip;
use core::pin::pin;
use core::str;
use core::time::Duration;

use std::net::Ipv4Addr;
use std::sync::Arc;

use anyhow::{anyhow, bail, ensure, Context};
use bytes::Bytes;
use futures::{stream, FutureExt as _, StreamExt as _, TryStreamExt as _};
use hyper::header::HOST;
use hyper::Uri;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Notify, RwLock};
use tokio::time::sleep;
use tokio::try_join;
use tracing::{info, instrument};
use wrpc::{DynamicFunctionType, ResourceType, Transmitter as _, Type, Value};
use wrpc_interface_blobstore::{ContainerMetadata, ObjectId, ObjectMetadata};
use wrpc_interface_http::{ErrorCode, Method, Request, RequestOptions, Response, Scheme};
use wrpc_transport::{AcceptedInvocation, Client as _, DynamicTuple};

mod common;
use common::{init, start_nats, with_nats};

#[instrument(skip(client, ty, params, results))]
async fn loopback_dynamic(
    client: &impl wrpc::Client,
    name: &str,
    ty: DynamicFunctionType,
    params: Vec<Value>,
    results: Vec<Value>,
) -> anyhow::Result<(Vec<Value>, Vec<Value>)> {
    match ty {
        DynamicFunctionType::Method {
            receiver: _,
            params: _,
            results: _,
        } => todo!("methods not supported yet"),
        DynamicFunctionType::Static {
            params: params_ty,
            results: results_ty,
        } => {
            info!("serve function");
            let invocations = client
                .serve_dynamic("wrpc:wrpc/test-dynamic", name, params_ty)
                .await
                .context("failed to serve static function")?;
            let (params, results) = try_join!(
                async {
                    info!("await invocation");
                    let AcceptedInvocation {
                        params,
                        result_subject,
                        transmitter,
                        ..
                    } = pin!(invocations)
                        .try_next()
                        .await
                        .with_context(|| "unexpected end of invocation stream".to_string())?
                        .with_context(|| "failed to decode parameters".to_string())?;
                    info!("transmit response to invocation");
                    transmitter
                        .transmit_tuple_dynamic(result_subject, results)
                        .await
                        .context("failed to transmit result tuple")?;
                    info!("finished serving invocation");
                    anyhow::Ok(params)
                },
                async {
                    info!("invoke function");
                    let (results, params_tx) = client
                        .invoke_dynamic(
                            "wrpc:wrpc/test-dynamic",
                            name,
                            DynamicTuple(params),
                            &results_ty,
                        )
                        .await
                        .with_context(|| "failed to invoke static function".to_string())?;
                    info!("transmit async parameters");
                    params_tx
                        .await
                        .context("failed to transmit parameter tuple")?;
                    info!("finished invocation");
                    Ok(results)
                },
            )?;
            Ok((params, results))
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rust_bindgen() -> anyhow::Result<()> {
    init().await;

    with_nats(|_, nats_client| async {
        let client = wrpc::transport::nats::Client::new(nats_client, "test-prefix".to_string());
        let client = Arc::new(client);

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let shutdown_rx =
            async move { shutdown_rx.await.expect("shutdown sender dropped") }.shared();
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
                        }

                        world test {
                            export shared;

                            export f: func(x: string) -> u32;
                            export foo: interface {
                                foo: func(x: string);
                            }
                        }"
                });

                #[derive(Clone, Default)]
                struct Component(Arc<RwLock<Option<String>>>);

                impl Handler<Option<async_nats::HeaderMap>> for Component {
                    async fn f(
                        &self,
                        _cx: Option<async_nats::HeaderMap>,
                        x: String,
                    ) -> anyhow::Result<u32> {
                        let stored = self.0.read().await.as_ref().unwrap().to_string();
                        assert_eq!(stored, x);
                        Ok(42)
                    }
                }

                impl exports::wrpc_test::integration::shared::Handler<Option<async_nats::HeaderMap>> for Component {
                    async fn fallible(
                        &self,
                        _cx: Option<async_nats::HeaderMap>,
                    ) -> anyhow::Result<Result<bool, String>> {
                        Ok(Ok(true))
                    }

                    async fn numbers(
                        &self,
                        _cx: Option<async_nats::HeaderMap>,
                    ) -> anyhow::Result<(u8, u16, u32, u64, i8, i16, i32, i64, f32, f64)> {
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
                        _cx: Option<async_nats::HeaderMap>,
                    ) -> anyhow::Result<exports::wrpc_test::integration::shared::Abc> {
                        use exports::wrpc_test::integration::shared::Abc;
                        Ok(Abc::A | Abc::C)
                    }
                }

                impl exports::foo::Handler<Option<async_nats::HeaderMap>> for Component {
                    async fn foo(
                        &self,
                        _cx: Option<async_nats::HeaderMap>,
                        x: String,
                    ) -> anyhow::Result<()> {
                        let old = self.0.write().await.replace(x);
                        assert!(old.is_none());
                        Ok(())
                    }
                }

                serve(client.as_ref(), Component::default(), shutdown_rx.clone())
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

                #[derive(Clone)]
                struct Component(Arc<wrpc_transport_nats::Client>);

                // TODO: Remove the need for this
                sleep(Duration::from_secs(1)).await;

                impl exports::bar::Handler<Option<async_nats::HeaderMap>> for Component {
                    async fn bar(
                        &self,
                        _cx: Option<async_nats::HeaderMap>,
                    ) -> anyhow::Result<String> {
                        use wrpc_test::integration::shared::Abc;

                        foo::foo(self.0.as_ref(), "foo")
                            .await
                            .context("failed to call `wrpc-test:integration/test.foo.foo`")?;
                        let v = f(self.0.as_ref(), "foo")
                            .await
                            .context("failed to call `wrpc-test:integration/test.f`")?;
                        assert_eq!(v, 42);
                        let v = wrpc_test::integration::shared::fallible(self.0.as_ref())
                            .await
                            .context("failed to call `wrpc-test:integration/shared.fallible`")?;
                        assert_eq!(v, Ok(true));
                        let v = wrpc_test::integration::shared::numbers(self.0.as_ref())
                            .await
                            .context("failed to call `wrpc-test:integration/shared.numbers`")?;
                        assert_eq!(v, (
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
                        ));
                        let v = wrpc_test::integration::shared::with_flags(self.0.as_ref())
                            .await
                            .context("failed to call `wrpc-test:integration/shared.with-flags`")?;
                        assert_eq!(v, Abc::A | Abc::C);
                        Ok("bar".to_string())
                    }
                }

                serve(
                    client.as_ref(),
                    Component(Arc::clone(&client)),
                    shutdown_rx.clone(),
                )
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

                let v = bar::bar(client.as_ref())
                    .await
                    .context("failed to call `wrpc-test:integration/test.bar.bar`")?;
                assert_eq!(v, "bar");
                shutdown_tx.send(()).expect("failed to send shutdown");
                Ok(())
            },
        )?;
        Ok(())
    }).await
}

#[tokio::test(flavor = "multi_thread")]
async fn rust_dynamic() -> anyhow::Result<()> {
    init().await;

    with_nats(|_, nats_client| async {
        let client = wrpc::transport::nats::Client::new(nats_client, "test-prefix".to_string());
        let client = Arc::new(client);

        let (params, results) = loopback_dynamic(
            client.as_ref(),
            "unit_unit",
            DynamicFunctionType::Static {
                params: vec![].into(),
                results: vec![].into(),
            },
            vec![],
            vec![],
        )
        .await
        .context("failed to invoke `unit_unit`")?;
        assert!(params.is_empty());
        assert!(results.is_empty());

        let flat_types = vec![
            Type::Bool,
            Type::U8,
            Type::U16,
            Type::U32,
            Type::U64,
            Type::S8,
            Type::S16,
            Type::S32,
            Type::S64,
            Type::F32,
            Type::F64,
            Type::Char,
            Type::String,
            Type::Enum,
            Type::Flags,
        ]
        .into();

        let flat_variant_type = vec![None, Some(Type::Bool)].into();

        let sync_tuple_type = vec![
            Type::Bool,
            Type::U8,
            Type::U16,
            Type::U32,
            Type::U64,
            Type::S8,
            Type::S16,
            Type::S32,
            Type::S64,
            Type::F32,
            Type::F64,
            Type::Char,
            Type::String,
            Type::List(Arc::new(Type::U64)),
            Type::List(Arc::new(Type::Bool)),
            Type::Record(Arc::clone(&flat_types)),
            Type::Tuple(Arc::clone(&flat_types)),
            Type::Variant(Arc::clone(&flat_variant_type)),
            Type::Variant(Arc::clone(&flat_variant_type)),
            Type::Enum,
            Type::Option(Arc::new(Type::Bool)),
            Type::Result {
                ok: Some(Arc::new(Type::Bool)),
                err: Some(Arc::new(Type::Bool)),
            },
            Type::Flags,
        ]
        .into();

        let (params, results) = loopback_dynamic(
            client.as_ref(),
            "sync",
            DynamicFunctionType::Static {
                params: Arc::clone(&sync_tuple_type),
                results: sync_tuple_type,
            },
            vec![
                Value::Bool(true),
                Value::U8(0xfe),
                Value::U16(0xfeff),
                Value::U32(0xfeff_ffff),
                Value::U64(0xfeff_ffff_ffff_ffff),
                Value::S8(0x7e),
                Value::S16(0x7eff),
                Value::S32(0x7eff_ffff),
                Value::S64(0x7eff_ffff_ffff_ffff),
                Value::F32(0.42),
                Value::F64(0.4242),
                Value::Char('a'),
                Value::String("test".into()),
                Value::List(vec![]),
                Value::List(vec![
                    Value::Bool(true),
                    Value::Bool(false),
                    Value::Bool(true),
                ]),
                Value::Record(vec![
                    Value::Bool(true),
                    Value::U8(0xfe),
                    Value::U16(0xfeff),
                    Value::U32(0xfeff_ffff),
                    Value::U64(0xfeff_ffff_ffff_ffff),
                    Value::S8(0x7e),
                    Value::S16(0x7eff),
                    Value::S32(0x7eff_ffff),
                    Value::S64(0x7eff_ffff_ffff_ffff),
                    Value::F32(0.42),
                    Value::F64(0.4242),
                    Value::Char('a'),
                    Value::String("test".into()),
                    Value::Enum(0xfeff),
                    Value::Flags(0xdead_beef),
                ]),
                Value::Tuple(vec![
                    Value::Bool(true),
                    Value::U8(0xfe),
                    Value::U16(0xfeff),
                    Value::U32(0xfeff_ffff),
                    Value::U64(0xfeff_ffff_ffff_ffff),
                    Value::S8(0x7e),
                    Value::S16(0x7eff),
                    Value::S32(0x7eff_ffff),
                    Value::S64(0x7eff_ffff_ffff_ffff),
                    Value::F32(0.42),
                    Value::F64(0.4242),
                    Value::Char('a'),
                    Value::String("test".into()),
                    Value::Enum(0xfeff),
                    Value::Flags(0xdead_beef),
                ]),
                Value::Variant {
                    discriminant: 0,
                    nested: None,
                },
                Value::Variant {
                    discriminant: 1,
                    nested: Some(Box::new(Value::Bool(true))),
                },
                Value::Enum(0xfeff),
                Value::Option(Some(Box::new(Value::Bool(true)))),
                Value::Result(Ok(Some(Box::new(Value::Bool(true))))),
                Value::Flags(0xdead_beef),
            ],
            vec![
                Value::Bool(true),
                Value::U8(0xfe),
                Value::U16(0xfeff),
                Value::U32(0xfeff_ffff),
                Value::U64(0xfeff_ffff_ffff_ffff),
                Value::S8(0x7e),
                Value::S16(0x7eff),
                Value::S32(0x7eff_ffff),
                Value::S64(0x7eff_ffff_ffff_ffff),
                Value::F32(0.42),
                Value::F64(0.4242),
                Value::Char('a'),
                Value::String("test".into()),
                Value::List(vec![]),
                Value::List(vec![
                    Value::Bool(true),
                    Value::Bool(false),
                    Value::Bool(true),
                ]),
                Value::Record(vec![
                    Value::Bool(true),
                    Value::U8(0xfe),
                    Value::U16(0xfeff),
                    Value::U32(0xfeff_ffff),
                    Value::U64(0xfeff_ffff_ffff_ffff),
                    Value::S8(0x7e),
                    Value::S16(0x7eff),
                    Value::S32(0x7eff_ffff),
                    Value::S64(0x7eff_ffff_ffff_ffff),
                    Value::F32(0.42),
                    Value::F64(0.4242),
                    Value::Char('a'),
                    Value::String("test".into()),
                    Value::Enum(0xfeff),
                    Value::Flags(0xdead_beef),
                ]),
                Value::Tuple(vec![
                    Value::Bool(true),
                    Value::U8(0xfe),
                    Value::U16(0xfeff),
                    Value::U32(0xfeff_ffff),
                    Value::U64(0xfeff_ffff_ffff_ffff),
                    Value::S8(0x7e),
                    Value::S16(0x7eff),
                    Value::S32(0x7eff_ffff),
                    Value::S64(0x7eff_ffff_ffff_ffff),
                    Value::F32(0.42),
                    Value::F64(0.4242),
                    Value::Char('a'),
                    Value::String("test".into()),
                    Value::Enum(0xfeff),
                    Value::Flags(0xdead_beef),
                ]),
                Value::Variant {
                    discriminant: 0,
                    nested: None,
                },
                Value::Variant {
                    discriminant: 1,
                    nested: Some(Box::new(Value::Bool(true))),
                },
                Value::Enum(0xfeff),
                Value::Option(Some(Box::new(Value::Bool(true)))),
                Value::Result(Ok(Some(Box::new(Value::Bool(true))))),
                Value::Flags(0xdead_beef),
            ],
        )
        .await
        .context("failed to invoke `sync`")?;
        let mut values = zip(params, results);
        assert!(matches!(
            values.next().unwrap(),
            (Value::Bool(true), Value::Bool(true))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::U8(0xfe), Value::U8(0xfe))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::U16(0xfeff), Value::U16(0xfeff))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::U32(0xfeff_ffff), Value::U32(0xfeff_ffff))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (
                Value::U64(0xfeff_ffff_ffff_ffff),
                Value::U64(0xfeff_ffff_ffff_ffff)
            )
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::S8(0x7e), Value::S8(0x7e))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::S16(0x7eff), Value::S16(0x7eff))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::S32(0x7eff_ffff), Value::S32(0x7eff_ffff))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (
                Value::S64(0x7eff_ffff_ffff_ffff),
                Value::S64(0x7eff_ffff_ffff_ffff)
            )
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::F32(p), Value::F32(r)) if p == 0.42 && r == 0.42
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::F64(p), Value::F64(r)) if p == 0.4242 && r == 0.4242
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::Char('a'), Value::Char('a'))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::String(p), Value::String(r)) if p == "test" && r == "test"
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::List(p), Value::List(r)) if p.is_empty() && r.is_empty()
        ));
        let (Value::List(p), Value::List(r)) = values.next().unwrap() else {
            bail!("list type mismatch")
        };
        {
            let mut values = zip(p, r);
            assert!(matches!(
                values.next().unwrap(),
                (Value::Bool(true), Value::Bool(true))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::Bool(false), Value::Bool(false))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::Bool(true), Value::Bool(true))
            ));
        }
        let (Value::Record(p), Value::Record(r)) = values.next().unwrap() else {
            bail!("record type mismatch")
        };
        {
            let mut values = zip(p, r);
            assert!(matches!(
                values.next().unwrap(),
                (Value::Bool(true), Value::Bool(true))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::U8(0xfe), Value::U8(0xfe))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::U16(0xfeff), Value::U16(0xfeff))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::U32(0xfeff_ffff), Value::U32(0xfeff_ffff))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (
                    Value::U64(0xfeff_ffff_ffff_ffff),
                    Value::U64(0xfeff_ffff_ffff_ffff)
                )
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::S8(0x7e), Value::S8(0x7e))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::S16(0x7eff), Value::S16(0x7eff))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::S32(0x7eff_ffff), Value::S32(0x7eff_ffff))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (
                    Value::S64(0x7eff_ffff_ffff_ffff),
                    Value::S64(0x7eff_ffff_ffff_ffff)
                )
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::F32(p), Value::F32(r)) if p == 0.42 && r == 0.42
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::F64(p), Value::F64(r)) if p == 0.4242 && r == 0.4242
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::Char('a'), Value::Char('a'))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::String(p), Value::String(r)) if p == "test" && r == "test"
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::Enum(0xfeff), Value::Enum(0xfeff))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::Flags(0xdead_beef), Value::Flags(0xdead_beef))
            ));
        }
        let (Value::Tuple(p), Value::Tuple(r)) = values.next().unwrap() else {
            bail!("tuple type mismatch")
        };
        {
            let mut values = zip(p, r);
            assert!(matches!(
                values.next().unwrap(),
                (Value::Bool(true), Value::Bool(true))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::U8(0xfe), Value::U8(0xfe))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::U16(0xfeff), Value::U16(0xfeff))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::U32(0xfeff_ffff), Value::U32(0xfeff_ffff))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (
                    Value::U64(0xfeff_ffff_ffff_ffff),
                    Value::U64(0xfeff_ffff_ffff_ffff)
                )
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::S8(0x7e), Value::S8(0x7e))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::S16(0x7eff), Value::S16(0x7eff))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::S32(0x7eff_ffff), Value::S32(0x7eff_ffff))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (
                    Value::S64(0x7eff_ffff_ffff_ffff),
                    Value::S64(0x7eff_ffff_ffff_ffff)
                )
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::F32(p), Value::F32(r)) if p == 0.42 && r == 0.42
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::F64(p), Value::F64(r)) if p == 0.4242 && r == 0.4242
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::Char('a'), Value::Char('a'))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::String(p), Value::String(r)) if p == "test" && r == "test"
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::Enum(0xfeff), Value::Enum(0xfeff))
            ));
            assert!(matches!(
                values.next().unwrap(),
                (Value::Flags(0xdead_beef), Value::Flags(0xdead_beef))
            ));
        }
        assert!(matches!(
            values.next().unwrap(),
            (
                Value::Variant {
                    discriminant: 0,
                    nested: None,
                },
                Value::Variant {
                    discriminant: 0,
                    nested: None,
                }
            )
        ));
        assert!(matches!(
            values.next().unwrap(),
            (
                Value::Variant {
                    discriminant: 1,
                    nested: p,
                },
                Value::Variant {
                    discriminant: 1,
                    nested: r,
                }
            ) if matches!((p.as_deref(), r.as_deref()), (Some(Value::Bool(true)), Some(Value::Bool(true))))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::Enum(0xfeff), Value::Enum(0xfeff))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::Option(p), Value::Option(r)) if matches!((p.as_deref(), r.as_deref()), (Some(Value::Bool(true)), Some(Value::Bool(true))))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::Result(Ok(p)), Value::Result(Ok(r))) if matches!((p.as_deref(), r.as_deref()), (Some(Value::Bool(true)), Some(Value::Bool(true))))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::Flags(0xdead_beef), Value::Flags(0xdead_beef))
        ));

        let async_tuple_type = vec![
            Type::Future(None),
            Type::Future(Some(Arc::new(Type::Bool))),
            Type::Future(Some(Arc::new(Type::Future(None)))),
            Type::Future(Some(Arc::new(Type::Future(Some(Arc::new(Type::Future(
                Some(Arc::new(Type::Bool)),
            ))))))),
            Type::Future(Some(Arc::new(Type::Stream(Some(Arc::new(Type::U8)))))),
            Type::Stream(None),
            Type::Stream(Some(Arc::new(Type::U8))),
            Type::Resource(ResourceType::Pollable),
            Type::Resource(ResourceType::InputStream),
            Type::Tuple(
                vec![
                    Type::Future(None),
                    Type::Future(Some(Arc::new(Type::Bool))),
                    Type::Future(Some(Arc::new(Type::Future(None)))),
                    Type::Resource(ResourceType::InputStream),
                ]
                .into(),
            ),
            Type::Record(
                vec![
                    Type::Future(None),
                    Type::Future(Some(Arc::new(Type::Bool))),
                    Type::Future(Some(Arc::new(Type::Future(None)))),
                    Type::Resource(ResourceType::InputStream),
                ]
                .into(),
            ),
        ]
        .into();

        let (params, results) = loopback_dynamic(
            client.as_ref(),
            "async",
            DynamicFunctionType::Static {
                params: Arc::clone(&async_tuple_type),
                results: async_tuple_type,
            },
            vec![
                Value::Future(Box::pin(async { Ok(None) })),
                Value::Future(Box::pin(async {
                    sleep(Duration::from_nanos(42)).await;
                    Ok(Some(Value::Bool(true)))
                })),
                Value::Future(Box::pin(async {
                    Ok(Some(Value::Future(Box::pin(async { Ok(None) }))))
                })),
                Value::Future(Box::pin(async {
                    sleep(Duration::from_nanos(42)).await;
                    Ok(Some(Value::Future(Box::pin(async {
                        sleep(Duration::from_nanos(42)).await;
                        Ok(Some(Value::Future(Box::pin(async {
                            sleep(Duration::from_nanos(42)).await;
                            Ok(Some(Value::Bool(true)))
                        }))))
                    }))))
                })),
                Value::Future(Box::pin(async {
                    sleep(Duration::from_nanos(42)).await;
                    Ok(Some(Value::Stream(Box::pin(stream::iter([
                        Ok(vec![Some(Value::U8(0x42))]),
                        Ok(vec![Some(Value::U8(0xff))]),
                    ])))))
                })),
                Value::Stream(Box::pin(stream::iter([Ok(vec![None, None, None, None])]))),
                Value::Stream(Box::pin(stream::iter([Ok(vec![
                    Some(Value::U8(0x42)),
                    Some(Value::U8(0xff)),
                ])]))),
                Value::Future(Box::pin(async { Ok(None) })),
                Value::Stream(Box::pin(
                    stream::iter([
                        Ok(vec![Some(Value::U8(0x42))]),
                        Ok(vec![Some(Value::U8(0xff))]),
                    ])
                    .then(|item| async {
                        sleep(Duration::from_nanos(42)).await;
                        item
                    }),
                )),
                Value::Tuple(vec![
                    Value::Future(Box::pin(async { Ok(None) })),
                    Value::Future(Box::pin(async {
                        sleep(Duration::from_nanos(42)).await;
                        Ok(Some(Value::Bool(true)))
                    })),
                    Value::Future(Box::pin(async {
                        Ok(Some(Value::Future(Box::pin(async { Ok(None) }))))
                    })),
                    Value::Stream(Box::pin(
                        stream::iter([
                            Ok(vec![Some(Value::U8(0x42))]),
                            Ok(vec![Some(Value::U8(0xff))]),
                        ])
                        .then(|item| async {
                            sleep(Duration::from_nanos(42)).await;
                            item
                        }),
                    )),
                ]),
                Value::Record(vec![
                    Value::Future(Box::pin(async { Ok(None) })),
                    Value::Future(Box::pin(async {
                        sleep(Duration::from_nanos(42)).await;
                        Ok(Some(Value::Bool(true)))
                    })),
                    Value::Future(Box::pin(async {
                        Ok(Some(Value::Future(Box::pin(async { Ok(None) }))))
                    })),
                    Value::Stream(Box::pin(
                        stream::iter([
                            Ok(vec![Some(Value::U8(0x42))]),
                            Ok(vec![Some(Value::U8(0xff))]),
                        ])
                        .then(|item| async {
                            sleep(Duration::from_nanos(42)).await;
                            item
                        }),
                    )),
                ]),
            ],
            vec![
                Value::Future(Box::pin(async { Ok(None) })),
                Value::Future(Box::pin(async {
                    sleep(Duration::from_nanos(42)).await;
                    Ok(Some(Value::Bool(true)))
                })),
                Value::Future(Box::pin(async {
                    Ok(Some(Value::Future(Box::pin(async { Ok(None) }))))
                })),
                Value::Future(Box::pin(async {
                    sleep(Duration::from_nanos(42)).await;
                    Ok(Some(Value::Future(Box::pin(async {
                        sleep(Duration::from_nanos(42)).await;
                        Ok(Some(Value::Future(Box::pin(async {
                            sleep(Duration::from_nanos(42)).await;
                            Ok(Some(Value::Bool(true)))
                        }))))
                    }))))
                })),
                Value::Future(Box::pin(async {
                    sleep(Duration::from_nanos(42)).await;
                    Ok(Some(Value::Stream(Box::pin(stream::iter([
                        Ok(vec![Some(Value::U8(0x42))]),
                        Ok(vec![Some(Value::U8(0xff))]),
                    ])))))
                })),
                Value::Stream(Box::pin(stream::iter([Ok(vec![None, None, None, None])]))),
                Value::Stream(Box::pin(stream::iter([Ok(vec![
                    Some(Value::U8(0x42)),
                    Some(Value::U8(0xff)),
                ])]))),
                Value::Future(Box::pin(async { Ok(None) })),
                Value::Stream(Box::pin(
                    stream::iter([
                        Ok(vec![Some(Value::U8(0x42))]),
                        Ok(vec![Some(Value::U8(0xff))]),
                    ])
                    .then(|item| async {
                        sleep(Duration::from_nanos(42)).await;
                        item
                    }),
                )),
                Value::Tuple(vec![
                    Value::Future(Box::pin(async { Ok(None) })),
                    Value::Future(Box::pin(async {
                        sleep(Duration::from_nanos(42)).await;
                        Ok(Some(Value::Bool(true)))
                    })),
                    Value::Future(Box::pin(async {
                        Ok(Some(Value::Future(Box::pin(async { Ok(None) }))))
                    })),
                    Value::Stream(Box::pin(
                        stream::iter([
                            Ok(vec![Some(Value::U8(0x42))]),
                            Ok(vec![Some(Value::U8(0xff))]),
                        ])
                        .then(|item| async {
                            sleep(Duration::from_nanos(42)).await;
                            item
                        }),
                    )),
                ]),
                Value::Record(vec![
                    Value::Future(Box::pin(async { Ok(None) })),
                    Value::Future(Box::pin(async {
                        sleep(Duration::from_nanos(42)).await;
                        Ok(Some(Value::Bool(true)))
                    })),
                    Value::Future(Box::pin(async {
                        Ok(Some(Value::Future(Box::pin(async { Ok(None) }))))
                    })),
                    Value::Stream(Box::pin(
                        stream::iter([
                            Ok(vec![Some(Value::U8(0x42))]),
                            Ok(vec![Some(Value::U8(0xff))]),
                        ])
                        .then(|item| async {
                            sleep(Duration::from_nanos(42)).await;
                            item
                        }),
                    )),
                ]),
            ],
        )
        .await
        .context("failed to invoke `async`")?;

        let mut values = zip(params, results);
        let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
            bail!("future type mismatch")
        };
        assert!(matches!((p.await, r.await), (Ok(None), Ok(None))));

        let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
            bail!("future type mismatch")
        };
        assert!(matches!(
            (p.await, r.await),
            (Ok(Some(Value::Bool(true))), Ok(Some(Value::Bool(true))))
        ));

        let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
            bail!("future type mismatch")
        };
        let (Ok(Some(Value::Future(p))), Ok(Some(Value::Future(r)))) = (p.await, r.await) else {
            bail!("future type mismatch")
        };
        assert!(matches!((p.await, r.await), (Ok(None), Ok(None))));

        let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
            bail!("future type mismatch")
        };
        let (Ok(Some(Value::Future(p))), Ok(Some(Value::Future(r)))) = (p.await, r.await) else {
            bail!("future type mismatch")
        };
        let (Ok(Some(Value::Future(p))), Ok(Some(Value::Future(r)))) = (p.await, r.await) else {
            bail!("future type mismatch")
        };
        assert!(matches!(
            (p.await, r.await),
            (Ok(Some(Value::Bool(true))), Ok(Some(Value::Bool(true))))
        ));

        let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
            bail!("future type mismatch")
        };
        let (Ok(Some(Value::Stream(mut p))), Ok(Some(Value::Stream(mut r)))) = (p.await, r.await)
        else {
            bail!("stream type mismatch")
        };
        assert!(matches!(
            (
                p.try_next().await.unwrap().as_deref().unwrap(),
                r.try_next().await.unwrap().as_deref().unwrap(),
            ),
            ([Some(Value::U8(0x42))], [Some(Value::U8(0x42))])
        ));
        assert!(matches!(
            (
                p.try_next().await.unwrap().as_deref().unwrap(),
                r.try_next().await.unwrap().as_deref().unwrap(),
            ),
            ([Some(Value::U8(0xff))], [Some(Value::U8(0xff))])
        ));
        assert!(matches!(
            (p.try_next().await.unwrap(), r.try_next().await.unwrap()),
            (None, None)
        ));

        let (Value::Stream(mut p), Value::Stream(mut r)) = values.next().unwrap() else {
            bail!("stream type mismatch")
        };
        assert!(matches!(
            (
                p.try_next().await.unwrap().as_deref().unwrap(),
                r.try_next().await.unwrap().as_deref().unwrap(),
            ),
            ([None, None, None, None], [None, None, None, None])
        ));
        assert!(matches!(
            (p.try_next().await.unwrap(), r.try_next().await.unwrap()),
            (None, None)
        ));

        let (Value::Stream(mut p), Value::Stream(mut r)) = values.next().unwrap() else {
            bail!("stream type mismatch")
        };
        assert!(matches!(
            (
                p.try_next().await.unwrap().as_deref().unwrap(),
                r.try_next().await.unwrap().as_deref().unwrap(),
            ),
            (
                [Some(Value::U8(0x42)), Some(Value::U8(0xff))],
                [Some(Value::U8(0x42)), Some(Value::U8(0xff))]
            )
        ));
        assert!(matches!(
            (p.try_next().await.unwrap(), r.try_next().await.unwrap()),
            (None, None)
        ));

        let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
            bail!("future type mismatch")
        };
        assert!(matches!((p.await, r.await), (Ok(None), Ok(None))));

        let (Value::Stream(mut p), Value::Stream(mut r)) = values.next().unwrap() else {
            bail!("stream type mismatch")
        };
        assert!(matches!(
            (
                p.try_next().await.unwrap().as_deref().unwrap(),
                r.try_next().await.unwrap().as_deref().unwrap(),
            ),
            ([Some(Value::U8(0x42))], [Some(Value::U8(0x42))])
        ));
        assert!(matches!(
            (
                p.try_next().await.unwrap().as_deref().unwrap(),
                r.try_next().await.unwrap().as_deref().unwrap(),
            ),
            ([Some(Value::U8(0xff))], [Some(Value::U8(0xff))])
        ));
        assert!(matches!(
            (p.try_next().await.unwrap(), r.try_next().await.unwrap()),
            (None, None)
        ));

        let (Value::Tuple(p), Value::Tuple(r)) = values.next().unwrap() else {
            bail!("tuple type mismatch")
        };
        {
            let mut values = zip(p, r);
            let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
                bail!("future type mismatch")
            };
            assert!(matches!((p.await, r.await), (Ok(None), Ok(None))));

            let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
                bail!("future type mismatch")
            };
            assert!(matches!(
                (p.await, r.await),
                (Ok(Some(Value::Bool(true))), Ok(Some(Value::Bool(true))))
            ));

            let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
                bail!("future type mismatch")
            };
            let (Ok(Some(Value::Future(p))), Ok(Some(Value::Future(r)))) = (p.await, r.await) else {
                bail!("future type mismatch")
            };
            assert!(matches!((p.await, r.await), (Ok(None), Ok(None))));

            let (Value::Stream(mut p), Value::Stream(mut r)) = values.next().unwrap() else {
                bail!("stream type mismatch")
            };
            assert!(matches!(
                (
                    p.try_next().await.unwrap().as_deref().unwrap(),
                    r.try_next().await.unwrap().as_deref().unwrap(),
                ),
                ([Some(Value::U8(0x42))], [Some(Value::U8(0x42))])
            ));
            assert!(matches!(
                (
                    p.try_next().await.unwrap().as_deref().unwrap(),
                    r.try_next().await.unwrap().as_deref().unwrap(),
                ),
                ([Some(Value::U8(0xff))], [Some(Value::U8(0xff))])
            ));
            assert!(matches!(
                (p.try_next().await.unwrap(), r.try_next().await.unwrap()),
                (None, None)
            ));
        }

        let (Value::Record(p), Value::Record(r)) = values.next().unwrap() else {
            bail!("record type mismatch")
        };
        {
            let mut values = zip(p, r);
            let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
                bail!("future type mismatch")
            };
            assert!(matches!((p.await, r.await), (Ok(None), Ok(None))));

            let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
                bail!("future type mismatch")
            };
            assert!(matches!(
                (p.await, r.await),
                (Ok(Some(Value::Bool(true))), Ok(Some(Value::Bool(true))))
            ));

            let (Value::Future(p), Value::Future(r)) = values.next().unwrap() else {
                bail!("future type mismatch")
            };
            let (Ok(Some(Value::Future(p))), Ok(Some(Value::Future(r)))) = (p.await, r.await) else {
                bail!("future type mismatch")
            };
            assert!(matches!((p.await, r.await), (Ok(None), Ok(None))));

            let (Value::Stream(mut p), Value::Stream(mut r)) = values.next().unwrap() else {
                bail!("stream type mismatch")
            };
            assert!(matches!(
                (
                    p.try_next().await.unwrap().as_deref().unwrap(),
                    r.try_next().await.unwrap().as_deref().unwrap(),
                ),
                ([Some(Value::U8(0x42))], [Some(Value::U8(0x42))])
            ));
            assert!(matches!(
                (
                    p.try_next().await.unwrap().as_deref().unwrap(),
                    r.try_next().await.unwrap().as_deref().unwrap(),
                ),
                ([Some(Value::U8(0xff))], [Some(Value::U8(0xff))])
            ));
            assert!(matches!(
                (p.try_next().await.unwrap(), r.try_next().await.unwrap()),
                (None, None)
            ));
        }

        let unit_invocations = client
            .serve_static::<()>("wrpc:wrpc/test-static", "unit_unit")
            .await
            .context("failed to serve")?;
        try_join!(
            async {
                let AcceptedInvocation {
                    params: (),
                    result_subject,
                    transmitter,
                    ..
                } = pin!(unit_invocations)
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                transmitter
                    .transmit_static(result_subject, ())
                    .await
                    .context("failed to transmit response")?;
                anyhow::Ok(())
            },
            async {
                let ((), tx) = client
                    .invoke_static("wrpc:wrpc/test-static", "unit_unit", ())
                    .await
                    .context("failed to invoke")?;
                tx.await.context("failed to transmit parameters")?;
                Ok(())
            }
        )?;
        Ok(())
    }).await
}

#[tokio::test(flavor = "multi_thread")]
async fn rust_keyvalue() -> anyhow::Result<()> {
    init().await;

    with_nats(|_, nats_client| async {
        let client = wrpc::transport::nats::Client::new(nats_client, "test-prefix".to_string());

        let shutdown = Notify::new();
        let started = Notify::new();

        try_join!(
            async {
                mod bindings {
                    wit_bindgen_wrpc::generate!("keyvalue-server");
                }

                #[derive(Clone)]
                struct Handler;
                use bindings::exports::wrpc::keyvalue;
                type Result<T, E = keyvalue::store::Error> = core::result::Result<T, E>;

                impl<Ctx: Send> keyvalue::store::Handler<Ctx> for Handler {
                    async fn delete(
                        &self,
                        _cx: Ctx,
                        bucket: String,
                        key: String,
                    ) -> anyhow::Result<Result<()>> {
                        assert_eq!(bucket, "bucket");
                        assert_eq!(key, "key");
                        Ok(Ok(()))
                    }

                    async fn exists(
                        &self,
                        _cx: Ctx,
                        bucket: String,
                        key: String,
                    ) -> anyhow::Result<Result<bool>> {
                        assert_eq!(bucket, "bucket");
                        assert_eq!(key, "key");
                        Ok(Ok(true))
                    }

                    async fn get(
                        &self,
                        _cx: Ctx,
                        bucket: String,
                        key: String,
                    ) -> anyhow::Result<Result<Option<Vec<u8>>>> {
                        assert_eq!(bucket, "bucket");
                        assert_eq!(key, "key");
                        Ok(Ok(Some(vec![0x42, 0xff])))
                    }

                    async fn set(
                        &self,
                        _cx: Ctx,
                        bucket: String,
                        key: String,
                        value: Vec<u8>,
                    ) -> anyhow::Result<Result<()>> {
                        assert_eq!(bucket, "bucket");
                        assert_eq!(key, "key");
                        assert_eq!(value, b"test");
                        Ok(Ok(()))
                    }

                    async fn list_keys(
                        &self,
                        _cx: Ctx,
                        bucket: String,
                        cursor: Option<u64>,
                    ) -> anyhow::Result<Result<keyvalue::store::KeyResponse>> {
                        assert_eq!(bucket, "bucket");
                        assert_eq!(cursor, Some(42));
                        Ok(Ok(keyvalue::store::KeyResponse {
                            cursor: None,
                            keys: vec!["key".to_string()],
                        }))
                    }
                }

                impl<Ctx: Send> keyvalue::atomics::Handler<Ctx> for Handler {
                    async fn increment(
                        &self,
                        _cx: Ctx,
                        bucket: String,
                        key: String,
                        delta: u64,
                    ) -> anyhow::Result<Result<u64, keyvalue::store::Error>> {
                        assert_eq!(bucket, "bucket");
                        assert_eq!(key, "key");
                        assert_eq!(delta, 42);
                        Ok(Ok(4242))
                    }
                }

                let fut = bindings::serve(&client, Handler, shutdown.notified());

                started.notify_one();

                fut.await.context("failed to serve world")
            },
            async {
                mod bindings {
                    wit_bindgen_wrpc::generate!({
                        world: "keyvalue-client",
                        additional_derives: [Eq, PartialEq],
                    });
                }
                use bindings::wrpc::keyvalue;

                started.notified().await;

                try_join!(
                    async {
                        keyvalue::store::delete(&client, "bucket", "key")
                            .await
                            .context("failed to call `delete`")?
                            .context("`delete` call failed")
                    },
                    async {
                        let v = keyvalue::store::exists(&client, "bucket", "key")
                            .await
                            .context("failed to call `exists`")?
                            .context("`exists` call failed")?;
                        ensure!(v, "`exists` should have returned `true`");
                        Ok(())
                    },
                    async {
                        let v = keyvalue::store::get(&client, "bucket", "key")
                            .await
                            .context("failed to call `get`")?
                            .context("`get` call failed")?;
                        ensure!(
                            v.as_deref() == Some(&[0x42, 0xff]),
                            "`get` should have returned `Some([0x42, 0xff])`, got `{v:?}`"
                        );
                        Ok(())
                    },
                    async {
                        keyvalue::store::set(&client, "bucket", "key", b"test")
                            .await
                            .context("failed to call `set`")?
                            .context("`set` call failed")
                    },
                    async {
                        let v = keyvalue::store::list_keys(&client, "bucket", Some(42))
                            .await
                            .context("failed to call `list-keys`")?
                            .context("`list-keys` call failed")?;
                        ensure!(
                            v == keyvalue::store::KeyResponse {
                                cursor: None,
                                keys: vec!["key".to_string()]
                            },
                            r#"`list-keys` should have returned `{{None, ["key"]}}`, got `{v:?}`"#
                        );
                        Ok(())
                    },
                )?;
                shutdown.notify_one();
                Ok(())
            }
        )?;
        Ok(())
    })
    .await
}

// TODO: Split this test into function per package
#[tokio::test(flavor = "multi_thread")]
async fn rust_interfaces() -> anyhow::Result<()> {
    init().await;

    let (_port, nats_client, nats_server, stop_tx) = start_nats().await?;

    let client = wrpc::transport::nats::Client::new(nats_client, "test-prefix".to_string());
    let client = Arc::new(client);

    {
        use wrpc_interface_http::IncomingHandler;

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .context("failed to start TCP listener")?;
        let addr = listener
            .local_addr()
            .context("failed to query listener local address")?;

        let invocations = client.serve_handle().await.context("failed to serve")?;
        let mut invocations = pin!(invocations);
        try_join!(
            async {
                let AcceptedInvocation {
                    params:
                        Request {
                            mut body,
                            trailers,
                            method,
                            path_with_query,
                            scheme,
                            authority,
                            headers,
                        },
                    result_subject,
                    transmitter,
                    ..
                } = invocations
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(method, Method::Post);
                assert_eq!(path_with_query.as_deref(), Some("path_with_query"));
                assert_eq!(scheme, Some(Scheme::HTTPS));
                assert_eq!(authority.as_deref(), Some("authority"));
                assert_eq!(
                    headers,
                    vec![("user-agent".into(), vec!["wrpc/0.1.0".into()])],
                );
                try_join!(
                    async {
                        info!("transmit response");
                        transmitter
                            .transmit_static(
                                result_subject,
                                Ok::<_, ErrorCode>(Response {
                                    body: stream::empty(),
                                    trailers: async { None },
                                    status: 400,
                                    headers: Vec::default(),
                                }),
                            )
                            .await
                            .context("failed to transmit response")?;
                        info!("response transmitted");
                        anyhow::Ok(())
                    },
                    async {
                        let item = body
                            .try_next()
                            .await
                            .context("failed to receive body item")?
                            .context("unexpected end of body stream")?;
                        assert_eq!(str::from_utf8(&item).unwrap(), "element");
                        info!("await request body end");
                        let item = body
                            .try_next()
                            .await
                            .context("failed to receive end item")?;
                        assert_eq!(item, None);
                        info!("request body verified");
                        Ok(())
                    },
                    async {
                        info!("await request trailers");
                        let trailers = trailers.await.context("failed to receive trailers")?;
                        assert_eq!(
                            trailers,
                            Some(vec![("trailer".into(), vec!["test".into()])])
                        );
                        info!("request trailers verified");
                        Ok(())
                    }
                )?;
                let AcceptedInvocation {
                    params:
                        Request {
                            mut body,
                            trailers,
                            method,
                            path_with_query,
                            scheme,
                            authority,
                            mut headers,
                        },
                    result_subject,
                    transmitter,
                    ..
                } = invocations
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(method, Method::Get);
                assert_eq!(path_with_query.as_deref(), Some("/reqwest"));
                assert_eq!(scheme, Some(Scheme::HTTP));
                assert_eq!(authority, Some(format!("localhost:{}", addr.port())));
                headers.sort();
                assert_eq!(
                    headers,
                    vec![
                        ("accept".into(), vec!["*/*".into()]),
                        (
                            "host".into(),
                            vec![format!("localhost:{}", addr.port()).into()]
                        )
                    ],
                );
                try_join!(
                    async {
                        info!("transmit response");
                        transmitter
                            .transmit_static(
                                result_subject,
                                Ok::<_, ErrorCode>(Response {
                                    body: stream::empty(),
                                    trailers: async { None },
                                    status: 400,
                                    headers: Vec::default(),
                                }),
                            )
                            .await
                            .context("failed to transmit response")?;
                        info!("response transmitted");
                        anyhow::Ok(())
                    },
                    async {
                        let item = body
                            .try_next()
                            .await
                            .context("failed to receive body item")?;
                        assert_eq!(item, None);
                        info!("request body verified");
                        Ok(())
                    },
                    async {
                        info!("await request trailers");
                        let trailers = trailers.await.context("failed to receive trailers")?;
                        assert_eq!(trailers, None);
                        info!("request trailers verified");
                        Ok(())
                    }
                )?;
                anyhow::Ok(())
            },
            async {
                let client = Arc::clone(&client);
                info!("invoke function");
                let (res, tx) = client
                    .invoke_handle(Request {
                        body: stream::iter([("element".into())]),
                        trailers: async { Some(vec![("trailer".into(), vec!["test".into()])]) },
                        method: Method::Post,
                        path_with_query: Some("path_with_query".to_string()),
                        scheme: Some(Scheme::HTTPS),
                        authority: Some("authority".to_string()),
                        headers: vec![("user-agent".into(), vec!["wrpc/0.1.0".into()])],
                    })
                    .await
                    .context("failed to invoke")?;
                let Response {
                    mut body,
                    trailers,
                    status,
                    headers,
                } = res.expect("invocation failed");
                assert_eq!(status, 400);
                assert_eq!(headers, Vec::default());
                try_join!(
                    async {
                        info!("transmit async parameters");
                        tx.await.context("failed to transmit parameters")?;
                        info!("async parameters transmitted");
                        anyhow::Ok(())
                    },
                    async {
                        info!("await response body end");
                        let item = body
                            .try_next()
                            .await
                            .context("failed to receive end item")?;
                        assert_eq!(item, None);
                        info!("response body verified");
                        Ok(())
                    },
                    async {
                        info!("await response trailers");
                        let trailers = trailers.await.context("failed to receive trailers")?;
                        assert_eq!(trailers, None);
                        info!("response trailers verified");
                        Ok(())
                    }
                )?;

                try_join!(
                    async {
                        info!("await connection");
                        let (stream, addr) = listener
                            .accept()
                            .await
                            .context("failed to accept connection")?;
                        info!("accepted connection from {addr}");
                        hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                            .serve_connection(
                                TokioIo::new(stream),
                                hyper::service::service_fn(
                                    move |mut request: hyper::Request<hyper::body::Incoming>| {
                                        let host = request.headers().get(HOST).expect("`host` header missing");
                                        let host = host.to_str().expect("`host` header value is not a valid string");
                                        let path_and_query = request.uri().path_and_query().expect("`path_and_query` missing");
                                        let uri = Uri::builder()
                                            .scheme("http")
                                            .authority(host)
                                            .path_and_query(path_and_query.clone())
                                            .build()
                                            .expect("failed to build a URI");
                                        *request.uri_mut() = uri;
                                        let client = Arc::clone(&client);
                                        async move {
                                            info!(?request, "invoke `handle`");
                                            let (response, tx, errors) =
                                                client.invoke_handle_hyper(request).await.context(
                                                    "failed to invoke `wrpc:http/incoming-handler.handle`",
                                                )?;
                                            info!("await parameter transmit");
                                            tx.await.context("failed to transmit parameters")?;
                                            info!("await error collect");
                                            let errors: Vec<_> = errors.collect().await;
                                            assert!(errors.is_empty());
                                            info!("request served");
                                            response
                                        }
                                    },
                                ),
                            )
                            .await
                            .map_err(|err| anyhow!(err).context("failed to serve connection"))
                    },
                    async {
                        reqwest::get(format!("http://localhost:{}/reqwest", addr.port()))
                            .await
                            .with_context(|| format!("failed to GET `{addr}`"))
                    }
                )?;
                Ok(())
            }
        )?;
    }

    {
        use wrpc_interface_http::OutgoingHandler;

        let invocations = client.serve_handle().await.context("failed to serve")?;
        try_join!(
            async {
                let AcceptedInvocation {
                    params:
                        (
                            Request {
                                mut body,
                                trailers,
                                method,
                                path_with_query,
                                scheme,
                                authority,
                                headers,
                            },
                            opts,
                        ),
                    result_subject,
                    transmitter,
                    ..
                } = pin!(invocations)
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(method, Method::Get);
                assert_eq!(path_with_query.as_deref(), Some("path_with_query"));
                assert_eq!(scheme, Some(Scheme::HTTPS));
                assert_eq!(authority.as_deref(), Some("authority"));
                assert_eq!(
                    headers,
                    vec![("user-agent".into(), vec!["wrpc/0.1.0".into()])],
                );
                assert_eq!(
                    opts,
                    Some(RequestOptions {
                        connect_timeout: None,
                        first_byte_timeout: Some(Duration::from_nanos(42)),
                        between_bytes_timeout: None,
                    })
                );
                try_join!(
                    async {
                        info!("transmit response");
                        transmitter
                            .transmit_static(
                                result_subject,
                                Ok::<_, ErrorCode>(Response {
                                    body: stream::empty(),
                                    trailers: async { None },
                                    status: 400,
                                    headers: Vec::default(),
                                }),
                            )
                            .await
                            .context("failed to transmit response")?;
                        info!("response transmitted");
                        anyhow::Ok(())
                    },
                    async {
                        info!("await request body element");
                        let item = body
                            .try_next()
                            .await
                            .context("failed to receive body item")?
                            .context("unexpected end of body stream")?;
                        assert_eq!(str::from_utf8(&item).unwrap(), "element");
                        info!("await request body end");
                        let item = body
                            .try_next()
                            .await
                            .context("failed to receive end item")?;
                        assert_eq!(item, None);
                        info!("request body verified");
                        Ok(())
                    },
                    async {
                        info!("await request trailers");
                        let trailers = trailers.await.context("failed to receive trailers")?;
                        assert_eq!(
                            trailers,
                            Some(vec![("trailer".into(), vec!["test".into()])])
                        );
                        info!("request trailers verified");
                        Ok(())
                    }
                )?;
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_handle(
                        Request {
                            body: stream::iter([("element".into())]),
                            trailers: async { Some(vec![("trailer".into(), vec!["test".into()])]) },
                            method: Method::Get,
                            path_with_query: Some("path_with_query".to_string()),
                            scheme: Some(Scheme::HTTPS),
                            authority: Some("authority".to_string()),
                            headers: vec![("user-agent".into(), vec!["wrpc/0.1.0".into()])],
                        },
                        Some(RequestOptions {
                            connect_timeout: None,
                            first_byte_timeout: Some(Duration::from_nanos(42)),
                            between_bytes_timeout: None,
                        }),
                    )
                    .await
                    .context("failed to invoke")?;
                let Response {
                    mut body,
                    trailers,
                    status,
                    headers,
                } = res.expect("invocation failed");
                assert_eq!(status, 400);
                assert_eq!(headers, Vec::default());
                try_join!(
                    async {
                        info!("transmit async parameters");
                        tx.await.context("failed to transmit parameters")?;
                        info!("async parameters transmitted");
                        anyhow::Ok(())
                    },
                    async {
                        info!("await response body end");
                        let item = body
                            .try_next()
                            .await
                            .context("failed to receive end item")?;
                        assert_eq!(item, None);
                        info!("response body verified");
                        Ok(())
                    },
                    async {
                        info!("await response trailers");
                        let trailers = trailers.await.context("failed to receive trailers")?;
                        assert_eq!(trailers, None);
                        info!("response trailers verified");
                        Ok(())
                    }
                )?;
                Ok(())
            }
        )?;
    }
    {
        use wrpc_interface_blobstore::Blobstore;

        let invocations = client
            .serve_clear_container()
            .await
            .context("failed to serve `clear-container`")?;
        let mut invocations = pin!(invocations);
        try_join!(
            async {
                let AcceptedInvocation {
                    params: name,
                    result_subject,
                    transmitter,
                    ..
                } = invocations
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(name, "test");
                info!("transmit response");
                transmitter
                    .transmit_static(result_subject, Ok::<_, String>(()))
                    .await
                    .context("failed to transmit response")?;
                info!("response transmitted");
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_clear_container("test")
                    .await
                    .context("failed to invoke")?;
                let () = res.expect("invocation failed");
                info!("transmit async parameters");
                tx.await.context("failed to transmit parameters")?;
                info!("async parameters transmitted");
                Ok(())
            }
        )?;

        try_join!(
            async {
                let AcceptedInvocation {
                    params: name,
                    result_subject,
                    transmitter,
                    ..
                } = invocations
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(name, "test");
                info!("transmit response");
                transmitter
                    .transmit_static(result_subject, Err::<(), _>("test"))
                    .await
                    .context("failed to transmit response")?;
                info!("response transmitted");
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_clear_container("test")
                    .await
                    .context("failed to invoke")?;
                let err = res.expect_err("invocation should have failed");
                assert_eq!(err, "test");
                info!("transmit async parameters");
                tx.await.context("failed to transmit parameters")?;
                info!("async parameters transmitted");
                Ok(())
            }
        )?;
    }

    {
        use wrpc_interface_blobstore::Blobstore;

        let invocations = client
            .serve_container_exists()
            .await
            .context("failed to serve `container-exists`")?;
        try_join!(
            async {
                let AcceptedInvocation {
                    params: name,
                    result_subject,
                    transmitter,
                    ..
                } = pin!(invocations)
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(name, "test");
                info!("transmit response");
                transmitter
                    .transmit_static(result_subject, Ok::<_, String>(true))
                    .await
                    .context("failed to transmit response")?;
                info!("response transmitted");
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_container_exists("test")
                    .await
                    .context("failed to invoke")?;
                let exists = res.expect("invocation failed");
                assert!(exists);
                info!("transmit async parameters");
                tx.await.context("failed to transmit parameters")?;
                info!("async parameters transmitted");
                Ok(())
            }
        )?;
    }
    {
        use wrpc_interface_blobstore::Blobstore;

        let invocations = client
            .serve_create_container()
            .await
            .context("failed to serve `create-container`")?;
        try_join!(
            async {
                let AcceptedInvocation {
                    params: name,
                    result_subject,
                    transmitter,
                    ..
                } = pin!(invocations)
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(name, "test");
                info!("transmit response");
                transmitter
                    .transmit_static(result_subject, Ok::<_, String>(()))
                    .await
                    .context("failed to transmit response")?;
                info!("response transmitted");
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_create_container("test")
                    .await
                    .context("failed to invoke")?;
                let () = res.expect("invocation failed");
                info!("transmit async parameters");
                tx.await.context("failed to transmit parameters")?;
                info!("async parameters transmitted");
                Ok(())
            }
        )?;
    }
    {
        use wrpc_interface_blobstore::Blobstore;

        let invocations = client
            .serve_delete_container()
            .await
            .context("failed to serve `delete-container`")?;
        try_join!(
            async {
                let AcceptedInvocation {
                    params: name,
                    result_subject,
                    transmitter,
                    ..
                } = pin!(invocations)
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(name, "test");
                info!("transmit response");
                transmitter
                    .transmit_static(result_subject, Ok::<_, String>(()))
                    .await
                    .context("failed to transmit response")?;
                info!("response transmitted");
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_delete_container("test")
                    .await
                    .context("failed to invoke")?;
                let () = res.expect("invocation failed");
                info!("transmit async parameters");
                tx.await.context("failed to transmit parameters")?;
                info!("async parameters transmitted");
                Ok(())
            }
        )?;
    }
    {
        use wrpc_interface_blobstore::Blobstore;

        let invocations = client
            .serve_get_container_info()
            .await
            .context("failed to serve `get-container-info`")?;
        try_join!(
            async {
                let AcceptedInvocation {
                    params: name,
                    result_subject,
                    transmitter,
                    ..
                } = pin!(invocations)
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(name, "test");
                info!("transmit response");
                transmitter
                    .transmit_static(
                        result_subject,
                        Ok::<_, String>(ContainerMetadata { created_at: 42 }),
                    )
                    .await
                    .context("failed to transmit response")?;
                info!("response transmitted");
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_get_container_info("test")
                    .await
                    .context("failed to invoke")?;
                let ContainerMetadata { created_at } = res.expect("invocation failed");
                assert_eq!(created_at, 42);
                info!("transmit async parameters");
                tx.await.context("failed to transmit parameters")?;
                info!("async parameters transmitted");
                Ok(())
            }
        )?;
    }
    {
        use wrpc_interface_blobstore::Blobstore;

        let invocations = client
            .serve_list_container_objects()
            .await
            .context("failed to serve `list-container-objects`")?;
        try_join!(
            async {
                let AcceptedInvocation {
                    params: (name, limit, offset),
                    result_subject,
                    transmitter,
                    ..
                } = pin!(invocations)
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(name, "test");
                assert_eq!(limit, Some(100));
                assert_eq!(offset, None);
                info!("transmit response");
                transmitter
                    .transmit_static(
                        result_subject,
                        Ok::<_, String>(Value::Stream(Box::pin(stream::iter([Ok(vec![
                            Some("first".to_string().into()),
                            Some("second".to_string().into()),
                        ])])))),
                    )
                    .await
                    .context("failed to transmit response")?;
                info!("response transmitted");
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_list_container_objects("test", Some(100), None)
                    .await
                    .context("failed to invoke")?;
                let names = res.expect("invocation failed");
                let names = names
                    .try_collect::<Vec<_>>()
                    .await
                    .context("failed to collect names")?;
                assert_eq!(names, [["first", "second"]]);
                info!("transmit async parameters");
                tx.await.context("failed to transmit parameters")?;
                info!("async parameters transmitted");
                Ok(())
            }
        )?;
    }
    {
        use wrpc_interface_blobstore::Blobstore;

        let invocations = client
            .serve_copy_object()
            .await
            .context("failed to serve `copy-object`")?;
        try_join!(
            async {
                let AcceptedInvocation {
                    params: (src, dest),
                    result_subject,
                    transmitter,
                    ..
                } = pin!(invocations)
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(
                    src,
                    ObjectId {
                        container: "container".to_string(),
                        object: "object".to_string(),
                    }
                );
                assert_eq!(
                    dest,
                    ObjectId {
                        container: "new-container".to_string(),
                        object: "new-object".to_string(),
                    }
                );
                info!("transmit response");
                transmitter
                    .transmit_static(result_subject, Ok::<_, String>(()))
                    .await
                    .context("failed to transmit response")?;
                info!("response transmitted");
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_copy_object(
                        &ObjectId {
                            container: "container".to_string(),
                            object: "object".to_string(),
                        },
                        &ObjectId {
                            container: "new-container".to_string(),
                            object: "new-object".to_string(),
                        },
                    )
                    .await
                    .context("failed to invoke")?;
                let () = res.expect("invocation failed");
                info!("transmit async parameters");
                tx.await.context("failed to transmit parameters")?;
                info!("async parameters transmitted");
                Ok(())
            }
        )?;
    }
    {
        use wrpc_interface_blobstore::Blobstore;

        let invocations = client
            .serve_delete_object()
            .await
            .context("failed to serve `delete-object`")?;
        try_join!(
            async {
                let AcceptedInvocation {
                    params: id,
                    result_subject,
                    transmitter,
                    ..
                } = pin!(invocations)
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(
                    id,
                    ObjectId {
                        container: "container".to_string(),
                        object: "object".to_string(),
                    }
                );
                info!("transmit response");
                transmitter
                    .transmit_static(result_subject, Ok::<_, String>(()))
                    .await
                    .context("failed to transmit response")?;
                info!("response transmitted");
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_delete_object(&ObjectId {
                        container: "container".to_string(),
                        object: "object".to_string(),
                    })
                    .await
                    .context("failed to invoke")?;
                let () = res.expect("invocation failed");
                info!("transmit async parameters");
                tx.await.context("failed to transmit parameters")?;
                info!("async parameters transmitted");
                Ok(())
            }
        )?;
    }
    {
        use wrpc_interface_blobstore::Blobstore;

        let invocations = client
            .serve_delete_objects()
            .await
            .context("failed to serve `delete-objects`")?;
        try_join!(
            async {
                let AcceptedInvocation {
                    params: (container, objects),
                    result_subject,
                    transmitter,
                    ..
                } = pin!(invocations)
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(container, "container".to_string());
                assert_eq!(objects, ["object".to_string(), "new-object".to_string()]);
                info!("transmit response");
                transmitter
                    .transmit_static(result_subject, Ok::<_, String>(()))
                    .await
                    .context("failed to transmit response")?;
                info!("response transmitted");
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_delete_objects("container", ["object", "new-object"])
                    .await
                    .context("failed to invoke")?;
                let () = res.expect("invocation failed");
                info!("transmit async parameters");
                tx.await.context("failed to transmit parameters")?;
                info!("async parameters transmitted");
                Ok(())
            }
        )?;
    }
    {
        use wrpc_interface_blobstore::Blobstore;

        let invocations = client
            .serve_get_container_data()
            .await
            .context("failed to serve `get-container-data`")?;
        try_join!(
            async {
                let AcceptedInvocation {
                    params: (id, start, end),
                    result_subject,
                    transmitter,
                    ..
                } = pin!(invocations)
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(
                    id,
                    ObjectId {
                        container: "container".to_string(),
                        object: "object".to_string(),
                    }
                );
                assert_eq!(start, 42);
                assert_eq!(end, 4242);
                info!("transmit response");
                transmitter
                    .transmit_static(
                        result_subject,
                        Ok::<_, String>(Value::Stream(Box::pin(stream::iter([Ok(vec![
                            Some(0x42u8.into()),
                            Some(0xffu8.into()),
                        ])])))),
                    )
                    .await
                    .context("failed to transmit response")?;
                info!("response transmitted");
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_get_container_data(
                        &ObjectId {
                            container: "container".to_string(),
                            object: "object".to_string(),
                        },
                        42,
                        4242,
                    )
                    .await
                    .context("failed to invoke")?;
                let data = res.expect("invocation failed");
                try_join!(
                    async {
                        let data = data
                            .try_collect::<Vec<_>>()
                            .await
                            .context("failed to collect data")?;
                        assert_eq!(data, [Bytes::from([0x42, 0xff].as_slice())]);
                        Ok(())
                    },
                    async {
                        info!("transmit async parameters");
                        tx.await.context("failed to transmit parameters")?;
                        info!("async parameters transmitted");
                        Ok(())
                    }
                )
            }
        )?;
    }
    {
        use wrpc_interface_blobstore::Blobstore;

        let invocations = client
            .serve_get_object_info()
            .await
            .context("failed to serve `get-object-info`")?;
        try_join!(
            async {
                let AcceptedInvocation {
                    params: id,
                    result_subject,
                    transmitter,
                    ..
                } = pin!(invocations)
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(
                    id,
                    ObjectId {
                        container: "container".to_string(),
                        object: "object".to_string(),
                    }
                );
                info!("transmit response");
                transmitter
                    .transmit_static(
                        result_subject,
                        Ok::<_, String>(ObjectMetadata {
                            created_at: 42,
                            size: 4242,
                        }),
                    )
                    .await
                    .context("failed to transmit response")?;
                info!("response transmitted");
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_get_object_info(&ObjectId {
                        container: "container".to_string(),
                        object: "object".to_string(),
                    })
                    .await
                    .context("failed to invoke")?;
                let v = res.expect("invocation failed");
                assert_eq!(
                    v,
                    ObjectMetadata {
                        created_at: 42,
                        size: 4242,
                    }
                );
                info!("transmit async parameters");
                tx.await.context("failed to transmit parameters")?;
                info!("async parameters transmitted");
                Ok(())
            }
        )?;
    }
    {
        use wrpc_interface_blobstore::Blobstore;

        let invocations = client
            .serve_has_object()
            .await
            .context("failed to serve `has-object`")?;
        try_join!(
            async {
                let AcceptedInvocation {
                    params: id,
                    result_subject,
                    transmitter,
                    ..
                } = pin!(invocations)
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(
                    id,
                    ObjectId {
                        container: "container".to_string(),
                        object: "object".to_string(),
                    }
                );
                info!("transmit response");
                transmitter
                    .transmit_static(result_subject, Ok::<_, String>(true))
                    .await
                    .context("failed to transmit response")?;
                info!("response transmitted");
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_has_object(&ObjectId {
                        container: "container".to_string(),
                        object: "object".to_string(),
                    })
                    .await
                    .context("failed to invoke")?;
                let v = res.expect("invocation failed");
                assert!(v);
                info!("transmit async parameters");
                tx.await.context("failed to transmit parameters")?;
                info!("async parameters transmitted");
                Ok(())
            }
        )?;
    }
    {
        use wrpc_interface_blobstore::Blobstore;

        let invocations = client
            .serve_move_object()
            .await
            .context("failed to serve `move-object`")?;
        try_join!(
            async {
                let AcceptedInvocation {
                    params: (src, dest),
                    result_subject,
                    transmitter,
                    ..
                } = pin!(invocations)
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(
                    src,
                    ObjectId {
                        container: "container".to_string(),
                        object: "object".to_string(),
                    }
                );
                assert_eq!(
                    dest,
                    ObjectId {
                        container: "new-container".to_string(),
                        object: "new-object".to_string(),
                    }
                );
                info!("transmit response");
                transmitter
                    .transmit_static(result_subject, Ok::<_, String>(()))
                    .await
                    .context("failed to transmit response")?;
                info!("response transmitted");
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_move_object(
                        &ObjectId {
                            container: "container".to_string(),
                            object: "object".to_string(),
                        },
                        &ObjectId {
                            container: "new-container".to_string(),
                            object: "new-object".to_string(),
                        },
                    )
                    .await
                    .context("failed to invoke")?;
                let () = res.expect("invocation failed");
                info!("transmit async parameters");
                tx.await.context("failed to transmit parameters")?;
                info!("async parameters transmitted");
                Ok(())
            }
        )?;
    }
    {
        use wrpc_interface_blobstore::Blobstore;

        let invocations = client
            .serve_write_container_data()
            .await
            .context("failed to serve `move-object`")?;
        try_join!(
            async {
                let AcceptedInvocation {
                    params: (id, data),
                    result_subject,
                    transmitter,
                    ..
                } = pin!(invocations)
                    .try_next()
                    .await
                    .context("failed to receive invocation")?
                    .context("unexpected end of stream")?;
                assert_eq!(
                    id,
                    ObjectId {
                        container: "container".to_string(),
                        object: "object".to_string(),
                    }
                );
                let data = data
                    .map_ok(|buf| String::from_utf8(buf.to_vec()).unwrap())
                    .try_collect::<Vec<_>>()
                    .await
                    .context("failed to collect data")?;
                assert_eq!(data, ["test"]);
                info!("transmit response");
                transmitter
                    .transmit_static(result_subject, Ok::<_, String>(()))
                    .await
                    .context("failed to transmit response")?;
                info!("response transmitted");
                anyhow::Ok(())
            },
            async {
                info!("invoke function");
                let (res, tx) = client
                    .invoke_write_container_data(
                        &ObjectId {
                            container: "container".to_string(),
                            object: "object".to_string(),
                        },
                        Box::pin(stream::iter(["test".into()])),
                    )
                    .await
                    .context("failed to invoke")?;
                let () = res.expect("invocation failed");
                info!("transmit async parameters");
                tx.await.context("failed to transmit parameters")?;
                info!("async parameters transmitted");
                Ok(())
            }
        )?;
    }

    stop_tx.send(()).expect("failed to stop NATS.io server");
    nats_server
        .await
        .context("failed to await NATS.io server stop")?
        .context("NATS.io server failed to stop")?;
    Ok(())
}
