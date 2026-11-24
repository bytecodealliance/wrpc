use std::sync::Arc;

use bytes::BytesMut;
use tokio_util::codec::Decoder as _;
use wasmtime::component::{self, Component, Type, types};
use wasmtime::{Config, Engine, bail};
use wrpc_core::{PackError, UnpackError, pack_with, unpack_with};
use wrpc_wasmtime::{TupleDecoder, TupleEncoder, Val, ValDecoder, ValEncoder};

#[test]
fn plain_values() -> wasmtime::Result<()> {
    let mut config = Config::default();
    config.wasm_component_model_map(true);
    config.wasm_component_model_fixed_length_lists(true);
    let engine = Engine::new(&config)?;
    let component = Component::new(
        &engine,
        r#"(component
            (type $r (record
                (field "a" u32)
                (field "b" (list string))
                (field "c" (map string u8))
                (field "d" (list u8 2))))
            (export "r" (type $r))
            (type $f (flags "a" "b" "c"))
            (export "f" (type $f))
        )"#,
    )?;
    let component_ty = component.component_type();
    let mut exports = component_ty.exports(&engine);
    let Some((
        ..,
        types::ComponentExtern {
            ty: types::ComponentItem::Type(ty),
            ..
        },
    )) = exports.next()
    else {
        bail!("record type missing")
    };
    let Some((
        ..,
        types::ComponentExtern {
            ty: types::ComponentItem::Type(flags),
            ..
        },
    )) = exports.next()
    else {
        bail!("flags type missing")
    };
    let v = component::Val::Record(vec![
        ("a".into(), component::Val::U32(42)),
        (
            "b".into(),
            component::Val::List(vec![component::Val::String("foo".into())]),
        ),
        (
            "c".into(),
            component::Val::Map(vec![(
                component::Val::String("k".into()),
                component::Val::U8(7),
            )]),
        ),
        (
            "d".into(),
            component::Val::FixedLengthList(vec![component::Val::U8(1), component::Val::U8(2)]),
        ),
    ]);

    let tys: Arc<[Type]> = Arc::from([ty.clone(), Type::Bool]);
    let record = Val::try_from(v.clone())?;
    let vs = vec![record, Val::Bool(true)];
    let mut buf = BytesMut::default();
    pack_with(TupleEncoder::new(Arc::clone(&tys)), vs, &mut buf)?;
    assert_eq!(buf, b"\x2a\x01\x03foo\x01\x01k\x07\x01\x02\x01"[..]);
    let mut borrowed = BytesMut::default();
    let vs = [v.clone(), component::Val::Bool(true)];
    pack_with(TupleEncoder::new(Arc::clone(&tys)), &vs[..], &mut borrowed)?;
    assert_eq!(borrowed, buf);
    let decoded = unpack_with(TupleDecoder::new(Arc::clone(&tys)), &mut buf)?;
    let decoded = decoded
        .into_iter()
        .map(component::Val::try_from)
        .collect::<wasmtime::Result<Vec<_>>>()?;
    assert_eq!(decoded, [v.clone(), component::Val::Bool(true)]);

    pack_with(ValEncoder::new(ty.clone()), &v, &mut buf)?;
    assert_eq!(buf, b"\x2a\x01\x03foo\x01\x01k\x07\x01\x02"[..]);
    let decoded = unpack_with(ValDecoder::new(ty), &mut buf)?;
    let decoded = component::Val::try_from(decoded)?;
    assert_eq!(decoded, v);

    let record = |a: &str, c| {
        vec![
            Val::Record(vec![
                (a.into(), Val::U32(42)),
                ("b".into(), Val::List(Vec::default())),
                ("c".into(), Val::Map(c)),
                (
                    "d".into(),
                    Val::FixedLengthList(vec![Val::U8(1), Val::U8(2)]),
                ),
            ]),
            Val::Bool(true),
        ]
    };
    let renamed = record("x", Vec::default());
    let duplicate = record(
        "a",
        vec![
            (Val::String("k".into()), Val::U8(1)),
            (Val::String("k".into()), Val::U8(2)),
        ],
    );
    for vs in [renamed, duplicate] {
        let err = pack_with(TupleEncoder::new(Arc::clone(&tys)), vs, &mut buf).unwrap_err();
        assert!(
            matches!(&err, PackError::Encode(err) if err.kind() == std::io::ErrorKind::InvalidInput),
            "{err:?}"
        );
    }

    for buf in [
        &b"\x2a\x03"[..],
        &b"\x2a\x01\x03foo\x01"[..],
        &b"\x2a\x00\x02\x01k\x01\x01k\x02\x01\x02\x01"[..],
    ] {
        let dec = TupleDecoder::new(Arc::clone(&tys)).with_max_len(2);
        let err = unpack_with(dec, &mut buf.into()).unwrap_err();
        assert!(
            matches!(&err, UnpackError::Decode(err) if err.kind() == std::io::ErrorKind::InvalidData),
            "{err:?}"
        );
    }

    let v = component::Val::Flags(vec!["x".into()]);
    let err = pack_with(ValEncoder::new(flags.clone()), &v, &mut buf).unwrap_err();
    assert!(
        matches!(&err, PackError::Encode(err) if err.kind() == std::io::ErrorKind::InvalidInput),
        "{err:?}"
    );

    let dec = TupleDecoder::new(Arc::from([flags]));
    let err = unpack_with(dec, &mut b"\x08"[..].into()).unwrap_err();
    assert!(
        matches!(&err, UnpackError::Decode(err) if err.kind() == std::io::ErrorKind::InvalidData),
        "{err:?}"
    );
    Ok(())
}

#[test]
fn clean_eof() -> wasmtime::Result<()> {
    let mut dec = TupleDecoder::new(Arc::from([Type::String]));
    let mut buf = BytesMut::from(&b"\x03foo"[..]);
    let v = dec.decode(&mut buf)?;
    assert!(
        matches!(v.as_deref(), Some([Val::String(v)]) if v == "foo"),
        "{v:?}"
    );
    let v = dec.decode(&mut buf)?;
    assert!(v.is_none());
    let v = dec.decode_eof(&mut buf)?;
    assert!(v.is_none());
    Ok(())
}
