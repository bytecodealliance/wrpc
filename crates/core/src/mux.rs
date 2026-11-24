use core::borrow::BorrowMut;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll, ready};

use bytes::BytesMut;
use futures::{Sink, StreamExt as _, TryStreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};
use tracing::debug;

use crate::{
    FrameDecoder, FrameEncoder, FrameRef, Receive, ReceiveError, Transmit, encode_close,
    encode_frames,
};

/// Receives a value from the frames read by `rx`, decoding it using `dec`
///
/// Returns the value and the [Receipt] receiving the asynchronous values nested in it. Ingress
/// fails on a frame on a path, which is not pending, on data after the end of a value, on `rx`
/// ending before all paths are closed, on an error of `rx` and, with [`ReceiveError::Unread`], on
/// data for an asynchronous value whose consumer was dropped. A decoder, which cannot take data,
/// such as one of a stream whose chunks are not consumed, stalls ingress, so the asynchronous
/// values must be consumed concurrently with each other and with the [Receipt], see the
/// [crate docs](crate).
pub async fn receive<D, F, I>(
    mut dec: D,
    mut rx: F,
) -> Result<(D::Item, Receipt<D, F, I>), ReceiveError>
where
    F: BorrowMut<FramedRead<I, FrameDecoder>>,
    I: AsyncRead + Unpin,
    D: Decoder<Error = std::io::Error> + Receive,
{
    let mut buf = BytesMut::default();
    let mut item = None;
    let (item, data) = loop {
        let rx = rx.borrow_mut();
        let Some(data) = rx.try_next().await? else {
            return Err(ReceiveError::Truncated);
        };
        let path = rx.decoder().path();
        if !path.is_empty() {
            let Some(item) = item else {
                return Err(ReceiveError::UnknownPath);
            };
            break (item, Some(data));
        }
        let closed = data.is_empty();
        if item.is_none() {
            buf.unsplit(data);
            item = if closed {
                dec.decode_eof(&mut buf)?
            } else {
                dec.decode(&mut buf)?
            };
            if item.is_some() && !buf.is_empty() {
                return Err(ReceiveError::DataAfterEnd);
            }
        } else if !closed {
            return Err(ReceiveError::DataAfterEnd);
        }
        if closed {
            let Some(item) = item else {
                return Err(ReceiveError::Truncated);
            };
            break (item, None);
        }
    };
    Ok((
        item,
        Receipt {
            rx,
            dec,
            closed: data.is_none(),
            data,
            _rx: PhantomData,
        },
    ))
}

/// Future receiving the asynchronous values nested in a value
///
/// Resolves once all paths are closed, stalls while the consumer of a received frame does not
/// take it.
#[must_use]
pub struct Receipt<D, F, I> {
    rx: F,
    dec: D,
    closed: bool,
    /// Data of the last frame, until its decoder took it
    data: Option<BytesMut>,
    _rx: PhantomData<fn() -> I>,
}

impl<D, F, I> Unpin for Receipt<D, F, I> {}

impl<D, F, I> Receipt<D, F, I> {
    /// Returns the frame reader, past the frame closing the last open path once this resolved
    pub fn into_inner(self) -> F {
        self.rx
    }
}

impl<D, F, I> Future for Receipt<D, F, I>
where
    F: BorrowMut<FramedRead<I, FrameDecoder>>,
    I: AsyncRead + Unpin,
    D: Receive,
{
    type Output = Result<(), ReceiveError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self {
            rx,
            dec,
            closed,
            data,
            ..
        } = self.get_mut();
        let rx = rx.borrow_mut();
        loop {
            if let Some(buf) = data {
                ready!(dec.poll_receive(cx, rx.decoder().path(), buf))?;
                if !buf.is_empty() {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "decoder did not take all data",
                    )
                    .into()));
                }
                *data = None;
            }
            if *closed && dec.is_complete() {
                debug!("ingress complete");
                return Poll::Ready(Ok(()));
            }
            let Some(buf) = ready!(rx.poll_next_unpin(cx)).transpose()? else {
                return Poll::Ready(Err(ReceiveError::Truncated));
            };
            if !rx.decoder().path().is_empty() {
                *data = Some(buf);
            } else if *closed || !buf.is_empty() {
                return Poll::Ready(Err(ReceiveError::DataAfterEnd));
            } else {
                *closed = true;
            }
        }
    }
}

/// Sends a value, the parameters when invoking and the results when serving, encoding it using
/// `enc` into the frames written by `tx`, which may be borrowed
///
/// The value is encoded before this returns. Returns the [Transmission] of the value and of the
/// asynchronous values nested in it.
pub fn transmit<E, W, O, T>(
    mut enc: E,
    mut tx: W,
    value: T,
) -> std::io::Result<Transmission<E, W, O>>
where
    E: Encoder<T, Error = std::io::Error> + Transmit,
    W: BorrowMut<FramedWrite<O, FrameEncoder>>,
    O: AsyncWrite + Unpin,
{
    let mut data = BytesMut::default();
    enc.encode(value, &mut data)?;
    let buf = tx.borrow_mut().write_buffer_mut();
    encode_frames(&[], data, &mut *buf)?;
    encode_close(&[], buf)?;
    Ok(Transmission {
        tx,
        path: Vec::default(),
        enc,
        _tx: PhantomData,
    })
}

/// Future transmitting a value and the asynchronous values nested in it, see [`transmit`]
///
/// Resolves once all paths are closed and all data was flushed, `tx` is neither shut down nor
/// dropped before.
#[must_use]
pub struct Transmission<E, W, O> {
    tx: W,
    path: Vec<usize>,
    enc: E,
    _tx: PhantomData<fn() -> O>,
}

impl<E, W, O> Unpin for Transmission<E, W, O> {}

impl<E, W, O> Future for Transmission<E, W, O>
where
    W: BorrowMut<FramedWrite<O, FrameEncoder>>,
    O: AsyncWrite + Unpin,
    E: Transmit,
{
    type Output = std::io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self { tx, path, enc, .. } = self.get_mut();
        let tx = tx.borrow_mut();
        loop {
            ready!(Sink::<FrameRef<'_>>::poll_ready(Pin::new(&mut *tx), cx))?;
            let buf = tx.write_buffer_mut();
            let n = buf.len();
            match enc.poll_transmit(cx, path, buf) {
                Poll::Ready(Ok(())) => return Sink::<FrameRef<'_>>::poll_flush(Pin::new(tx), cx),
                Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                Poll::Pending if buf.len() == n => {
                    ready!(Sink::<FrameRef<'_>>::poll_flush(Pin::new(tx), cx))?;
                    return Poll::Pending;
                }
                Poll::Pending => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use futures::stream::BoxStream;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, duplex};
    use tokio::try_join;
    use tokio_util::codec::FramedRead;

    use super::*;
    use crate::{Decode, FrameDecodeError, RawCodec};

    fn frame(path: &[usize], data: &[u8]) -> Vec<u8> {
        let mut buf = BytesMut::default();
        FrameEncoder
            .encode(FrameRef { path, data }, &mut buf)
            .unwrap();
        buf.to_vec()
    }

    async fn receive_stream(
        rx: &mut FramedRead<&[u8], FrameDecoder>,
    ) -> Result<Vec<u8>, ReceiveError> {
        let dec = <(BoxStream<'static, Bytes>,) as Decode>::Decoder::default();
        let ((s,), rx) = receive(dec, rx).await?;
        let (buf, ()) = try_join!(
            async { Ok(s.map(|chunk| chunk.to_vec()).concat().await) },
            rx
        )?;
        Ok(buf)
    }

    #[test_log::test(tokio::test)]
    async fn ingress_frames() {
        let frames = [
            frame(&[], b"\0"),
            frame(&[0], b"\x03foo"),
            frame(&[], b""),
            frame(&[0], b"\x03b"),
            frame(&[0], b"ar"),
            frame(&[0], b""),
            b"\x2a".to_vec(),
        ]
        .concat();
        let mut rx = FramedRead::new(frames.as_slice(), FrameDecoder::default());
        let buf = receive_stream(&mut rx).await.unwrap();
        assert_eq!(buf, b"foobar");
        assert_eq!(rx.read_buffer().as_ref(), b"\x2a");
    }

    #[test_log::test(tokio::test)]
    async fn ingress_raw() {
        let frames = [frame(&[], b"foo"), frame(&[], b"bar"), frame(&[], b"")].concat();
        let rx = FramedRead::new(frames.as_slice(), FrameDecoder::default());
        let (buf, rx) = receive(RawCodec, rx).await.unwrap();
        rx.await.unwrap();
        assert_eq!(buf, b"foobar".as_slice());
    }

    #[test_log::test(tokio::test)]
    async fn ingress_rejects_invalid_frames() {
        let root = [frame(&[], b"\0"), frame(&[], b"")].concat();
        let cases: [(Vec<u8>, fn(&ReceiveError) -> bool); 5] = [
            ([&root[..], &frame(&[1], b"")].concat(), |err| {
                matches!(err, ReceiveError::UnknownPath)
            }),
            (frame(&[], b"\0\x2a"), |err| {
                matches!(err, ReceiveError::DataAfterEnd)
            }),
            (root.clone(), |err| matches!(err, ReceiveError::Truncated)),
            (
                [&root[..], &frame(&[0], b"\x03fo"), &frame(&[0], b"")].concat(),
                |err| matches!(err, ReceiveError::Decode(err) if err.kind() == std::io::ErrorKind::UnexpectedEof),
            ),
            ([&root[..], b"\x01\x00\x05\x2a"].concat(), |err| {
                matches!(err, ReceiveError::Frame(FrameDecodeError::Truncated))
            }),
        ];
        for (frames, expected) in cases {
            let mut rx = FramedRead::new(frames.as_slice(), FrameDecoder::default());
            let err = receive_stream(&mut rx).await.unwrap_err();
            assert!(expected(&err), "{frames:02x?}: {err:?}");
        }
    }

    #[test_log::test(tokio::test)]
    async fn egress_partial_writes() {
        let data: Bytes = (0..1 << 18).map(|i: usize| i.to_le_bytes()[0]).collect();
        let (tx, rx) = duplex(1000);
        let tx = transmit(RawCodec, FramedWrite::new(tx, FrameEncoder), data.clone()).unwrap();
        let (chunks, ()) = try_join!(
            FramedRead::new(rx, FrameDecoder::default())
                .map_err(std::io::Error::from)
                .try_collect::<Vec<_>>(),
            tx
        )
        .unwrap();
        let Some((close, chunks)) = chunks.split_last() else {
            panic!("no frames sent");
        };
        assert!(close.is_empty());
        assert!(chunks.iter().all(|chunk| !chunk.is_empty()));
        let rx: Vec<u8> = chunks.concat();
        assert_eq!(rx, data);
    }

    #[test_log::test(tokio::test)]
    async fn egress_keeps_stream() {
        let (mut tx, mut rx) = duplex(16);
        let fut = transmit(
            RawCodec,
            FramedWrite::new(&mut tx, FrameEncoder),
            Bytes::default(),
        )
        .unwrap();
        fut.await.unwrap();
        tx.write_all(b"\x2a").await.unwrap();
        let mut buf = [0; 3];
        rx.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, *b"\0\0\x2a");
    }
}
