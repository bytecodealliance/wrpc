//! wRPC transport server handle

use core::borrow::BorrowMut;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, FramedRead, FramedWrite};
use tracing::{Instrument as _, debug, trace};

use crate::Receive;
use crate::frame::{self, Outgoing};
use crate::mux::ingress;

/// Serves an invocation over the byte stream `tx` and the frames `rx`, which follow the header
/// read by [`Header::read`](crate::Header::read), decoding the parameters using `dec`
///
/// Returns a future receiving the parameters, which resolves to the synchronous parameters and
/// a future, which receives the asynchronous parameters and resolves once all are received,
/// and the [Outgoing] handle the results are sent on using [`Outgoing::send`]. Nothing is
/// received unless it is polled.
///
/// No frame past the invocation is read from `rx`, but it may have read bytes past it into its
/// read buffer, so to reuse the byte stream pass `rx` by `&mut` and keep reading from it once
/// all parameters are received, for example, using [`FramedRead::map_decoder`].
pub fn serve<R, I, O, D>(
    tx: O,
    rx: R,
    dec: D,
) -> (
    impl Future<Output = std::io::Result<(D::Item, impl Future<Output = std::io::Result<()>>)>>,
    Outgoing<O>,
)
where
    R: BorrowMut<FramedRead<I, frame::Decoder>>,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    D: Decoder<Error = std::io::Error> + Receive,
{
    (
        async move {
            debug!("receiving sync parameters");
            let (params, rx) = ingress(rx, dec).await?;
            trace!("received sync parameters");
            Ok((
                params,
                async move {
                    debug!("receiving async parameters");
                    rx.await
                }
                .in_current_span(),
            ))
        }
        .in_current_span(),
        Outgoing {
            tx: FramedWrite::new(tx, frame::Encoder),
        },
    )
}
