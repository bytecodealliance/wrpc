use wasi::{http::types::ErrorCode, io::streams::StreamError};

use crate::wasi::http::types::{
    Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
};

wit_bindgen::generate!({
    world: "echo",
    exports: {
        "wasi:http/incoming-handler": Echo,
    },
});

struct Echo;

impl exports::wasi::http::incoming_handler::Guest for Echo {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let request_body = request
            .consume()
            .expect("failed to get incoming request body");
        let response = OutgoingResponse::new(Fields::new());
        let response_body = response
            .body()
            .expect("failed to get outgoing response body");
        {
            let response_stream = response_body
                .write()
                .expect("failed to get outgoing response stream");
            let request_stream = request_body
                .stream()
                .expect("failed to get incoming request stream");
            loop {
                match request_stream.blocking_read(4096) {
                    Ok(buf) => response_stream
                        .blocking_write_and_flush(&buf)
                        .expect("failed to write bytes to response stream"),
                    Err(StreamError::Closed) => break,
                    Err(StreamError::LastOperationFailed(err)) => {
                        ResponseOutparam::set(
                            response_out,
                            Err(&ErrorCode::InternalError(Some(format!(
                                "failed to read from request stream: {}",
                                err.to_debug_string(),
                            )))),
                        );
                        return;
                    }
                }
            }
        }
        OutgoingBody::finish(response_body, None).expect("failed to finish response body");
        ResponseOutparam::set(response_out, Ok(response));
    }
}
