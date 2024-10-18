use clap::Parser;

mod bindings {
    use crate::Handler;

    wit_bindgen::generate!({
        world: "server",
        with: {
            "wrpc-examples:hello/handler": generate,
        },
    });
    export!(Handler);
}

mod wrpc_bindings {
    wit_bindgen_wrpc::generate!({
        world: "client",
        with: {
            "wrpc-examples:hello/handler": generate
        },
    });
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to invoke `wrpc-examples:hello/handler.hello` on
    #[arg(default_value = "[::1]:7761")]
    addr: String,
}

struct Handler;

/// Poll and wake. This function will be redundant in WASI 0.3
#[cfg(target_family = "wasm")]
fn wake_ready(
    pollables: &mut Vec<(wasi::io::poll::Pollable, core::task::Waker)>,
    timeout: Option<core::time::Duration>,
) {
    let it = pollables.iter().map(|(p, _)| p);
    let timeout = timeout.map(|d| {
        wasi::clocks::monotonic_clock::subscribe_duration(
            d.as_nanos().try_into().unwrap_or(u64::MAX),
        )
    });
    let timeout = timeout.as_ref();
    let ps: Box<[&wasi::io::poll::Pollable]> = if let Some(timeout) = timeout {
        it.chain(core::iter::once(timeout)).collect()
    } else {
        it.collect()
    };
    tracing::trace!(?ps, "polling");
    let mut ns = wasi::io::poll::poll(&ps);
    tracing::trace!(?ns, "poll returned");
    ns.sort_by_key(|i| core::cmp::Reverse(*i));
    let mut ns = ns.as_slice();
    if let (Some(i), Some(_)) = (ns.first(), timeout) {
        if *i as usize == pollables.len() {
            tracing::debug!("polling timed out");
            ns = &ns[1..]
        }
    }
    for i in ns {
        let (_, w) = pollables.remove(*i as _);
        tracing::trace!(i, "calling `wake`");
        w.wake()
    }
}

#[cfg(target_family = "wasm")]
async fn invoke(addr: &str) -> String {
    use tokio::sync::mpsc::error::TryRecvError;

    let wrpc = wrpc_transport::tcp::Client::from(addr);
    let (pollables_tx, mut pollables_rx) = tokio::sync::mpsc::unbounded_channel();
    let (s, _) = tokio::join!(
        async {
            wrpc_bindings::wrpc_examples::hello::handler::hello(&wrpc, pollables_tx)
                .await
                .expect("failed to invoke `wrpc-examples.hello/handler.hello`")
        },
        async {
            // Drive asynchronous I/O, this will be redundant in WASI 0.3
            let mut pollables = Vec::default();
            loop {
                tracing::trace!("receiving pollable");
                match pollables_rx.try_recv() {
                    Ok((p, w)) => pollables.push((p, w)),
                    Err(TryRecvError::Empty) => {
                        tracing::trace!("pollable channel empty");
                        if pollables.is_empty() {
                            let Some((p, w)) = pollables_rx.recv().await else {
                                return;
                            };
                            pollables.push((p, w));
                            continue;
                        }
                        wake_ready(&mut pollables, Some(core::time::Duration::from_millis(10)));
                    }
                    Err(TryRecvError::Disconnected) => {
                        tracing::trace!("pollable channel disconnected");
                        while !pollables.is_empty() {
                            wake_ready(&mut pollables, None);
                        }
                    }
                }
            }
        }
    );
    s
}

#[cfg(not(target_family = "wasm"))]
async fn invoke(addr: &str) -> String {
    let wrpc = wrpc_transport::tcp::Client::from(addr);
    wrpc_bindings::wrpc_examples::hello::handler::hello(&wrpc, ())
        .await
        .expect("failed to invoke `wrpc-examples.hello/handler.hello`")
}

impl bindings::exports::wrpc_examples::hello::handler::Guest for Handler {
    fn hello() -> String {
        tracing_subscriber::fmt().init();

        let Args { addr } = Args::parse();

        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("failed to create runtime")
            .block_on(invoke(&addr))
    }
}
