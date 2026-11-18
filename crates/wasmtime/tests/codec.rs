use bytes::BytesMut;
use tokio_util::codec::Decoder as _;
use wasmtime::component::{Component, Type, Val, types};
use wasmtime::{Engine, bail};
use wrpc_transport::{pack, unpack};
use wrpc_wasmtime::{ResourceTypes, TupleDecoder, TupleEncoder, ValDecoder, ValEncoder};

#[test]
fn plain_values() -> wasmtime::Result<()> {
    let engine = Engine::default();
    let component = Component::new(
        &engine,
        r#"(component
            (type $r (record (field "a" u32) (field "b" (list string))))
            (export "r" (type $r))
        )"#,
    )?;
    let Some((
        ..,
        types::ComponentExtern {
            ty: types::ComponentItem::Type(ty),
            ..
        },
    )) = component.component_type().exports(&engine).next()
    else {
        bail!("record type missing")
    };
    let v = Val::Record(vec![
        ("a".into(), Val::U32(42)),
        ("b".into(), Val::List(vec![Val::String("foo".into())])),
    ]);

    let mut buf = BytesMut::default();
    pack(
        ValEncoder::new((), &ty, ResourceTypes::default()),
        &v,
        &mut buf,
    )?;
    assert_eq!(buf, b"\x2a\x01\x03foo"[..]);
    let decoded = unpack(ValDecoder::new((), &ty, ResourceTypes::default()), &mut buf)?;
    assert_eq!(decoded, v);

    let renamed = Val::Record(vec![
        ("c".into(), Val::U32(42)),
        ("b".into(), Val::List(Vec::default())),
    ]);
    let err = pack(
        ValEncoder::new((), &ty, ResourceTypes::default()),
        &renamed,
        &mut buf,
    )
    .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);

    let tys = [ty, Type::Bool];
    let vs = [v, Val::Bool(true)];
    let mut buf = BytesMut::default();
    pack(
        TupleEncoder::new((), &tys, ResourceTypes::default()),
        &vs[..],
        &mut buf,
    )?;
    assert_eq!(buf, b"\x2a\x01\x03foo\x01"[..]);
    let decoded = unpack(
        TupleDecoder::new((), &tys, ResourceTypes::default()),
        &mut buf,
    )?;
    assert_eq!(decoded, vs);

    for buf in [&b"\x2a\x03"[..], &b"\x2a\x01\x03foo\x01"[..]] {
        let dec = TupleDecoder::new((), &tys, ResourceTypes::default()).with_max_len(2);
        let err = unpack(dec, &mut buf.into()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
    Ok(())
}

#[test]
fn clean_eof() -> wasmtime::Result<()> {
    let tys = [Type::String];
    let mut dec = TupleDecoder::new((), &tys, ResourceTypes::default());
    let mut buf = BytesMut::from(&b"\x03foo"[..]);
    let v = dec.decode(&mut buf)?;
    assert_eq!(v, Some(vec![Val::String("foo".into())]));
    let v = dec.decode(&mut buf)?;
    assert_eq!(v, None);
    let v = dec.decode_eof(&mut buf)?;
    assert_eq!(v, None);
    Ok(())
}
