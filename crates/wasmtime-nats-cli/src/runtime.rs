use core::{iter::zip, pin::pin};

use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use tokio::try_join;
use tokio_util::bytes::BytesMut;
use tokio_util::codec::Encoder;
use tracing::{error, instrument, trace, warn};
use wasmtime::component::{types, Linker, Val};
use wasmtime::AsContextMut as _;
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiView};
use wit_parser::FunctionKind;
use wrpc_introspect::rpc_func_name;
use wrpc_runtime_wasmtime::{read_value, RemoteResource, ValEncoder};
use wrpc_transport::{Invocation, Session};
use wrpc_transport_next as wrpc_transport;


