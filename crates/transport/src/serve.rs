//! wRPC transport server handle

use core::borrow::BorrowMut;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, FramedRead, FramedWrite};
use tracing::Instrument as _;

use crate::frame::{self, Outgoing, Receipt};
use crate::mux::ingress;
use crate::{Decode, Encode, Receive};

/// Server of invocations, which decodes parameters using `dec` and encodes results using `enc`
#[derive(Clone, Copy, Debug, Default)]
pub struct Server<E, D> {
    /// Result encoder
    pub enc: E,
    /// Parameter decoder
    pub dec: D,
}

impl<E, D> Server<E, D>
where
    D: Decoder<Error = std::io::Error> + Receive,
{
    /// Serves an invocation over the byte stream `tx` and the frames `rx`, which follow the header
    /// read by [`HeaderDecoder`](crate::HeaderDecoder)
    ///
    /// Returns a future receiving the parameters, which resolves to the synchronous parameters and
    /// the [Receipt] of the asynchronous parameters, and the [Outgoing] handle the results are sent
    /// on using [`Outgoing::send`]. Nothing is received unless it is polled.
    ///
    /// No frame past the invocation is read from `rx`, but it may have read bytes past it into its
    /// read buffer, so to reuse the byte stream pass `rx` by `&mut` and keep reading from it once
    /// all parameters are received, for example, using [`FramedRead::map_decoder`].
    pub fn serve<F, I, O>(
        self,
        tx: O,
        rx: F,
    ) -> (
        impl Future<Output = std::io::Result<(D::Item, Receipt<F, I, D>)>>,
        Outgoing<O, E>,
    )
    where
        F: BorrowMut<FramedRead<I, frame::Decoder>>,
        I: AsyncRead + Unpin,
        O: AsyncWrite + Unpin,
    {
        let Self { enc, dec } = self;
        (
            ingress(rx, dec).in_current_span(),
            Outgoing {
                tx: FramedWrite::new(tx, frame::Encoder),
                enc,
            },
        )
    }
}

/// Serves an invocation over the byte stream `tx` and the frames `rx` using the default codecs of
/// parameters `P` and results `R`, see [`Server::serve`]
pub fn serve<P, R, F, I, O>(
    tx: O,
    rx: F,
) -> (
    impl Future<Output = std::io::Result<(P, Receipt<F, I, P::Decoder>)>>,
    Outgoing<O, R::Encoder>,
)
where
    F: BorrowMut<FramedRead<I, frame::Decoder>>,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    P: Decode,
    R: Encode,
{
    Server::<R::Encoder, P::Decoder>::default().serve(tx, rx)
}
