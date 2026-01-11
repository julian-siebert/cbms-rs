use std::io::Cursor;

use bytes::{Buf, BufMut, BytesMut};
use ciborium::from_reader;
use tokio::io::{
    AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, Stdin, Stdout, stdin, stdout,
};

use crate::{Error, Message};

pub const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

#[async_trait::async_trait]
pub trait Transport: Send {
    async fn send(&mut self, msg: &Message) -> Result<(), Error>;

    async fn recv(&mut self) -> Result<Message, Error>;

    async fn try_recv(&mut self) -> Result<Option<Message>, Error> {
        Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Non-blocking receive not supported by this transport",
        )))
    }

    async fn shutdown(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

struct CborDecoder {
    buffer: BytesMut,
}

impl CborDecoder {
    fn new() -> Self {
        Self {
            buffer: BytesMut::with_capacity(8192),
        }
    }

    fn try_decode(&mut self) -> Result<Option<Message>, Error> {
        if self.buffer.is_empty() {
            return Ok(None);
        }

        let cursor = Cursor::new(&self.buffer[..]);

        match from_reader::<Message, Cursor<&[u8]>>(cursor) {
            Ok(msg) => {
                let encoded = msg.to_cbor()?;
                let consumed = encoded.len();

                self.buffer.advance(consumed);
                Ok(Some(msg))
            }
            Err(ciborium::de::Error::Io(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                Ok(None)
            }
            Err(err) => {
                self.buffer.clear();
                Err(Error::CborDe(err))
            }
        }
    }

    async fn decode<R: AsyncRead + Unpin>(&mut self, reader: &mut R) -> Result<Message, Error> {
        loop {
            if let Some(msg) = self.try_decode()? {
                return Ok(msg);
            }

            let mut temp = vec![0u8; 8192];
            let n = reader.read(&mut temp).await?;

            if n == 0 {
                if self.buffer.is_empty() {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "Stream closed",
                    )));
                } else {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "Stream closed with incomplete message",
                    )));
                }
            }

            self.buffer.put_slice(&temp[..n]);

            if self.buffer.len() > MAX_MESSAGE_SIZE {
                return Err(Error::InvalidMessage(format!(
                    "Message exceeds maximum size of {} bytes",
                    MAX_MESSAGE_SIZE
                )));
            }
        }
    }
}

pub type StdioTransport = StreamTransport<Stdin, Stdout>;

impl StdioTransport {
    pub fn stdio() -> Self {
        StreamTransport::new(stdin(), stdout())
    }
}

impl Default for StdioTransport {
    fn default() -> Self {
        Self::stdio()
    }
}

pub struct StreamTransport<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    reader: BufReader<R>,
    writer: W,
    decoder: CborDecoder,
}

impl<R, W> StreamTransport<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            decoder: CborDecoder::new(),
        }
    }
}

#[async_trait::async_trait]
impl<R, W> Transport for StreamTransport<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    async fn send(&mut self, msg: &Message) -> Result<(), Error> {
        let bytes = msg.to_cbor()?;
        self.writer.write_all(&bytes).await?;
        self.writer.flush().await?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<Message, Error> {
        self.decoder.decode(&mut self.reader).await
    }

    async fn shutdown(&mut self) -> Result<(), Error> {
        self.writer.shutdown().await?;
        Ok(())
    }
}

#[cfg(unix)]
pub mod unix {
    use super::*;
    use std::path::Path;
    use tokio::net::{UnixDatagram, UnixStream, unix::UCred};

    pub struct UnixStreamTransport {
        stream: UnixStream,
        decoder: CborDecoder,
    }

    impl UnixStreamTransport {
        pub fn new(stream: UnixStream) -> Self {
            Self {
                stream,
                decoder: CborDecoder::new(),
            }
        }

        pub async fn connect(path: impl AsRef<Path>) -> Result<Self, Error> {
            let stream = UnixStream::connect(path).await?;
            Ok(Self::new(stream))
        }

        pub fn peer_cred(&self) -> Result<UCred, Error> {
            Ok(self.stream.peer_cred()?)
        }
    }

    #[async_trait::async_trait]
    impl Transport for UnixStreamTransport {
        async fn send(&mut self, msg: &Message) -> Result<(), Error> {
            let bytes = msg.to_cbor()?;
            self.stream.write_all(&bytes).await?;
            self.stream.flush().await?;
            Ok(())
        }

        async fn recv(&mut self) -> Result<Message, Error> {
            self.decoder.decode(&mut self.stream).await
        }

        async fn shutdown(&mut self) -> Result<(), Error> {
            self.stream.shutdown().await?;
            Ok(())
        }
    }

    pub struct UnixDatagramTransport {
        socket: UnixDatagram,
        buffer: BytesMut,
    }

    impl UnixDatagramTransport {
        pub fn new(socket: UnixDatagram) -> Self {
            Self {
                socket,
                buffer: BytesMut::with_capacity(MAX_MESSAGE_SIZE),
            }
        }

        pub async fn bind(path: impl AsRef<Path>) -> Result<Self, Error> {
            let socket = UnixDatagram::bind(path)?;
            Ok(Self::new(socket))
        }

        pub async fn connect(path: impl AsRef<Path>) -> Result<Self, Error> {
            let socket = UnixDatagram::unbound()?;
            socket.connect(path)?;
            Ok(Self::new(socket))
        }

        pub async fn send_to(
            &mut self,
            msg: &Message,
            path: impl AsRef<Path>,
        ) -> Result<(), Error> {
            let bytes = msg.to_cbor()?;
            if bytes.len() > MAX_MESSAGE_SIZE {
                return Err(Error::InvalidMessage(format!(
                    "Message size {} exceeds max datagram size {}",
                    bytes.len(),
                    MAX_MESSAGE_SIZE
                )));
            }
            self.socket.send_to(&bytes, path).await?;
            Ok(())
        }

        pub async fn recv_from(
            &mut self,
        ) -> Result<(Message, tokio::net::unix::SocketAddr), Error> {
            self.buffer.resize(MAX_MESSAGE_SIZE, 0);
            let (len, addr) = self.socket.recv_from(&mut self.buffer).await?;
            let msg = Message::from_cbor(&self.buffer[..len])?;
            Ok((msg, addr))
        }
    }

    #[async_trait::async_trait]
    impl Transport for UnixDatagramTransport {
        async fn send(&mut self, msg: &Message) -> Result<(), Error> {
            let bytes = msg.to_cbor()?;
            if bytes.len() > MAX_MESSAGE_SIZE {
                return Err(Error::InvalidMessage(format!(
                    "Message size {} exceeds max datagram size {}",
                    bytes.len(),
                    MAX_MESSAGE_SIZE
                )));
            }
            self.socket.send(&bytes).await?;
            Ok(())
        }

        async fn recv(&mut self) -> Result<Message, Error> {
            self.buffer.resize(MAX_MESSAGE_SIZE, 0);
            let len = self.socket.recv(&mut self.buffer).await?;
            let msg = Message::from_cbor(&self.buffer[..len])?;
            Ok(msg)
        }
    }
}

pub struct BufferedTransport<T: Transport> {
    inner: T,
    send_buffer: Vec<Message>,
    max_buffer_size: usize,
    auto_flush: bool,
}

impl<T: Transport> BufferedTransport<T> {
    pub fn new(transport: T, max_buffer_size: usize) -> Self {
        Self {
            inner: transport,
            send_buffer: Vec::with_capacity(max_buffer_size),
            max_buffer_size,
            auto_flush: true,
        }
    }

    pub fn with_auto_flush(mut self, auto_flush: bool) -> Self {
        self.auto_flush = auto_flush;
        self
    }

    pub fn buffer(&mut self, msg: Message) {
        self.send_buffer.push(msg);
    }

    pub async fn flush(&mut self) -> Result<(), Error> {
        for msg in self.send_buffer.drain(..) {
            self.inner.send(&msg).await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl<T: Transport> Transport for BufferedTransport<T> {
    async fn send(&mut self, msg: &Message) -> Result<(), Error> {
        self.send_buffer.push(msg.clone());
        if self.auto_flush && self.send_buffer.len() >= self.max_buffer_size {
            self.flush().await?;
        }
        Ok(())
    }

    async fn recv(&mut self) -> Result<Message, Error> {
        self.inner.recv().await
    }

    async fn try_recv(&mut self) -> Result<Option<Message>, Error> {
        self.inner.try_recv().await
    }
}

impl<T: Transport> Drop for BufferedTransport<T> {
    fn drop(&mut self) {
        // Note: We can't call async flush in Drop
        // Users should manually flush before dropping
        if !self.send_buffer.is_empty() {
            eprintln!(
                "Warning: AsyncBufferedTransport dropped with {} unsent messages",
                self.send_buffer.len()
            );
        }
    }
}
