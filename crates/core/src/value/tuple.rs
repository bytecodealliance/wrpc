use core::fmt::Debug;
use core::task::{Context, Poll};

use std::vec;

use bytes::BytesMut;
use tokio_util::codec::Decoder as _;
use tracing::instrument;
use wasm_tokio::cm::{TupleDecoder, TupleEncoder};
use wasm_tokio::{CoreVecDecoder, Leb128DecoderU32};

use crate::{Decode, Encode, ListDecoder, Receive, ReceiveError, Transmit, error_unknown_path};

/// Codec for `()`
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
pub struct UnitCodec;

impl tokio_util::codec::Encoder<()> for UnitCodec {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), ret)]
    fn encode(&mut self, (): (), dst: &mut BytesMut) -> std::io::Result<()> {
        Ok(())
    }
}

impl tokio_util::codec::Encoder<&()> for UnitCodec {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), ret)]
    fn encode(&mut self, (): &(), dst: &mut BytesMut) -> std::io::Result<()> {
        Ok(())
    }
}

impl tokio_util::codec::Decoder for UnitCodec {
    type Item = ();
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        Ok(Some(()))
    }
}

impl Encode for () {
    type Encoder = UnitCodec;
}

impl Decode for () {
    type Decoder = UnitCodec;
    type ListDecoder = CoreVecDecoder<Self::Decoder>;

    fn decode_chunk(
        _: &mut ListDecoder<Self::Decoder>,
        src: &mut BytesMut,
    ) -> std::io::Result<Option<Vec<Self>>> {
        let Some(len) = Leb128DecoderU32.decode(src)? else {
            return Ok(None);
        };
        let len = usize::try_from(len)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        Ok(Some(vec![(); len]))
    }
}

macro_rules! impl_tuple_codec {
    (@impl $(($vt:ident $ct:ident $cn:ident $i:tt))+) => {
        impl<$($ct: Transmit),+> Transmit for TupleEncoder::<($($ct),+,)> {
            fn poll_transmit(
                &mut self,
                cx: &mut Context<'_>,
                path: &mut Vec<usize>,
                dst: &mut BytesMut,
            ) -> Poll<std::io::Result<()>> {
                let ($($cn),+,) = &mut self.0;
                let mut ready = true;
                $(
                    path.push($i);
                    let res = $cn.poll_transmit(cx, path, dst);
                    path.pop();
                    let res = res?;
                    ready &= res.is_ready();
                )+
                if ready {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            }

            fn is_complete(&self) -> bool {
                let ($($cn),+,) = &self.0;
                true $(&& $cn.is_complete())+
            }
        }

        impl<$($vt),+> Encode for ($($vt),+,)
        where
            $(
                $vt: Encode,
            )+
        {
            type Encoder = TupleEncoder::<($($vt::Encoder),+,)>;
        }

        impl<'a, $($vt),+> Encode for &'a ($($vt),+,)
        where
            $(
                $vt: Encode,
                $vt::Encoder: tokio_util::codec::Encoder<&'a $vt, Error = std::io::Error>,
            )+
        {
            type Encoder = TupleEncoder::<($($vt::Encoder),+,)>;
        }

        impl<$($ct: Receive),+, V> Receive for TupleDecoder<($($ct),+,), V> {
            fn poll_receive(
                &mut self,
                cx: &mut Context<'_>,
                path: &[usize],
                src: &mut BytesMut,
            ) -> Poll<Result<(), ReceiveError>> {
                let Some((i, rest)) = path.split_first() else {
                    return Poll::Ready(Err(error_unknown_path(path).into()));
                };
                let ($($cn),+,) = self.get_mut();
                match i {
                    $($i => $cn.poll_receive(cx, rest, src),)+
                    _ => Poll::Ready(Err(error_unknown_path(path).into())),
                }
            }

            fn is_complete(&self) -> bool {
                let ($($cn),+,) = self.get_ref();
                true $(&& $cn.is_complete())+
            }
        }

        impl<$($vt),+> Decode for ($($vt),+,)
        where
            $(
                $vt: Decode + 'static,
            )+
        {
            type Decoder = TupleDecoder::<($($vt::Decoder),+,), ($(Option<$vt>),+,)>;
            type ListDecoder = ListDecoder<Self::Decoder>;
        }
    };
    ([$($acc:tt)*]) => {};
    ([$($acc:tt)*] $next:tt $($rest:tt)*) => {
        impl_tuple_codec!(@impl $($acc)* $next);
        impl_tuple_codec!([$($acc)* $next] $($rest)*);
    };
}

impl_tuple_codec!(
    []
    (V0 C0 c0 0)
    (V1 C1 c1 1)
    (V2 C2 c2 2)
    (V3 C3 c3 3)
    (V4 C4 c4 4)
    (V5 C5 c5 5)
    (V6 C6 c6 6)
    (V7 C7 c7 7)
    (V8 C8 c8 8)
    (V9 C9 c9 9)
    (V10 C10 c10 10)
    (V11 C11 c11 11)
);
