use anyhow::Context as _;
use tokio::io::{duplex, split};
use tokio::try_join;
use tokio_util::codec::{FramedRead, FramedWrite};
use wasm_wave::value::{Type, Value};
use wasm_wave::wasm::WasmValue as _;
use wrpc_transport::{Codec, FrameDecoder, FrameEncoder, Listener};
use wrpc_wave::{TupleDecoder, TupleEncoder};

#[tokio::test]
async fn invoke_tuple() -> anyhow::Result<()> {
    let param_tys = [
        Type::U32,
        Type::STRING,
        Type::list(Type::option(Type::BOOL)),
    ];
    let result_tys = [Type::result(Some(Type::U64), None), Type::CHAR];
    let params = vec![
        Value::make_u32(42),
        Value::make_string("foo".into()),
        Value::make_list(
            &param_tys[2],
            [
                Value::make_option(&Type::option(Type::BOOL), None)?,
                Value::make_option(&Type::option(Type::BOOL), Some(Value::make_bool(true)))?,
            ],
        )?,
    ];
    let results = vec![
        Value::make_result(&result_tys[0], Ok(Some(Value::make_u64(u64::MAX))))?,
        Value::make_char('😀'),
    ];

    let (clt, srv) = duplex(64);
    let (clt_rx, clt_tx) = split(clt);
    let clt_tx = FramedWrite::new(clt_tx, FrameEncoder);
    let clt_rx = FramedRead::new(clt_rx, FrameDecoder::default());
    let (srv_rx, srv_tx) = split(srv);
    let (got, ()) = try_join!(
        async {
            let (got, io) = Codec {
                enc: TupleEncoder::new(&param_tys),
                dec: TupleDecoder::<Value>::new(&result_tys),
            }
            .invoke(clt_tx, clt_rx, "foo", "bar", params.as_slice())
            .await?;
            io.await?;
            anyhow::Ok(got)
        },
        async {
            let mut lis = Listener::new(srv_tx, srv_rx);
            let next = lis.accept().await?;
            let (header, tx, rx) = next.context("connection closed")?;
            assert_eq!(header.instance, "foo");
            assert_eq!(header.name, "bar");
            Codec {
                enc: TupleEncoder::new(&result_tys),
                dec: TupleDecoder::<Value>::new(&param_tys),
            }
            .serve(tx, rx, async |got| {
                assert_eq!(got, params);
                anyhow::Ok(results.as_slice())
            })
            .await
        },
    )?;
    assert_eq!(got, results);
    Ok(())
}
