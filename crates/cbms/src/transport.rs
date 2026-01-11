use std::io::{BufReader, Read, Stdin, Stdout, Write, stdin, stdout};

use crate::{Error, Message};

pub const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

pub trait Transport {
    fn send(&mut self, msg: &Message) -> Result<(), Error>;

    fn recv(&mut self) -> Result<Message, Error>;

    fn try_recv(&mut self) -> Result<Option<Message>, Error> {
        Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Non-blocking receive not supported by this transport",
        )))
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
    R: Read,
    W: Write,
{
    reader: BufReader<R>,
    writer: W,
}

impl<R, W> StreamTransport<R, W>
where
    R: Read,
    W: Write,
{
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
        }
    }
}

impl<R, W> Transport for StreamTransport<R, W>
where
    R: Read,
    W: Write,
{
    fn send(&mut self, msg: &Message) -> Result<(), Error> {
        let bytes = msg.to_cbor()?;
        self.writer.write_all(&bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    fn recv(&mut self) -> Result<Message, Error> {
        Ok(ciborium::from_reader(&mut self.reader)?)
    }
}

#[cfg(unix)]
pub mod unix {
    use std::{
        os::unix::net::{SocketAddr, UnixDatagram, UnixStream},
        path::Path,
    };

    use super::*;

    pub struct UnixStreamTransport {
        reader: BufReader<UnixStream>,
        writer: UnixStream,
    }

    impl UnixStreamTransport {
        pub fn new(stream: UnixStream) -> Result<Self, Error> {
            let reader_stream = stream.try_clone()?;
            Ok(Self {
                reader: BufReader::new(reader_stream),
                writer: stream,
            })
        }

        pub fn connect(path: impl AsRef<Path>) -> Result<Self, Error> {
            let stream = UnixStream::connect(path)?;
            Self::new(stream)
        }

        pub fn peer_addr(&self) -> Result<SocketAddr, Error> {
            Ok(self.writer.peer_addr()?)
        }

        pub fn local_addr(&self) -> Result<SocketAddr, Error> {
            Ok(self.writer.local_addr()?)
        }
    }

    impl Transport for UnixStreamTransport {
        fn send(&mut self, msg: &Message) -> Result<(), Error> {
            let bytes = msg.to_cbor()?;
            self.writer.write_all(&bytes)?;
            self.writer.flush()?;
            Ok(())
        }

        fn recv(&mut self) -> Result<Message, Error> {
            Ok(ciborium::from_reader(&mut self.reader)?)
        }
    }

    pub struct UnixDatagramTransport {
        socket: UnixDatagram,
        buffer: Vec<u8>,
    }

    impl UnixDatagramTransport {
        pub fn new(socket: UnixDatagram) -> Self {
            Self {
                socket,
                buffer: vec![0u8; MAX_MESSAGE_SIZE],
            }
        }

        pub fn bind(path: impl AsRef<Path>) -> Result<Self, Error> {
            let socket = UnixDatagram::bind(path)?;
            Ok(Self::new(socket))
        }

        pub fn connect(path: impl AsRef<Path>) -> Result<Self, Error> {
            let socket = UnixDatagram::unbound()?;
            socket.connect(path)?;
            Ok(Self::new(socket))
        }

        pub fn send_to(&mut self, msg: &Message, path: impl AsRef<Path>) -> Result<(), Error> {
            let bytes = msg.to_cbor()?;
            if bytes.len() > MAX_MESSAGE_SIZE {
                return Err(Error::InvalidMessage(format!(
                    "Message size {} exceeds max datagram size {}",
                    bytes.len(),
                    MAX_MESSAGE_SIZE
                )));
            }
            self.socket.send_to(&bytes, path)?;
            Ok(())
        }

        pub fn recv_from(&mut self) -> Result<(Message, SocketAddr), Error> {
            let (len, addr) = self.socket.recv_from(&mut self.buffer)?;
            let msg = Message::from_cbor(&self.buffer[..len])?;
            Ok((msg, addr))
        }
    }

    impl Transport for UnixDatagramTransport {
        fn send(&mut self, msg: &Message) -> Result<(), Error> {
            let bytes = msg.to_cbor()?;
            if bytes.len() > MAX_MESSAGE_SIZE {
                return Err(Error::InvalidMessage(format!(
                    "Message size {} exceeds max datagram size {}",
                    bytes.len(),
                    MAX_MESSAGE_SIZE
                )));
            }
            self.socket.send(&bytes)?;
            Ok(())
        }

        fn recv(&mut self) -> Result<Message, Error> {
            let len = self.socket.recv(&mut self.buffer)?;
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

    pub fn buffered_count(&self) -> usize {
        self.send_buffer.len()
    }

    pub fn flush(&mut self) -> Result<(), Error> {
        for msg in self.send_buffer.drain(..) {
            self.inner.send(&msg)?;
        }
        Ok(())
    }
}

impl<T: Transport> Transport for BufferedTransport<T> {
    fn send(&mut self, msg: &Message) -> Result<(), Error> {
        self.send_buffer.push(msg.clone());
        if self.auto_flush && self.send_buffer.len() >= self.max_buffer_size {
            self.flush()?;
        }
        Ok(())
    }

    fn recv(&mut self) -> Result<Message, Error> {
        self.inner.recv()
    }

    fn try_recv(&mut self) -> Result<Option<Message>, Error> {
        self.inner.try_recv()
    }
}

impl<T: Transport> Drop for BufferedTransport<T> {
    fn drop(&mut self) {
        if !self.send_buffer.is_empty() {
            eprintln!(
                "Warning: BufferedTransport dropped with {} unsent messages",
                self.send_buffer.len()
            );
        }
        let _ = self.flush();
    }
}
