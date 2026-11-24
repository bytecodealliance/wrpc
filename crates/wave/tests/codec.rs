use core::fmt::Debug;

use std::sync::Arc;

use anyhow::Context as _;
use bytes::{BufMut as _, BytesMut};
use tokio::io::{duplex, split};
use tokio::try_join;
use tokio_util::codec::{Decoder as _, Encoder, FramedRead, FramedWrite};
use wasm_wave::value::{Type, Value};
use wasm_wave::wasm::{WasmTypeKind, WasmValue as _};
use wrpc_core::{Codec, Encode, FrameDecoder, FrameEncoder, Listener};
use wrpc_wave::{DecodeError, EncodeError, TupleDecoder, TupleEncoder, ValueDecoder, ValueEncoder};

#[test]
fn roundtrip() -> anyhow::Result<()> {
    fn encode<T: Encode>(v: T) -> BytesMut
    where
        <T::Encoder as Encoder<T>>::Error: Debug,
    {
        let mut buf = BytesMut::default();
        T::Encoder::default()
            .encode(v, &mut buf)
            .expect("failed to encode");
        buf
    }

    let names = |n| (0..n).map(|i| format!("c{i}"));
    let variant = Type::variant(names(200).map(|name| {
        let ty = (name == "c150").then_some(Type::STRING);
        (name, ty)
    }))
    .context("invalid variant")?;
    let enum_ty = Type::enum_ty(names(200)).context("invalid enum")?;
    let flags_12 = Type::flags(names(12)).context("invalid flags")?;
    let flags_70 = Type::flags(names(70)).context("invalid flags")?;
    let record = Type::record([("a", Type::U32), ("b", Type::STRING)]).context("invalid record")?;
    let tuple = Type::tuple([Type::S8, Type::BOOL]).context("invalid tuple")?;
    let payload = Type::tuple([Type::U8, Type::STRING]).context("invalid tuple")?;
    let case = Type::variant([("a", None), ("b", Some(payload))]).context("invalid variant")?;
    let map = Type::map(Type::STRING, Type::U32).context("invalid map")?;
    let nested = Type::record([
        ("x", Type::option(case)),
        ("y", Type::result(Some(Type::list(Type::STRING)), None)),
    ])
    .context("invalid record")?;
    for (ty, s, expected) in [
        (Type::BOOL, "true", encode(true)),
        (Type::S8, "-42", encode(-42i8)),
        (Type::U8, "255", encode(u8::MAX)),
        (Type::S16, "-32768", encode(i16::MIN)),
        (Type::U16, "65535", encode(u16::MAX)),
        (Type::S32, "-100000", encode(-100_000i32)),
        (Type::U32, "4294967295", encode(u32::MAX)),
        (Type::S64, "-9223372036854775808", encode(i64::MIN)),
        (Type::U64, "18446744073709551615", encode(u64::MAX)),
        (Type::F32, "1.5", encode(1.5f32)),
        (Type::F64, "-2.5", encode(-2.5f64)),
        (Type::CHAR, "'😀'", encode('😀')),
        (Type::STRING, r#""hello 😀""#, encode("hello 😀")),
        (Type::list(Type::U32), "[1, 300]", encode(vec![1u32, 300])),
        (
            map,
            r#"{"b": 300, "a": 1}"#,
            encode(vec![("b", 300u32), ("a", 1)]),
        ),
        (record, r#"{a: 42, b: "foo"}"#, encode((42u32, "foo"))),
        (tuple, "(-1, false)", encode((-1i8, false))),
        (Type::option(Type::U32), "none", encode(None::<u32>)),
        (Type::option(Type::U32), "some(42)", encode(Some(42u32))),
        (
            Type::result(Some(Type::U32), Some(Type::STRING)),
            r#"err("x")"#,
            encode(Err::<u32, _>("x")),
        ),
        (Type::result(None, None), "ok", encode(Ok::<_, ()>(()))),
        (variant.clone(), r#"c150("x")"#, encode((150u32, "x"))),
        (variant, "c199", encode(199u32)),
        (enum_ty, "c199", encode(199u32)),
        (flags_12, "{c0, c9}", BytesMut::from(&b"\x01\x02"[..])),
        (
            flags_70,
            "{c0, c8, c69}",
            BytesMut::from(&b"\x01\x01\0\0\0\0\0\0\x20"[..]),
        ),
        (
            Type::list(nested),
            r#"[{x: some(b((7, "a"))), y: ok(["b"])}, {x: none, y: err}]"#,
            encode(vec![
                (Some((1u32, (7u8, "a"))), Ok(vec!["b"])),
                (None, Err(())),
            ]),
        ),
    ] {
        let v: Value = wasm_wave::from_str(&ty, s)?;
        let mut buf = BytesMut::default();
        ValueEncoder::new(ty.clone()).encode(&v, &mut buf)?;
        assert_eq!(buf, expected, "{s}");

        let mut src = buf.clone();
        let got = ValueDecoder::new(ty.clone()).decode(&mut src)?;
        assert_eq!(got, Some(v.clone()), "{s}");
        assert!(src.is_empty());

        let (last, init) = buf.split_last().context("empty encoding")?;
        let mut dec = ValueDecoder::new(ty);
        for b in init {
            src.put_u8(*b);
            let got = dec.decode(&mut src)?;
            assert_eq!(got, None, "{s}");
        }
        src.put_u8(*last);
        let got = dec.decode(&mut src)?;
        assert_eq!(got, Some(v), "{s}");
        assert!(src.is_empty());
        let got = dec.decode(&mut src)?;
        assert_eq!(got, None, "{s}");
        let got = dec.decode_eof(&mut src)?;
        assert_eq!(got, None, "{s}");
    }
    Ok(())
}

#[test]
fn errors() -> anyhow::Result<()> {
    let enum_ty = Type::enum_ty(["a", "b"]).context("invalid enum")?;
    let map = Type::map(Type::STRING, Type::U32).context("invalid map")?;
    let fixed = Type::fixed_length_list(Type::U8, 2);
    let flags = Type::flags(["a", "b", "c"]).context("invalid flags")?;
    for (ty, src, expected) in [
        (
            Type::BOOL,
            &b"\x02"[..],
            (|err| matches!(err, DecodeError::Primitive(..))) as fn(&DecodeError) -> bool,
        ),
        (Type::STRING, b"\x01\xff", |err| {
            matches!(err, DecodeError::Primitive(..))
        }),
        (Type::option(Type::U32), b"\x02", |err| {
            matches!(err, DecodeError::UnknownDiscriminant(2))
        }),
        (enum_ty, b"\x02", |err| {
            matches!(err, DecodeError::UnknownDiscriminant(2))
        }),
        (flags, b"\x08", |err| {
            matches!(err, DecodeError::InvalidFlags)
        }),
        (map.clone(), b"\x02\x01a\x01\x01a\x02", |err| {
            matches!(err, DecodeError::DuplicateKey)
        }),
        (Type::list(Type::STRING), b"\x01\x03ab", |err| {
            matches!(err, DecodeError::Truncated)
        }),
        (Type::U32, b"\x80", |err| {
            matches!(err, DecodeError::Truncated)
        }),
        (fixed, b"\0\0", |err| {
            matches!(err, DecodeError::UnsupportedType(..))
        }),
    ] {
        let err = ValueDecoder::<Value>::new(ty.clone())
            .decode_eof(&mut BytesMut::from(src))
            .expect_err("decoding should fail");
        let err = err.downcast::<DecodeError>().expect("typed error");
        assert!(expected(&err), "{ty}: {err:?}");
    }

    let tys: Arc<[Type]> = Arc::from([Type::list(Type::U8), Type::STRING]);
    let v = TupleDecoder::<Value>::new(Arc::clone(&tys))
        .with_max_len(2)
        .decode(&mut BytesMut::from(&b"\x02\x01\x02\x02ab"[..]))?;
    let list = Value::make_list(&tys[0], [Value::make_u8(1), Value::make_u8(2)])?;
    assert_eq!(v, Some(vec![list, Value::make_string("ab".into())]));
    let err = ValueDecoder::<Value>::new(map.clone())
        .with_max_len(2)
        .decode(&mut BytesMut::from(&b"\x03"[..]))
        .expect_err("decoding should fail");
    let err = err.downcast::<DecodeError>().expect("typed error");
    assert!(
        matches!(err, DecodeError::TooLong { len: 3, max: 2 }),
        "{err:?}"
    );
    for src in [&b"\x03"[..], b"\x00\x03foo"] {
        let err = TupleDecoder::<Value>::new(Arc::clone(&tys))
            .with_max_len(2)
            .decode(&mut BytesMut::from(src))
            .expect_err("decoding should fail");
        let err = err.downcast::<DecodeError>().expect("typed error");
        assert!(
            matches!(err, DecodeError::TooLong { len: 3, max: 2 }),
            "{err:?}"
        );
    }

    let mut src = BytesMut::from(&b"\x2a\x63"[..]);
    let v = ValueDecoder::new(Type::U8).decode(&mut src)?;
    assert_eq!(v, Some(Value::make_u8(42)));
    assert_eq!(src, b"\x63"[..]);

    let mut buf = BytesMut::default();
    let err = ValueEncoder::new(Type::STRING)
        .encode(&Value::make_u32(42), &mut buf)
        .expect_err("encoding should fail");
    let err = err.downcast::<EncodeError>().expect("typed error");
    assert!(
        matches!(
            err,
            EncodeError::TypeMismatch {
                expected: WasmTypeKind::String,
                got: WasmTypeKind::U32
            }
        ),
        "{err:?}"
    );
    let ab = Type::record([("a", Type::U32), ("b", Type::U32)]).context("invalid record")?;
    let ba = Type::record([("b", Type::U32), ("a", Type::U32)]).context("invalid record")?;
    let v = Value::make_record(&ab, [("a", Value::make_u32(1)), ("b", Value::make_u32(2))])?;
    let err = ValueEncoder::new(ba)
        .encode(&v, &mut buf)
        .expect_err("encoding should fail");
    let err = err.downcast::<EncodeError>().expect("typed error");
    assert!(matches!(err, EncodeError::FieldMismatch { .. }), "{err:?}");
    let dup = Value::make_map(
        &map,
        [
            (Value::make_string("a".into()), Value::make_u32(1)),
            (Value::make_string("a".into()), Value::make_u32(2)),
        ],
    )?;
    let err = ValueEncoder::new(map)
        .encode(&dup, &mut buf)
        .expect_err("encoding should fail");
    let err = err.downcast::<EncodeError>().expect("typed error");
    assert!(matches!(err, EncodeError::DuplicateKey), "{err:?}");
    let err = TupleEncoder::new(Arc::from([Type::U32]))
        .encode(&[] as &[Value], &mut buf)
        .expect_err("encoding should fail");
    let err = err.downcast::<EncodeError>().expect("typed error");
    assert!(
        matches!(
            err,
            EncodeError::LengthMismatch {
                expected: 1,
                got: 0
            }
        ),
        "{err:?}"
    );
    Ok(())
}

#[tokio::test]
async fn invoke_tuple() -> anyhow::Result<()> {
    let param_tys: Arc<[Type]> = Arc::from([
        Type::U32,
        Type::STRING,
        Type::list(Type::option(Type::BOOL)),
    ]);
    let result_tys: Arc<[Type]> = Arc::from([Type::result(Some(Type::U64), None), Type::CHAR]);
    let option_ty = Type::option(Type::BOOL);
    let none = Value::make_option(&option_ty, None)?;
    let some = Value::make_option(&option_ty, Some(Value::make_bool(true)))?;
    let list = Value::make_list(&param_tys[2], [none, some])?;
    let params = vec![Value::make_u32(42), Value::make_string("foo".into()), list];
    let result = Value::make_result(&result_tys[0], Ok(Some(Value::make_u64(u64::MAX))))?;
    let results = vec![result, Value::make_char('😀')];

    let (clt, srv) = duplex(64);
    let (clt_rx, clt_tx) = split(clt);
    let clt_tx = FramedWrite::new(clt_tx, FrameEncoder);
    let clt_rx = FramedRead::new(clt_rx, FrameDecoder::default());
    let (srv_rx, srv_tx) = split(srv);
    let (got, ()) = try_join!(
        async {
            let (got, io) = Codec {
                enc: TupleEncoder::new(Arc::clone(&param_tys)),
                dec: TupleDecoder::<Value>::new(Arc::clone(&result_tys)),
            }
            .invoke(clt_tx, clt_rx, "foo", "bar", params.as_slice())
            .await?;
            io.await?;
            anyhow::Ok(got)
        },
        async {
            let mut lis = Listener::new(srv_tx, srv_rx);
            let next = lis.accept().await?;
            let (header, srv) = next.context("connection closed")?;
            assert_eq!(header.instance, "foo");
            assert_eq!(header.name, "bar");
            let codec = Codec {
                enc: TupleEncoder::new(Arc::clone(&result_tys)),
                dec: TupleDecoder::<Value>::new(Arc::clone(&param_tys)),
            };
            codec
                .serve(&mut srv.tx, &mut srv.rx, async |got| {
                    assert_eq!(got, params);
                    Ok(results.as_slice())
                })
                .await?;
            anyhow::Ok(())
        },
    )?;
    assert_eq!(got, results);
    Ok(())
}
