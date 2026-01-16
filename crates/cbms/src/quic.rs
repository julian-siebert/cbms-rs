use crate::{Error, Message};
use async_trait::async_trait;
use quinn::{RecvStream, SendStream};

pub struct QuicTransport {
    send: SendStream,
    recv: RecvStream,
}

impl QuicTransport {
    pub fn new(send: SendStream, recv: RecvStream) -> Self {
        Self { send, recv }
    }

    async fn send_async(&mut self, msg: &Message) -> Result<(), Error> {
        let bytes = msg.to_cbor()?;
        let len = (bytes.len() as u32).to_be_bytes();

        self.send
            .write_all(&len)
            .await
            .map_err(|e| Error::External(Box::new(e)))?;
        self.send
            .write_all(&bytes)
            .await
            .map_err(|e| Error::External(Box::new(e)))?;

        tokio::io::AsyncWriteExt::flush(&mut self.send).await?;

        Ok(())
    }

    async fn recv_async(&mut self) -> Result<Message, Error> {
        let mut len_buf = [0u8; 4];
        self.recv
            .read_exact(&mut len_buf)
            .await
            .map_err(|e| Error::External(Box::new(e)))?;
        let msg_len = u32::from_be_bytes(len_buf) as usize;

        if msg_len > 16 * 1024 * 1024 {
            return Err(Error::InvalidMessage("Message too large".into()));
        }

        let mut buf = vec![0u8; msg_len];
        self.recv
            .read_exact(&mut buf)
            .await
            .map_err(|e| Error::External(Box::new(e)))?;

        Message::from_cbor(&buf)
    }
}

#[cfg(feature = "async")]
#[async_trait]
impl crate::transport_async::Transport for QuicTransport {
    async fn send(&mut self, msg: &Message) -> Result<(), Error> {
        self.send_async(msg).await
    }

    async fn recv(&mut self) -> Result<Message, Error> {
        self.recv_async().await
    }
}

impl crate::transport::Transport for QuicTransport {
    fn send(&mut self, msg: &Message) -> Result<(), Error> {
        tokio::runtime::Handle::current().block_on(self.send_async(msg))
    }

    fn recv(&mut self) -> Result<Message, Error> {
        tokio::runtime::Handle::current().block_on(self.recv_async())
    }
}
