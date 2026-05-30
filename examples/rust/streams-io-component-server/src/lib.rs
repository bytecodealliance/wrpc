mod bindings {
    use crate::Handler;

    wit_bindgen::generate!({
        with: {
            "wasi:io/error@0.2.0": generate,
            "wasi:io/poll@0.2.0": generate,
            "wasi:io/streams@0.2.0": generate,
        }
    });

    export!(Handler);
}

use bindings::exports::wrpc_examples::streams_io::handler::Guest;
use bindings::wasi::io::streams::{InputStream, StreamError};

pub struct Handler;

impl Guest for Handler {
    /// Read the entire input stream and return the total number of bytes read.
    fn count(data: InputStream) -> u64 {
        let mut total: u64 = 0;
        loop {
            // `blocking-read` waits for at least one byte, then returns up to `len`.
            match data.blocking_read(8096) {
                Ok(chunk) => total += chunk.len() as u64,
                Err(StreamError::Closed) => return total,
                Err(StreamError::LastOperationFailed(err)) => {
                    // Surface the failure on stderr; return what we have so far.
                    eprintln!("stream read failed: {}", err.to_debug_string());
                    return total;
                }
            }
        }
    }
}
