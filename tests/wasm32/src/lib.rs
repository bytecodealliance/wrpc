use core::error::Error;

use http_body::Body;
use wrpc::http::IncomingRequest;
use wrpc::wasmtime::{TupleDecoder, TupleEncoder, Val};
use wrpc::{Codec, ServeError};

pub async fn serve_with<T>(
    req: IncomingRequest<T>,
    codec: Codec<TupleEncoder, TupleDecoder>,
    f: impl AsyncFnOnce(Vec<Val>) -> Vec<Val>,
) -> Result<(), ServeError>
where
    T: Body<Error: Into<Box<dyn Error + Send + Sync>>> + Unpin,
{
    req.serve_local_with(codec, async move |params| Ok(f(params).await))
        .await?;
    Ok(())
}
