use super::Protocol;
use crate::ConnectRequest;
use anyhow::Result;
use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncWrite};

pub struct TestProtocol {
    bytes: BytesMut,
    target: ConnectRequest,
}

impl Protocol for TestProtocol {
    fn wrap(&mut self, plain: &[u8]) -> Result<Self> {
        todo!()
    }

    fn unwrap(&mut self, inp: &mut BytesMut) -> Result<Self> {
        todo!()
    }

    async fn tunnel<C, U>(&mut self, client: C, upstream: U) -> Result<()>
    where
        C: AsyncRead + AsyncWrite + Unpin + Send,
        U: AsyncRead + AsyncWrite + Unpin + Send,
    {
        todo!()
    }
}
