use core::convert::Infallible;
use core::error::Error;

use http_body::Body;
use wrpc::http::IncomingRequest;
use wrpc::wasmtime::{TupleDecoder, TupleEncoder, Val};
use wrpc::{Codec, ServeError};

pub async fn serve<T>(
    req: IncomingRequest<T>,
    codec: Codec<TupleEncoder, TupleDecoder>,
    f: impl AsyncFnOnce(Vec<Val>) -> Vec<Val>,
) -> Result<(), ServeError<Infallible>>
where
    T: Body<Error: Into<Box<dyn Error + Send + Sync>>> + Unpin,
{
    req.serve(codec, async move |params| Ok(f(params).await))
        .await?;
    Ok(())
}
