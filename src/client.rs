use anyhow::{bail, Context};
use bytes::Bytes;
use http::Uri;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::spawn;
use tokio::task::JoinHandle;

use crate::Transmit;

#[derive(Clone)]
pub struct Invocation {
    id: String,
    h2: h2::client::SendRequest<Bytes>,
}

impl Invocation {
    pub async fn start_send(
        &mut self,
        path: &[u32],
    ) -> anyhow::Result<(h2::client::ResponseFuture, h2::SendStream<Bytes>)> {
        let path = path
            .iter()
            .map(|idx| idx.to_string())
            .reduce(|l, r| format!("{l}/{r}"));
        let uri = Uri::builder()
            .scheme(http::uri::Scheme::HTTP)
            .authority("host")
            .path_and_query(if let Some(path) = path {
                format!("invocation/{}/{path}", self.id)
            } else {
                format!("invocation/{}", self.id)
            })
            .build()
            .context("failed to construct URI")?;
        let req = http::Request::builder()
            .uri(uri)
            .method(http::Method::POST)
            .version(http::Version::HTTP_2)
            .body(())
            .context("failed to build HTTP request")?;
        self.h2
            .send_request(req, false)
            .context("failed to send head")
    }

    pub async fn send(
        &mut self,
        path: &[u32],
        val: impl Transmit,
    ) -> anyhow::Result<h2::client::ResponseFuture> {
        let (res, s) = self.start_send(path).await?;
        val.transmit(s).await?;
        Ok(res)
    }
}

pub struct Client {
    h2: h2::client::SendRequest<Bytes>,
    conn: JoinHandle<Result<(), h2::Error>>,
}

impl Client {
    pub async fn connect(
        stream: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
    ) -> anyhow::Result<Self> {
        let (h2, conn) = h2::client::handshake(stream)
            .await
            .context("failed to perform HTTP/2 handshake")?;
        let conn = spawn(conn);
        let h2 = h2
            .ready()
            .await
            .context("failed to wait for HTTP/2 connection to reach ready state")?;
        //let request = http::Request::builder()
        //    .method(http::Method::GET)
        //    .uri("https://www.example.com/")
        //    .body(())
        //    .context("failed to construct HTTP request")?;

        //let (response, _) = h2
        //    .send_request(request, true)
        //    .context("failed to send request")?;
        //let (head, mut body) = response.await?.into_parts();
        //info!(?head, "received response");

        //let mut flow_control = body.flow_control().clone();
        //while let Some(chunk) = body.data().await {
        //    let chunk = chunk?;
        //    info!(?chunk, "RX");
        //    let _ = flow_control.release_capacity(chunk.len());
        //}
        Ok(Self { h2, conn })
    }

    pub async fn call(&mut self, func: &str) -> anyhow::Result<Invocation> {
        let uri = Uri::builder()
            .scheme(http::uri::Scheme::HTTP)
            .authority("host")
            .path_and_query(format!("invoke/{func}"))
            .build()
            .context("failed to construct URI")?;
        let req = http::Request::builder()
            .uri(&uri)
            .method(http::Method::POST)
            .version(http::Version::HTTP_2)
            .header("wrpc-invocation-id", "0")
            .body(())
            .context("failed to build HTTP request")?;
        let (res, _) = self
            .h2
            .send_request(req, true)
            .context("failed to invoke function")?;
        let res = res.await.context("failed to receive response")?;
        let (http::response::Parts { status, .. }, mut body) = res.into_parts();
        if !status.is_success() {
            bail!("request failed with `{status}`")
        }
        let mut flow_control = body.flow_control().clone();
        while let Some(data) = body.data().await {
            let data = data.context("failed to receive body chunk")?;
            let _ = flow_control.release_capacity(data.len());
        }
        Ok(Invocation {
            h2: self.h2.clone(),
            id: String::from("0"),
        })
    }
}
