use core::pin::Pin;
use core::time::Duration;

use anyhow::{Context as _, bail, ensure};
use bytes::Bytes;
use futures::{StreamExt as _, future, stream};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::oneshot;
use tokio::task::yield_now;
use tokio::time::timeout;
use tokio::{join, try_join};
use tracing::info;

pub async fn assert_async<F, I, O, E>(connect: impl Fn() -> F) -> anyhow::Result<()>
where
    F: Future<Output = Result<(O, I), E>>,
    I: AsyncRead + Send + Unpin,
    O: AsyncWrite + Send + Unpin,
    E: core::error::Error + Send + Sync + 'static,
{
    wit_bindgen_wrpc::generate!({
        world: "async-client",
        path: "tests/wit",
    });

    fn pending(s: &'static str) -> Pin<Box<dyn Future<Output = String> + Send>> {
        Box::pin(async move {
            yield_now().await;
            s.to_string()
        })
    }

    info!("calling `wrpc-test:integration/async.with-streams`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let ((a, b), io) = wrpc_test::integration::async_::with_streams(tx, rx)
        .await
        .context("failed to call `wrpc-test:integration/async.with-streams`")?;
    join!(
        async {
            info!("receiving `a`");
            assert_eq!(a.collect::<Vec<Bytes>>().await.concat(), b"test");
        },
        async {
            info!("receiving `b`");
            assert_eq!(
                b.collect::<Vec<_>>().await.concat(),
                [["foo"].as_slice(), ["bar", "baz"].as_slice()]
            );
        },
        async {
            info!("performing I/O");
            io.await.expect("failed to complete async I/O");
        }
    );

    info!("calling `wrpc-test:integration/async.with-future`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let (fut, io) = wrpc_test::integration::async_::with_future(
        tx,
        rx,
        &wrpc_test::integration::async_::Something {
            foo: "bar".to_string(),
        },
        Box::pin(stream::iter(["foo".into(), "bar".into()])),
    )
    .await
    .context("failed to call `wrpc-test:integration/async.with-future`")?;
    join!(
        async {
            info!("receiving results");
            assert_eq!(fut.await.collect::<Vec<Bytes>>().await.concat(), b"foobar");
        },
        async {
            info!("performing I/O");
            io.await.expect("failed to complete async I/O");
        }
    );

    info!("calling `wrpc-test:integration/async.identity-future`");
    let (fut_tx, fut_rx) = oneshot::channel();
    let (tx, rx) = connect().await.context("failed to connect")?;
    let (mut fut, io) = wrpc_test::integration::async_::identity_future(
        tx,
        rx,
        Box::pin(async { fut_rx.await.expect("sender dropped") }),
    )
    .await
    .context("failed to call `wrpc-test:integration/async.identity-future`")?;
    try_join!(
        async {
            info!("receiving results");
            ensure!(
                timeout(Duration::from_millis(100), &mut fut).await.is_err(),
                "future resolved before its parameter"
            );
            fut_tx.send(()).expect("receiver dropped");
            fut.await;
            Ok(())
        },
        async {
            info!("performing I/O");
            io.await.context("failed to complete async I/O")?;
            Ok(())
        }
    )?;

    info!("calling `wrpc-test:integration/async.identity-cases`");
    let (o_tx, o_rx) = oneshot::channel();
    let (r_tx, r_rx) = oneshot::channel();
    let (v_tx, v_rx) = oneshot::channel();
    let (tx, rx) = connect().await.context("failed to connect")?;
    let ((o, r, v), io) = wrpc_test::integration::async_::identity_cases(
        tx,
        rx,
        Some(Box::pin(async { o_rx.await.expect("sender dropped") })),
        Err(Box::pin(async { r_rx.await.expect("sender dropped") })),
        wrpc_test::integration::async_::AsyncCase::Fut(Box::pin(async {
            v_rx.await.expect("sender dropped")
        })),
    )
    .await
    .context("failed to call `wrpc-test:integration/async.identity-cases`")?;
    try_join!(
        async {
            info!("receiving results");
            let Some(mut o) = o else {
                bail!("expected `option::some`");
            };
            let Err(r) = r else {
                bail!("expected `result::err`");
            };
            let wrpc_test::integration::async_::AsyncCase::Fut(v) = v else {
                bail!("expected `async-case::fut`");
            };
            ensure!(
                timeout(Duration::from_millis(100), &mut o).await.is_err(),
                "future resolved before its parameter"
            );
            o_tx.send(()).expect("receiver dropped");
            r_tx.send(42).expect("receiver dropped");
            v_tx.send("foo".to_string()).expect("receiver dropped");
            o.await;
            ensure!(r.await == 42);
            ensure!(v.await == "foo");
            Ok(())
        },
        async {
            info!("performing I/O");
            io.await.context("failed to complete async I/O")?;
            Ok(())
        }
    )?;

    for v in [
        wrpc_test::integration::async_::AsyncCase::Elements(vec![pending("foo"), pending("bar")]),
        wrpc_test::integration::async_::AsyncCase::Pair((1, pending("baz"))),
        wrpc_test::integration::async_::AsyncCase::Chunks(Box::pin(stream::iter([
            vec![pending("foo")],
            vec![pending("bar"), pending("baz")],
        ]))),
    ] {
        info!(?v, "calling `wrpc-test:integration/async.identity-cases`");
        let (tx, rx) = connect().await.context("failed to connect")?;
        let ((o, r, v), io) = wrpc_test::integration::async_::identity_cases(
            tx,
            rx,
            None,
            Err(Box::pin(async { 42 })),
            v,
        )
        .await
        .context("failed to call `wrpc-test:integration/async.identity-cases`")?;
        try_join!(
            async {
                info!("receiving results");
                ensure!(o.is_none(), "expected `option::none`");
                let Err(r) = r else {
                    bail!("expected `result::err`");
                };
                ensure!(r.await == 42);
                match v {
                    wrpc_test::integration::async_::AsyncCase::Elements(v) => {
                        ensure!(future::join_all(v).await == ["foo", "bar"])
                    }
                    wrpc_test::integration::async_::AsyncCase::Pair((1, v)) => {
                        ensure!(v.await == "baz")
                    }
                    wrpc_test::integration::async_::AsyncCase::Chunks(v) => {
                        let v = v
                            .flat_map(stream::iter)
                            .then(|v| v)
                            .collect::<Vec<_>>()
                            .await;
                        ensure!(v == ["foo", "bar", "baz"])
                    }
                    _ => bail!("unexpected `async-case`"),
                }
                Ok(())
            },
            async {
                info!("performing I/O");
                io.await.context("failed to complete async I/O")?;
                Ok(())
            }
        )?;
    }

    Ok(())
}

/// Call every function of the `sync-client` world in `tests/wit` and check the
/// results the `sync-server` world is expected to return.
pub async fn assert_sync<F, I, O, E>(connect: impl Fn() -> F) -> anyhow::Result<()>
where
    F: Future<Output = Result<(O, I), E>>,
    I: AsyncRead + Send + Unpin,
    O: AsyncWrite + Send + Unpin,
    E: core::error::Error + Send + Sync + 'static,
{
    mod bindings {
        wit_bindgen_wrpc::generate!({
            world: "sync-client",
            path: "tests/wit",
            additional_derives: [::core::cmp::PartialEq],
        });
    }

    use bindings::wrpc_test::integration::resources::{self, Counter, Foo};
    use bindings::wrpc_test::integration::sync;
    use bindings::wrpc_test::integration::sync::{Abc, Foobar, Rec, RecNested, Var};
    use bindings::{foo, strange};

    info!("calling `wrpc-test:integration/sync-client.foo.f`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = foo::f(tx, rx, "f")
        .await
        .context("failed to call `wrpc-test:integration/sync-client.foo.f`")?;
    ensure!(v == 42);

    info!("calling `wrpc-test:integration/sync-client.foo.foo`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    foo::foo(tx, rx, "foo")
        .await
        .context("failed to call `wrpc-test:integration/sync-client.foo.foo`")?;

    info!("calling `wrpc-test:integration/sync.fallible`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let res = sync::fallible(tx, rx, true)
        .await
        .context("failed to call `wrpc-test:integration/sync.fallible`")?;
    ensure!(res == Ok(true));

    info!("calling `wrpc-test:integration/sync.fallible`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let res = sync::fallible(tx, rx, false)
        .await
        .context("failed to call `wrpc-test:integration/sync.fallible`")?;
    ensure!(res == Err("test".to_string()));

    info!("calling `wrpc-test:integration/sync.numbers`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let (a, b, c, d, e, f, g, h, i, j) = sync::numbers(tx, rx)
        .await
        .context("failed to call `wrpc-test:integration/sync.numbers`")?;
    ensure!(a == 1);
    ensure!(b == 2);
    ensure!(c == 3);
    ensure!(d == 4);
    ensure!(e == 5);
    ensure!(f == 6);
    ensure!(g == 7);
    ensure!(h == 8);
    ensure!(i == 9.);
    ensure!(j == 10.);

    info!("calling `wrpc-test:integration/sync.with-flags`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = sync::with_flags(tx, rx, true, false, true)
        .await
        .context("failed to call `wrpc-test:integration/sync.with-flags`")?;
    ensure!(v == Abc::A | Abc::C, "{v:?}");

    info!("calling `wrpc-test:integration/sync.with-variant-option`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = sync::with_variant_option(tx, rx, false)
        .await
        .context("failed to call `wrpc-test:integration/sync.with-variant-option`")?;
    ensure!(v.is_none(), "{v:?}");

    info!("calling `wrpc-test:integration/sync.with-variant-option`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = sync::with_variant_option(tx, rx, true)
        .await
        .context("failed to call `wrpc-test:integration/sync.with-variant-option`")?;
    ensure!(
        v == Some(Var::Var(Rec {
            nested: RecNested {
                foo: "bar".to_string()
            }
        })),
        "{v:?}"
    );

    info!("calling `wrpc-test:integration/sync.with-variant-list`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = sync::with_variant_list(tx, rx)
        .await
        .context("failed to call `wrpc-test:integration/sync.with-variant-list`")?;
    ensure!(
        v == [
            Var::Empty,
            Var::Var(Rec {
                nested: RecNested {
                    foo: "foo".to_string()
                }
            }),
            Var::Empty,
            Var::Empty,
            Var::Empty,
            Var::Var(Rec {
                nested: RecNested {
                    foo: "bar".to_string()
                }
            }),
        ],
        "{v:?}"
    );

    info!("calling `wrpc-test:integration/sync.with-record`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = sync::with_record(tx, rx)
        .await
        .context("failed to call `wrpc-test:integration/sync.with-record`")?;
    ensure!(
        v == Rec {
            nested: RecNested {
                foo: "foo".to_string()
            }
        },
        "{v:?}"
    );

    info!("calling `wrpc-test:integration/sync.with-record-list`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = sync::with_record_list(tx, rx, 0)
        .await
        .context("failed to call `wrpc-test:integration/sync.with-record-list`")?;
    ensure!(v.is_empty(), "{v:?}");

    info!("calling `wrpc-test:integration/sync.with-record-list`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = sync::with_record_list(tx, rx, 3)
        .await
        .context("failed to call `wrpc-test:integration/sync.with-record-list`")?;
    ensure!(
        v == [
            Rec {
                nested: RecNested {
                    foo: "0".to_string()
                }
            },
            Rec {
                nested: RecNested {
                    foo: "1".to_string()
                }
            },
            Rec {
                nested: RecNested {
                    foo: "2".to_string()
                }
            },
        ],
        "{v:?}",
    );

    info!("calling `wrpc-test:integration/sync.with-record-tuple`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = sync::with_record_tuple(tx, rx)
        .await
        .context("failed to call `wrpc-test:integration/sync.with-record-tuple`")?;
    ensure!(
        v == (
            Rec {
                nested: RecNested {
                    foo: "0".to_string()
                }
            },
            Rec {
                nested: RecNested {
                    foo: "1".to_string()
                }
            },
        ),
        "{v:?}",
    );

    info!("calling `wrpc-test:integration/sync.with-enum`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = sync::with_enum(tx, rx)
        .await
        .context("failed to call `wrpc-test:integration/sync.with-enum`")?;
    ensure!(v == Foobar::Bar, "{v:?}");

    for c in ['a', 'é', '🚩'] {
        info!("calling `wrpc-test:integration/sync.identity-char`");
        let (tx, rx) = connect().await.context("failed to connect")?;
        let v = sync::identity_char(tx, rx, c)
            .await
            .context("failed to call `wrpc-test:integration/sync.identity-char`")?;
        ensure!(v == c, "{v:?}");
    }

    for (a, b) in [
        (None, None),
        (Some(Bytes::default()), Some(Vec::default())),
        (Some(Bytes::from("foo")), Some(vec!["foo".to_string()])),
    ] {
        let b_ref = b
            .as_ref()
            .map(|b| b.iter().map(String::as_str).collect::<Vec<_>>());
        info!("calling `wrpc-test:integration/sync.identity-option-lists`");
        let (tx, rx) = connect().await.context("failed to connect")?;
        let v = sync::identity_option_lists(tx, rx, a.as_ref(), b_ref.as_deref())
            .await
            .context("failed to call `wrpc-test:integration/sync.identity-option-lists`")?;
        ensure!(v == (a, b), "{v:?}");
    }

    info!("calling `wrpc-test:integration/sync-client.ping`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = bindings::ping(tx, rx)
        .await
        .context("failed to call `wrpc-test:integration/sync-client.ping`")?;
    ensure!(v == "pong", "{v:?}");

    info!("calling `wrpc-test:integration/resources.[constructor]foo`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let res = Foo::new(tx, rx)
        .await
        .context("failed to call `wrpc-test:integration/resources.[constructor]foo`")?;

    info!("calling `wrpc-test:integration/resources.[method]foo.bar`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = Foo::bar(tx, rx, &res.as_borrow())
        .await
        .context("failed to call `wrpc-test:integration/resources.[method]foo.bar`")?;
    ensure!(v == "bar", "{v:?}");

    info!("calling `wrpc-test:integration/resources.bar`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = resources::bar(tx, rx, &res.as_borrow())
        .await
        .context("failed to call `wrpc-test:integration/resources.bar`")?;
    ensure!(v == "bar", "{v:?}");

    info!("calling `wrpc-test:integration/sync-client.strange.bar`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = strange::bar(tx, rx, &res.as_borrow())
        .await
        .context("failed to call `wrpc-test:integration/sync-client.strange.bar`")?;
    ensure!(v == 42, "{v:?}");

    info!("calling `wrpc-test:integration/resources.[static]foo.foo`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = Foo::foo(tx, rx, &res)
        .await
        .context("failed to call `wrpc-test:integration/resources.[static]foo.foo`")?;
    ensure!(v == "foo", "{v:?}");

    info!("calling `wrpc-test:integration/resources.[constructor]counter`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let a = Counter::new(tx, rx, 1)
        .await
        .context("failed to call `wrpc-test:integration/resources.[constructor]counter`")?;

    info!("calling `wrpc-test:integration/resources.[method]counter.increment-by`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    Counter::increment_by(tx, rx, &a.as_borrow(), 2)
        .await
        .context("failed to call `wrpc-test:integration/resources.[method]counter.increment-by`")?;

    info!("calling `wrpc-test:integration/resources.[method]counter.get-count`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = Counter::get_count(tx, rx, &a.as_borrow())
        .await
        .context("failed to call `wrpc-test:integration/resources.[method]counter.get-count`")?;
    ensure!(v == 3, "{v:?}");

    info!("calling `wrpc-test:integration/resources.[method]counter.clone-counter`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let b = Counter::clone_counter(tx, rx, &a.as_borrow())
        .await
        .context(
            "failed to call `wrpc-test:integration/resources.[method]counter.clone-counter`",
        )?;

    info!("calling `wrpc-test:integration/resources.[method]counter.increment-by`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    Counter::increment_by(tx, rx, &b.as_borrow(), 1)
        .await
        .context("failed to call `wrpc-test:integration/resources.[method]counter.increment-by`")?;

    info!("calling `wrpc-test:integration/resources.[static]counter.sum`");
    let (tx, rx) = connect().await.context("failed to connect")?;
    let v = Counter::sum(tx, rx, &a.as_borrow(), &b.as_borrow())
        .await
        .context("failed to call `wrpc-test:integration/resources.[static]counter.sum`")?;
    ensure!(v == 7, "{v:?}");
    Ok(())
}
