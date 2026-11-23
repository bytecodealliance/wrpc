use tokio::try_join;
use wrpc::{Framed, Header, Listener};

#[tokio::test]
async fn generate() -> anyhow::Result<()> {
    mod bindings {
        wrpc::generate!({
            inline: "
                package wrpc-test:umbrella;

                interface greeter {
                    greet: func(name: string) -> string;
                }

                world umbrella {
                    import greeter;
                    export greeter;
                }
            ",
        });
    }

    #[derive(Clone)]
    struct Handler;

    impl bindings::exports::wrpc_test::umbrella::greeter::Handler for Handler {
        async fn greet(&self, name: String) -> anyhow::Result<String> {
            Ok(format!("hello, {name}"))
        }
    }

    let (clt, srv) = tokio::io::duplex(1024);
    let (srv_rx, srv_tx) = tokio::io::split(srv);
    let (clt_rx, clt_tx) = tokio::io::split(clt);
    let (.., greeting) = try_join!(
        async {
            let mut lis = Listener::new(srv_tx, srv_rx);
            let Some((Header { instance, name }, srv)) = lis.accept().await? else {
                anyhow::bail!("connection closed");
            };
            let Ok(fut) = bindings::handle(&Handler, &instance, &name, srv) else {
                anyhow::bail!("`{instance}#{name}` is not exported");
            };
            fut.await?;
            Ok(())
        },
        bindings::wrpc_test::umbrella::greeter::greet(Framed::new(clt_tx, clt_rx), "wrpc"),
    )?;
    assert_eq!(greeting, "hello, wrpc");
    Ok(())
}
