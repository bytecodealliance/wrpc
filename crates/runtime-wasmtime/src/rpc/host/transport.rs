use core::marker::PhantomData;

use std::sync::Arc;

use anyhow::{bail, Context as _};
use wasmtime::component::Resource;
use wasmtime_wasi::bindings::io::streams::{InputStream, OutputStream};
use wasmtime_wasi::pipe::AsyncReadStream;
use wasmtime_wasi::subscribe;
use wasmtime_wasi::{bindings::io::poll::Pollable, pipe::AsyncWriteStream};
use wrpc_transport::{Index as _, Invoke};

use crate::bindings::rpc::error::Error;
use crate::bindings::rpc::transport::{
    Host, HostIncomingChannel, HostInvocation, HostOutgoingChannel, IncomingChannel, Invocation,
    OutgoingChannel,
};
use crate::rpc::{IncomingChannelStream, OutgoingChannelStream, WrpcRpcImpl};
use crate::{WrpcView, WrpcViewExt as _};

impl<T: WrpcView> Host for WrpcRpcImpl<T> {}

impl<T: WrpcView> HostInvocation for WrpcRpcImpl<T> {
    fn subscribe(
        &mut self,
        invocation: Resource<Invocation>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        subscribe(self.0.table(), invocation)
    }

    async fn finish(
        &mut self,
        invocation: Resource<Invocation>,
    ) -> wasmtime::Result<
        Result<(Resource<OutgoingChannel>, Resource<IncomingChannel>), Resource<Error>>,
    > {
        let invocation = self.0.delete_invocation(invocation)?;
        match invocation.await {
            Ok((tx, rx)) => {
                let rx = self.0.push_incoming_channel(rx)?;
                let tx = self.0.push_outgoing_channel(tx)?;
                Ok(Ok((tx, rx)))
            }
            Err(error) => {
                let error = self.0.push_error(error)?;
                Ok(Err(error))
            }
        }
    }

    fn drop(&mut self, invocation: Resource<Invocation>) -> wasmtime::Result<()> {
        _ = self.0.delete_invocation(invocation)?;
        Ok(())
    }
}

impl<T: WrpcView> HostIncomingChannel for WrpcRpcImpl<T> {
    fn data(
        &mut self,
        incoming: Resource<IncomingChannel>,
    ) -> wasmtime::Result<Option<Resource<InputStream>>> {
        let IncomingChannel(stream) = self
            .0
            .table()
            .get_mut(&incoming)
            .context("failed to get incoming channel from table")?;
        if Arc::get_mut(stream).is_none() {
            return Ok(None);
        }
        let stream = Arc::clone(stream);
        let stream = self
            .0
            .table()
            .push_child(
                Box::new(AsyncReadStream::new(IncomingChannelStream {
                    incoming: IncomingChannel(stream),
                    _ty: PhantomData::<<T::Invoke as Invoke>::Incoming>,
                })) as InputStream,
                &incoming,
            )
            .context("failed to push input stream to table")?;
        Ok(Some(stream))
    }

    fn index(
        &mut self,
        incoming: Resource<IncomingChannel>,
        path: Vec<u32>,
    ) -> wasmtime::Result<Result<Resource<IncomingChannel>, Resource<Error>>> {
        let path = path
            .into_iter()
            .map(usize::try_from)
            .collect::<Result<Box<[_]>, _>>()
            .context("failed to construct subscription path")?;
        let IncomingChannel(incoming) = self
            .0
            .table()
            .get(&incoming)
            .context("failed to get incoming channel from table")?;
        let incoming = {
            let Ok(incoming) = incoming.read() else {
                bail!("lock poisoned");
            };
            let incoming = incoming
                .downcast_ref::<<T::Invoke as Invoke>::Incoming>()
                .context("invalid incoming channel type")?;
            incoming.index(&path)
        };
        match incoming {
            Ok(incoming) => {
                let incoming = self.0.push_incoming_channel(incoming)?;
                Ok(Ok(incoming))
            }
            Err(error) => {
                let error = self.0.push_error(error)?;
                Ok(Err(error))
            }
        }
    }

    fn drop(&mut self, incoming: Resource<IncomingChannel>) -> wasmtime::Result<()> {
        self.0.delete_incoming_channel(incoming)?;
        Ok(())
    }
}

impl<T: WrpcView> HostOutgoingChannel for WrpcRpcImpl<T> {
    fn data(
        &mut self,
        outgoing: Resource<OutgoingChannel>,
    ) -> wasmtime::Result<Option<Resource<OutputStream>>> {
        let OutgoingChannel(stream) = self
            .0
            .table()
            .get_mut(&outgoing)
            .context("failed to get outgoing channel from table")?;
        if Arc::get_mut(stream).is_none() {
            return Ok(None);
        }
        let stream = Arc::clone(stream);
        let stream = self
            .0
            .table()
            .push_child(
                Box::new(AsyncWriteStream::new(
                    8192,
                    OutgoingChannelStream {
                        outgoing: OutgoingChannel(stream),
                        _ty: PhantomData::<<T::Invoke as Invoke>::Outgoing>,
                    },
                )) as OutputStream,
                &outgoing,
            )
            .context("failed to push output stream to table")?;
        Ok(Some(stream))
    }

    fn index(
        &mut self,
        outgoing: Resource<OutgoingChannel>,
        path: Vec<u32>,
    ) -> wasmtime::Result<Result<Resource<OutgoingChannel>, Resource<Error>>> {
        let path = path
            .into_iter()
            .map(usize::try_from)
            .collect::<Result<Box<[_]>, _>>()
            .context("failed to construct subscription path")?;
        let OutgoingChannel(outgoing) = self
            .0
            .table()
            .get(&outgoing)
            .context("failed to get outgoing channel from table")?;
        let incoming = {
            let Ok(outgoing) = outgoing.read() else {
                bail!("lock poisoned");
            };
            let outgoing = outgoing
                .downcast_ref::<<T::Invoke as Invoke>::Outgoing>()
                .context("invalid outgoing channel type")?;
            outgoing.index(&path)
        };
        match incoming {
            Ok(outgoing) => {
                let outgoing = self.0.push_outgoing_channel(outgoing)?;
                Ok(Ok(outgoing))
            }
            Err(error) => {
                let error = self.0.push_error(error)?;
                Ok(Err(error))
            }
        }
    }

    fn drop(&mut self, outgoing: Resource<OutgoingChannel>) -> wasmtime::Result<()> {
        self.0.delete_outgoing_channel(outgoing)?;
        Ok(())
    }
}
