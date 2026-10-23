use core::future;
use core::pin::Pin;
use core::task::{Context, Poll};

use std::sync::Arc;

use futures::FutureExt as _;
use futures::future::BoxFuture;
use tokio::io::{AsyncRead as _, AsyncWrite as _, ReadBuf};
use tokio::sync::oneshot;
use wasmtime::component::{
    Access, Accessor, Destination, FutureReader, Resource, Source, StreamConsumer, StreamProducer,
    StreamReader, StreamResult, VecBuffer,
};
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut as _, StoreContextMut};
use wrpc_transport::READ_CHUNK_SIZE;
use wrpc_transport::frame::{Incoming, Outgoing};

use crate::bindings::rpc::transport::{
    Host, HostIncomingChannel, HostIncomingChannelWithStore, HostOutgoingChannel,
    HostOutgoingChannelWithStore, IncomingChannel, OutgoingChannel,
};
use crate::rpc::host::ResultProducer;
use crate::rpc::{Deferred, Error, IncomingInvocation, WrpcRpcImpl};
use crate::{WrpcView, WrpcViewExt as _};

fn index_path(path: Vec<u32>) -> wasmtime::Result<Box<[usize]>> {
    let path = path
        .into_iter()
        .map(usize::try_from)
        .collect::<Result<_, _>>()
        .context("failed to construct index path")?;
    Ok(path)
}

/// Outcome of reading the data of a channel
enum ReadOutcome {
    /// Reading the data finished
    Finished(Result<(), Error>),
    /// The stream was dropped before its end, so its result is that of the incoming direction
    Dropped(BoxFuture<'static, std::io::Result<()>>),
}

struct IncomingProducer {
    incoming: Option<Deferred<Incoming>>,
    result: Option<oneshot::Sender<ReadOutcome>>,
    /// Incoming direction of the invocation, until the handle reading the data is available
    invocation: Option<Arc<IncomingInvocation>>,
}

impl IncomingProducer {
    fn finish(&mut self, res: Result<(), Error>) {
        if let Some(tx) = self.result.take() {
            _ = tx.send(ReadOutcome::Finished(res));
        }
    }
}

impl Drop for IncomingProducer {
    fn drop(&mut self) {
        let (Some(incoming), Some(tx)) = (self.incoming.take(), self.result.take()) else {
            return;
        };
        let finished = match (&incoming, &self.invocation) {
            (Deferred::Ready(incoming), ..) => incoming.finished().boxed(),
            (Deferred::Pending(..), Some(invocation)) => invocation.finished(),
            (Deferred::Pending(..), None) => future::ready(Ok(())).boxed(),
        };
        _ = tx.send(ReadOutcome::Dropped(finished));
    }
}

impl<D> StreamProducer<D> for IncomingProducer {
    type Item = u8;
    type Buffer = VecBuffer<u8>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if dst.remaining(store.as_context_mut()) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        let this = &mut *self;
        let Some(incoming) = &mut this.incoming else {
            return Poll::Ready(Ok(StreamResult::Dropped));
        };
        let incoming = match incoming.poll_ready(cx) {
            Poll::Ready(Ok(incoming)) => {
                this.invocation = None;
                incoming
            }
            Poll::Ready(Err(err)) => {
                self.finish(Err(Error::Read(err)));
                return Poll::Ready(Ok(StreamResult::Dropped));
            }
            Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => return Poll::Pending,
        };
        let mut dst = dst.as_direct(store, READ_CHUNK_SIZE);
        let mut buf = ReadBuf::new(dst.remaining());
        let res = Pin::new(incoming).poll_read(cx, &mut buf);
        match res {
            Poll::Ready(Ok(())) if buf.filled().is_empty() => {
                self.finish(Ok(()));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Ready(Ok(())) => {
                let n = buf.filled().len();
                dst.mark_written(n);
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(Err(err)) => {
                self.finish(Err(Error::Read(err)));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
    }
}

struct OutgoingConsumer {
    outgoing: Deferred<Outgoing>,
    result: Option<oneshot::Sender<Result<(), Error>>>,
}

impl OutgoingConsumer {
    fn finish(&mut self, res: Result<(), Error>) {
        if let Some(tx) = self.result.take() {
            _ = tx.send(res);
        }
    }
}

impl Drop for OutgoingConsumer {
    fn drop(&mut self) {
        self.finish(Ok(()));
    }
}

impl<D> StreamConsumer<D> for OutgoingConsumer {
    type Item = u8;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        src: Source<Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut src = src.as_direct(store);
        let outgoing = match self.outgoing.poll_ready(cx) {
            Poll::Ready(Ok(outgoing)) => outgoing,
            Poll::Ready(Err(err)) => {
                self.finish(Err(Error::Write(err)));
                return Poll::Ready(Ok(StreamResult::Dropped));
            }
            Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => return Poll::Pending,
        };
        if src.remaining().is_empty() {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        let res = Pin::new(outgoing).poll_write(cx, src.remaining());
        match res {
            Poll::Ready(Ok(n)) => {
                src.mark_read(n);
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(Err(err)) => {
                self.finish(Err(Error::Write(err)));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T: WrpcView> Host for WrpcRpcImpl<T> {}

impl<T: WrpcView + 'static> HostIncomingChannelWithStore<T> for WrpcRpcImpl<T> {
    fn read_via_stream(
        mut store: Access<'_, T, Self>,
        incoming: Resource<IncomingChannel>,
    ) -> wasmtime::Result<
        Result<(StreamReader<u8>, FutureReader<Result<(), Resource<Error>>>), Resource<Error>>,
    > {
        let WrpcRpcImpl(wrpc) = store.get();
        let incoming = wrpc
            .wrpc()
            .table
            .get_mut(&incoming)
            .context("failed to get incoming channel from table")?;
        let invocation = Arc::clone(&incoming.invocation);
        let incoming = match incoming.take_data() {
            Ok(incoming) => incoming,
            Err(err) => {
                let err = wrpc.push_error(err)?;
                return Ok(Err(err));
            }
        };
        let (tx, rx) = oneshot::channel();
        let data = StreamReader::new(
            &mut store,
            IncomingProducer {
                incoming: Some(incoming),
                result: Some(tx),
                invocation: Some(invocation),
            },
        )?;
        let res = async move {
            match rx.await {
                Ok(ReadOutcome::Finished(res)) => res,
                Ok(ReadOutcome::Dropped(finished)) => finished.await.map_err(Error::Read),
                Err(..) => Err(Error::Read(std::io::ErrorKind::BrokenPipe.into())),
            }
        }
        .boxed();
        let res = FutureReader::new(&mut store, ResultProducer(res))?;
        Ok(Ok((data, res)))
    }
}

impl<T: WrpcView> HostIncomingChannel for WrpcRpcImpl<T> {
    fn index(
        &mut self,
        incoming: Resource<IncomingChannel>,
        path: Vec<u32>,
    ) -> wasmtime::Result<Result<Resource<IncomingChannel>, Resource<Error>>> {
        let table = self.0.wrpc().table;
        let incoming = table
            .get(&incoming)
            .context("failed to get incoming channel from table")?;
        match incoming.index(&index_path(path)?) {
            Ok(incoming) => {
                let incoming = table
                    .push(incoming)
                    .context("failed to push incoming channel to table")?;
                Ok(Ok(incoming))
            }
            Err(err) => {
                let err = self.0.push_error(Error::IncomingIndex(err))?;
                Ok(Err(err))
            }
        }
    }

    fn drop(&mut self, incoming: Resource<IncomingChannel>) -> wasmtime::Result<()> {
        self.0
            .wrpc()
            .table
            .delete(incoming)
            .context("failed to delete incoming channel from table")?;
        Ok(())
    }
}

impl<T: WrpcView + 'static> HostOutgoingChannelWithStore<T> for WrpcRpcImpl<T> {
    fn write_via_stream(
        mut store: Access<'_, T, Self>,
        outgoing: Resource<OutgoingChannel>,
        mut data: StreamReader<u8>,
    ) -> wasmtime::Result<Result<FutureReader<Result<(), Resource<Error>>>, Resource<Error>>> {
        let WrpcRpcImpl(wrpc) = store.get();
        let outgoing = wrpc
            .wrpc()
            .table
            .get_mut(&outgoing)
            .context("failed to get outgoing channel from table")?;
        let outgoing = match outgoing.take_data() {
            Ok(outgoing) => outgoing,
            Err(err) => {
                let err = wrpc.push_error(err)?;
                data.close(&mut store)?;
                return Ok(Err(err));
            }
        };
        let (tx, rx) = oneshot::channel();
        let res = async move {
            rx.await
                .unwrap_or_else(|_| Err(Error::Write(std::io::ErrorKind::BrokenPipe.into())))
        }
        .boxed();
        data.pipe(
            &mut store,
            OutgoingConsumer {
                outgoing: Deferred::new(outgoing),
                result: Some(tx),
            },
        )?;
        let res = FutureReader::new(&mut store, ResultProducer(res))?;
        Ok(Ok(res))
    }

    async fn close(
        store: &Accessor<T, Self>,
        outgoing: Resource<OutgoingChannel>,
    ) -> wasmtime::Result<Result<(), Resource<Error>>> {
        let outgoing = store.with(|mut store| {
            let WrpcRpcImpl(wrpc) = store.get();
            let outgoing = wrpc
                .wrpc()
                .table
                .get(&outgoing)
                .context("failed to get outgoing channel from table")?;
            Ok::<_, wasmtime::Error>(outgoing.outgoing())
        })?;
        let res = async {
            let mut outgoing = outgoing.await?;
            outgoing.close().await
        }
        .await;
        store.with(|mut store| {
            let WrpcRpcImpl(wrpc) = store.get();
            match res {
                Ok(()) => Ok(Ok(())),
                Err(err) => {
                    let err = wrpc.push_error(Error::Write(err))?;
                    Ok(Err(err))
                }
            }
        })
    }
}

impl<T: WrpcView> HostOutgoingChannel for WrpcRpcImpl<T> {
    fn index(
        &mut self,
        outgoing: Resource<OutgoingChannel>,
        path: Vec<u32>,
    ) -> wasmtime::Result<Result<Resource<OutgoingChannel>, Resource<Error>>> {
        let table = self.0.wrpc().table;
        let outgoing = table
            .get(&outgoing)
            .context("failed to get outgoing channel from table")?;
        match outgoing.index(&index_path(path)?) {
            Ok(outgoing) => {
                let outgoing = table
                    .push(outgoing)
                    .context("failed to push outgoing channel to table")?;
                Ok(Ok(outgoing))
            }
            Err(err) => {
                let err = self.0.push_error(Error::OutgoingIndex(err))?;
                Ok(Err(err))
            }
        }
    }

    fn drop(&mut self, outgoing: Resource<OutgoingChannel>) -> wasmtime::Result<()> {
        self.0
            .wrpc()
            .table
            .delete(outgoing)
            .context("failed to delete outgoing channel from table")?;
        Ok(())
    }
}
