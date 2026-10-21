use anyhow::{Context as _, ensure};
use bytes::Bytes;
use futures::{StreamExt as _, stream};
use tokio::join;
use tracing::info;

pub async fn assert_async<C: Clone>(
    cx: C,
    client: &impl wrpc_transport::Invoke<Context = C>,
) -> anyhow::Result<()> {
    wit_bindgen_wrpc::generate!({
        world: "async-client",
        path: "tests/wit",
    });

    info!("calling `wrpc-test:integration/async.with-streams`");
    let ((a, b), io) = wrpc_test::integration::async_::with_streams(client, cx.clone())
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
            if let Some(io) = io {
                info!("performing I/O");
                io.await.expect("failed to complete async I/O");
            }
        }
    );

    info!("calling `wrpc-test:integration/async.with-future`");
    let (fut, io) = wrpc_test::integration::async_::with_future(
        client,
        cx.clone(),
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
            if let Some(io) = io {
                info!("performing I/O");
                io.await.expect("failed to complete async I/O");
            }
        }
    );

    Ok(())
}

/// Call each function of the `sync-client` world in `tests/wit` and check the
/// results the `sync-server` world is expected to return.
pub async fn assert_sync<C: Clone>(
    cx: C,
    client: &impl wrpc_transport::Invoke<Context = C>,
) -> anyhow::Result<()> {
    mod bindings {
        wit_bindgen_wrpc::generate!({
            world: "sync-client",
            path: "tests/wit",
            additional_derives: [::core::cmp::PartialEq],
        });
    }

    use bindings::foo;
    use bindings::wrpc_test::integration::sync;
    use bindings::wrpc_test::integration::sync::{Abc, Foobar, Rec, RecNested, Var};

    info!("calling `wrpc-test:integration/sync-client.foo.f`");
    let v = foo::f(client, cx.clone(), "f")
        .await
        .context("failed to call `wrpc-test:integration/sync-client.foo.f`")?;
    ensure!(v == 42);

    info!("calling `wrpc-test:integration/sync-client.foo.foo`");
    foo::foo(client, cx.clone(), "foo")
        .await
        .context("failed to call `wrpc-test:integration/sync-client.foo.foo`")?;

    info!("calling `wrpc-test:integration/sync.fallible`");
    let res = sync::fallible(client, cx.clone(), true)
        .await
        .context("failed to call `wrpc-test:integration/sync.fallible`")?;
    ensure!(res == Ok(true));

    info!("calling `wrpc-test:integration/sync.fallible`");
    let res = sync::fallible(client, cx.clone(), false)
        .await
        .context("failed to call `wrpc-test:integration/sync.fallible`")?;
    ensure!(res == Err("test".to_string()));

    info!("calling `wrpc-test:integration/sync.numbers`");
    let (a, b, c, d, e, f, g, h, i, j) = sync::numbers(client, cx.clone())
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
    let v = sync::with_flags(client, cx.clone(), true, false, true)
        .await
        .context("failed to call `wrpc-test:integration/sync.with-flags`")?;
    ensure!(v == Abc::A | Abc::C, "{v:?}");

    info!("calling `wrpc-test:integration/sync.with-variant-option`");
    let v = sync::with_variant_option(client, cx.clone(), false)
        .await
        .context("failed to call `wrpc-test:integration/sync.with-variant-option`")?;
    ensure!(v.is_none(), "{v:?}");

    info!("calling `wrpc-test:integration/sync.with-variant-option`");
    let v = sync::with_variant_option(client, cx.clone(), true)
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
    let v = sync::with_variant_list(client, cx.clone())
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
    let v = sync::with_record(client, cx.clone())
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
    let v = sync::with_record_list(client, cx.clone(), 0)
        .await
        .context("failed to call `wrpc-test:integration/sync.with-record-list`")?;
    ensure!(v.is_empty(), "{v:?}");

    info!("calling `wrpc-test:integration/sync.with-record-list`");
    let v = sync::with_record_list(client, cx.clone(), 3)
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
    let v = sync::with_record_tuple(client, cx.clone())
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
    let v = sync::with_enum(client, cx.clone())
        .await
        .context("failed to call `wrpc-test:integration/sync.with-enum`")?;
    ensure!(v == Foobar::Bar, "{v:?}");
    Ok(())
}
