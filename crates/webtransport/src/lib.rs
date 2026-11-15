//! wRPC WebTransport transport

use futures::{Stream, stream};

/// Returns the stream of invocations accepted over the bidirectional streams the peer opens on
/// `session`, which ends after the first error
pub fn incoming(
    session: web_transport_quinn::Session,
) -> impl Stream<
    Item = Result<
        (
            (),
            web_transport_quinn::SendStream,
            web_transport_quinn::RecvStream,
        ),
        web_transport_quinn::SessionError,
    >,
> {
    stream::try_unfold(session, async |session| {
        let (tx, rx) = session.accept_bi().await?;
        Ok(Some((((), tx, rx), session)))
    })
}
