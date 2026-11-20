use std::sync::Arc;

use bytes::BytesMut;
use tokio_util::codec::Decoder as _;
use wasmtime::component::{self, Component, Type, types};
use wasmtime::{Engine, bail};
use wrpc_transport::{pack, unpack};
use wrpc_wasmtime::{TupleDecoder, TupleEncoder, Val};

#[test]
fn plain_values() -> wasmtime::Result<()> {
    let engine = Engine::default();
    let component = Component::new(
        &engine,
        r#"(component
            (type $r (record (field "a" u32) (field "b" (list string))))
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
    ]);

    let tys: Arc<[Type]> = Arc::from([ty, Type::Bool]);
    let record = Val::try_from(v.clone())?;
    let vs = vec![record, Val::Bool(true)];
    let mut buf = BytesMut::default();
    pack(TupleEncoder::new(Arc::clone(&tys)), vs, &mut buf)?;
    assert_eq!(buf, b"\x2a\x01\x03foo\x01"[..]);
    let decoded = unpack(TupleDecoder::new(Arc::clone(&tys)), &mut buf)?;
    let decoded = decoded
        .into_iter()
        .map(component::Val::try_from)
        .collect::<wasmtime::Result<Vec<_>>>()?;
    assert_eq!(decoded, [v, component::Val::Bool(true)]);

    let renamed = vec![
        Val::Record(vec![
            ("c".into(), Val::U32(42)),
            ("b".into(), Val::List(Vec::default())),
        ]),
        Val::Bool(true),
    ];
    let err = pack(TupleEncoder::new(Arc::clone(&tys)), renamed, &mut buf).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);

    for buf in [&b"\x2a\x03"[..], &b"\x2a\x01\x03foo\x01"[..]] {
        let dec = TupleDecoder::new(Arc::clone(&tys)).with_max_len(2);
        let err = unpack(dec, &mut buf.into()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    let dec = TupleDecoder::new(Arc::from([flags]));
    let err = unpack(dec, &mut b"\x08"[..].into()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
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
