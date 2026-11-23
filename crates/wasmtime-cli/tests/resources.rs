use core::time::Duration;

use std::io::Write as _;

use bytes::Bytes;
use tokio::net::{TcpListener, TcpStream};
use tokio::select;
use wrpc_core::{Framed, invoke_blocking};

const COMPONENT: &str = r#"
(component
  (type $r (resource (rep i32)))
  (type $s (resource (rep i32)))
  (core func $new-r (canon resource.new $r))
  (core func $new-s (canon resource.new $s))
  (core module $m
    (import "" "new-r" (func $new-r (param i32) (result i32)))
    (import "" "new-s" (func $new-s (param i32) (result i32)))
    (func (export "new-r") (param i32) (result i32)
      (call $new-r (local.get 0)))
    (func (export "new-s") (param i32) (result i32)
      (call $new-s (local.get 0)))
    (func (export "get") (param i32) (result i32)
      local.get 0)
    (func (export "consume") (param i32))
    (func (export "both") (param i32 i32)))
  (core instance $i (instantiate $m (with "" (instance
    (export "new-r" (func $new-r))
    (export "new-s" (func $new-s))))))
  (export $r' "r" (type $r))
  (export $s' "s" (type $s))
  (func (export "[constructor]r") (param "v" u32) (result (own $r'))
    (canon lift (core func $i "new-r")))
  (func (export "[constructor]s") (param "v" u32) (result (own $s'))
    (canon lift (core func $i "new-s")))
  (func (export "[method]r.get") (param "self" (borrow $r')) (result u32)
    (canon lift (core func $i "get")))
  (func (export "[static]r.consume") (param "r" (own $r'))
    (canon lift (core func $i "consume")))
  (func (export "[static]r.both") (param "a" (borrow $r')) (param "b" (own $r'))
    (canon lift (core func $i "both")))
)"#;

#[tokio::test]
async fn invalid_resource_handles() -> anyhow::Result<()> {
    let mut wasm = tempfile::NamedTempFile::new()?;
    wasm.write_all(COMPONENT.as_bytes())?;
    let workload = wasm.path().to_str().unwrap();
    let lis = TcpListener::bind("127.0.0.1:0").await?;
    let addr = lis.local_addr()?;
    let connect = async || {
        let stream = TcpStream::connect(addr).await?;
        let (rx, tx) = stream.into_split();
        anyhow::Ok(Framed::new(tx, rx))
    };
    let srv = wrpc_wasmtime_cli::serve(
        lis,
        Duration::from_secs(10),
        u32::MAX,
        Vec::default(),
        workload,
    );
    let clt = async {
        let mut clt = connect().await?;
        let (r,) = invoke_blocking::<(u32,), (Bytes,)>(&mut clt, "", "r", (42,)).await?;
        invoke_blocking::<(Bytes,), ()>(&mut clt, "", "r.consume", (r.clone(),)).await?;
        invoke_blocking::<(Bytes,), ()>(clt, "", "r.consume", (r,))
            .await
            .expect_err("consumed resource accepted");

        let mut clt = connect().await?;
        let (r,) = invoke_blocking::<(u32,), (Bytes,)>(&mut clt, "", "r", (42,)).await?;
        let (s,) = invoke_blocking::<(u32,), (Bytes,)>(&mut clt, "", "s", (42,)).await?;
        invoke_blocking::<(Bytes,), (u32,)>(clt, "", "r.get", (s,))
            .await
            .expect_err("resource of a different type accepted");

        let clt = connect().await?;
        invoke_blocking::<(Bytes, Bytes), ()>(clt, "", "r.both", (r.clone(), r.clone()))
            .await
            .expect_err("borrowed resource moved");

        let clt = connect().await?;
        let (v,) = invoke_blocking::<(Bytes,), (u32,)>(clt, "", "r.get", (r,)).await?;
        assert_eq!(v, 42);
        anyhow::Ok(())
    };
    select! {
        res = srv => panic!("server stopped: {res:?}"),
        res = clt => res,
    }
}
