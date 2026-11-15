//! wRPC QUIC transport

use futures::{Stream, stream};

/// Returns the stream of invocations accepted over the bidirectional streams the peer opens on
/// `conn`, which ends after the first error
pub fn incoming(
    conn: quinn::Connection,
) -> impl Stream<Item = Result<((), quinn::SendStream, quinn::RecvStream), quinn::ConnectionError>>
{
    stream::try_unfold(conn, async |conn| {
        let (tx, rx) = conn.accept_bi().await?;
        Ok(Some((((), tx, rx), conn)))
    })
}
