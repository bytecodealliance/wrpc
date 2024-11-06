wit_bindgen_wrpc::generate!({
    world: "proxy",
    generate_all,
});

// TODO: Generate a single type for both imports and exports

impl From<wasi::keyvalue::store::Error> for exports::wasi::keyvalue::store::Error {
    fn from(v: wasi::keyvalue::store::Error) -> Self {
        match v {
            wasi::keyvalue::store::Error::NoSuchStore => Self::NoSuchStore,
            wasi::keyvalue::store::Error::AccessDenied => Self::AccessDenied,
            wasi::keyvalue::store::Error::Other(err) => Self::Other(err),
        }
    }
}

impl From<exports::wasi::keyvalue::store::Error> for wasi::keyvalue::store::Error {
    fn from(v: exports::wasi::keyvalue::store::Error) -> Self {
        match v {
            exports::wasi::keyvalue::store::Error::NoSuchStore => Self::NoSuchStore,
            exports::wasi::keyvalue::store::Error::AccessDenied => Self::AccessDenied,
            exports::wasi::keyvalue::store::Error::Other(err) => Self::Other(err),
        }
    }
}

impl From<wasi::keyvalue::store::KeyResponse> for exports::wasi::keyvalue::store::KeyResponse {
    fn from(
        wasi::keyvalue::store::KeyResponse { keys, cursor }: wasi::keyvalue::store::KeyResponse,
    ) -> Self {
        Self { keys, cursor }
    }
}

impl From<exports::wasi::keyvalue::store::KeyResponse> for wasi::keyvalue::store::KeyResponse {
    fn from(
        exports::wasi::keyvalue::store::KeyResponse { keys, cursor }: exports::wasi::keyvalue::store::KeyResponse,
    ) -> Self {
        Self { keys, cursor }
    }
}
