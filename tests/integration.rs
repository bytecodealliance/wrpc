use core::iter::zip;
use core::time::Duration;

use std::net::Ipv6Addr;
use std::process::ExitStatus;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use futures::{stream, StreamExt as _, TryStreamExt as _};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio::{select, spawn, try_join};
use tracing::{info, instrument};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use wrpc::{DynamicFunctionType, ResourceType, Transmitter as _, Type, Value};
use wrpc_interface_http::{ErrorCode, Method, Request, RequestOptions, Response, Scheme};
use wrpc_transport::{AcceptedInvocation, Client as _, DynamicTuple};

async fn free_port() -> Result<u16> {
    TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
        .await
        .context("failed to start TCP listener")?
        .local_addr()
        .context("failed to query listener local address")
        .map(|v| v.port())
}

async fn spawn_server(
    cmd: &mut Command,
) -> Result<(JoinHandle<Result<ExitStatus>>, oneshot::Sender<()>)> {
    let mut child = cmd
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn child")?;
    let (stop_tx, stop_rx) = oneshot::channel();
    let child = spawn(async move {
        select!(
            res = stop_rx => {
                res.context("failed to wait for shutdown")?;
                child.kill().await.context("failed to kill child")?;
                child.wait().await
            }
            status = child.wait() => {
                status
            }
        )
        .context("failed to wait for child")
    });
    Ok((child, stop_tx))
}

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
            let mut invocations = client
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
                    } = invocations
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
async fn nats() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let port = free_port().await?;
    let (nats_server, stop_tx) =
        spawn_server(Command::new("nats-server").args(["-V", "-T=false", "-p", &port.to_string()]))
            .await
            .context("failed to start NATS")?;

    let (nats_conn_tx, mut nats_conn_rx) = mpsc::channel(1);
    let nats_client = async_nats::connect_with_options(
        format!("nats://localhost:{port}"),
        async_nats::ConnectOptions::new()
            .retry_on_initial_connect()
            .event_callback(move |event| {
                let nats_conn_tx = nats_conn_tx.clone();
                async move {
                    if let async_nats::Event::Connected = event {
                        nats_conn_tx
                            .send(())
                            .await
                            .expect("failed to send NATS connection notification");
                    }
                }
            }),
    )
    .await
    .context("failed to connect to NATS")?;
    nats_conn_rx
        .recv()
        .await
        .context("failed to await NATS connection to be established")?;

    let client = wrpc::transport::nats::Client::new(nats_client, "test-prefix".to_string());

    let (params, results) = loopback_dynamic(
        &client,
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
        Type::Float32,
        Type::Float64,
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
        Type::Float32,
        Type::Float64,
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
        &client,
        "sync",
        DynamicFunctionType::Static {
            params: Arc::clone(&sync_tuple_type),
            results: sync_tuple_type,
        },
        vec![
            Value::Bool(true),
            Value::U8(0xfe),
            Value::U16(0xfeff),
            Value::U32(0xfeffffff),
            Value::U64(0xfeffffffffffffff),
            Value::S8(0x7e),
            Value::S16(0x7eff),
            Value::S32(0x7effffff),
            Value::S64(0x7effffffffffffff),
            Value::Float32(0.42),
            Value::Float64(0.4242),
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
                Value::U32(0xfeffffff),
                Value::U64(0xfeffffffffffffff),
                Value::S8(0x7e),
                Value::S16(0x7eff),
                Value::S32(0x7effffff),
                Value::S64(0x7effffffffffffff),
                Value::Float32(0.42),
                Value::Float64(0.4242),
                Value::Char('a'),
                Value::String("test".into()),
                Value::Enum(0xfeff),
                Value::Flags(0xdeadbeef),
            ]),
            Value::Tuple(vec![
                Value::Bool(true),
                Value::U8(0xfe),
                Value::U16(0xfeff),
                Value::U32(0xfeffffff),
                Value::U64(0xfeffffffffffffff),
                Value::S8(0x7e),
                Value::S16(0x7eff),
                Value::S32(0x7effffff),
                Value::S64(0x7effffffffffffff),
                Value::Float32(0.42),
                Value::Float64(0.4242),
                Value::Char('a'),
                Value::String("test".into()),
                Value::Enum(0xfeff),
                Value::Flags(0xdeadbeef),
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
            Value::Flags(0xdeadbeef),
        ],
        vec![
            Value::Bool(true),
            Value::U8(0xfe),
            Value::U16(0xfeff),
            Value::U32(0xfeffffff),
            Value::U64(0xfeffffffffffffff),
            Value::S8(0x7e),
            Value::S16(0x7eff),
            Value::S32(0x7effffff),
            Value::S64(0x7effffffffffffff),
            Value::Float32(0.42),
            Value::Float64(0.4242),
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
                Value::U32(0xfeffffff),
                Value::U64(0xfeffffffffffffff),
                Value::S8(0x7e),
                Value::S16(0x7eff),
                Value::S32(0x7effffff),
                Value::S64(0x7effffffffffffff),
                Value::Float32(0.42),
                Value::Float64(0.4242),
                Value::Char('a'),
                Value::String("test".into()),
                Value::Enum(0xfeff),
                Value::Flags(0xdeadbeef),
            ]),
            Value::Tuple(vec![
                Value::Bool(true),
                Value::U8(0xfe),
                Value::U16(0xfeff),
                Value::U32(0xfeffffff),
                Value::U64(0xfeffffffffffffff),
                Value::S8(0x7e),
                Value::S16(0x7eff),
                Value::S32(0x7effffff),
                Value::S64(0x7effffffffffffff),
                Value::Float32(0.42),
                Value::Float64(0.4242),
                Value::Char('a'),
                Value::String("test".into()),
                Value::Enum(0xfeff),
                Value::Flags(0xdeadbeef),
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
            Value::Flags(0xdeadbeef),
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
        (Value::U32(0xfeffffff), Value::U32(0xfeffffff))
    ));
    assert!(matches!(
        values.next().unwrap(),
        (
            Value::U64(0xfeffffffffffffff),
            Value::U64(0xfeffffffffffffff)
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
        (Value::S32(0x7effffff), Value::S32(0x7effffff))
    ));
    assert!(matches!(
        values.next().unwrap(),
        (
            Value::S64(0x7effffffffffffff),
            Value::S64(0x7effffffffffffff)
        )
    ));
    assert!(matches!(
        values.next().unwrap(),
        (Value::Float32(p), Value::Float32(r)) if p == 0.42 && r == 0.42
    ));
    assert!(matches!(
        values.next().unwrap(),
        (Value::Float64(p), Value::Float64(r)) if p == 0.4242 && r == 0.4242
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
            (Value::U32(0xfeffffff), Value::U32(0xfeffffff))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (
                Value::U64(0xfeffffffffffffff),
                Value::U64(0xfeffffffffffffff)
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
            (Value::S32(0x7effffff), Value::S32(0x7effffff))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (
                Value::S64(0x7effffffffffffff),
                Value::S64(0x7effffffffffffff)
            )
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::Float32(p), Value::Float32(r)) if p == 0.42 && r == 0.42
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::Float64(p), Value::Float64(r)) if p == 0.4242 && r == 0.4242
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
            (Value::Flags(0xdeadbeef), Value::Flags(0xdeadbeef))
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
            (Value::U32(0xfeffffff), Value::U32(0xfeffffff))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (
                Value::U64(0xfeffffffffffffff),
                Value::U64(0xfeffffffffffffff)
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
            (Value::S32(0x7effffff), Value::S32(0x7effffff))
        ));
        assert!(matches!(
            values.next().unwrap(),
            (
                Value::S64(0x7effffffffffffff),
                Value::S64(0x7effffffffffffff)
            )
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::Float32(p), Value::Float32(r)) if p == 0.42 && r == 0.42
        ));
        assert!(matches!(
            values.next().unwrap(),
            (Value::Float64(p), Value::Float64(r)) if p == 0.4242 && r == 0.4242
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
            (Value::Flags(0xdeadbeef), Value::Flags(0xdeadbeef))
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
        (Value::Flags(0xdeadbeef), Value::Flags(0xdeadbeef))
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
        &client,
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

    let mut unit_invocations = client
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
            } = unit_invocations
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

    {
        use wrpc_interface_http::IncomingHandler;

        let mut invocations = client.serve_handle().await.context("failed to serve")?;
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
                assert_eq!(method, Method::Get);
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
                        assert_eq!(String::from_utf8(item).unwrap(), "element");
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
                    .invoke_handle(Request {
                        body: stream::iter([("element".into())]),
                        trailers: async { Some(vec![("trailer".into(), vec!["test".into()])]) },
                        method: Method::Get,
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
                Ok(())
            }
        )?;
    }

    {
        use wrpc_interface_http::OutgoingHandler;

        let mut invocations = client.serve_handle().await.context("failed to serve")?;
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
                } = invocations
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
                        assert_eq!(String::from_utf8(item).unwrap(), "element");
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

    stop_tx.send(()).expect("failed to stop NATS");
    nats_server
        .await
        .context("failed to stop NATS")?
        .context("NATS failed to stop")?;
    Ok(())
}
