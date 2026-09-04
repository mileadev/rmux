use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use rmux_ipc::{is_peer_disconnect, LocalStream};
use rmux_proto::AttachFrameDecoder;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, ReadHalf, WriteHalf};
use tokio::sync::Mutex;
use tokio::time::timeout;

const ATTACH_READ_BUFFER_SIZE: usize = 8192;
const ATTACH_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) struct AttachTransport {
    reader: Mutex<Box<dyn AsyncRead + Send + Unpin>>,
    writer: Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
}

pub(super) enum TryAttachRead {
    Read,
    Closed,
    WouldBlock,
}

impl AttachTransport {
    pub(super) fn from_io<T>(stream: T) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let (reader, writer) = tokio::io::split(stream);
        Self::from_split(reader, writer)
    }

    pub(super) async fn read_into(&self, decoder: &mut AttachFrameDecoder) -> io::Result<bool> {
        let mut buffer = [0_u8; ATTACH_READ_BUFFER_SIZE];
        let mut reader = self.reader.lock().await;
        match reader.read(&mut buffer).await {
            Ok(0) => Ok(false),
            Ok(bytes_read) => {
                decoder.push_bytes(&buffer[..bytes_read]);
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn try_read_into(
        &self,
        decoder: &mut AttachFrameDecoder,
    ) -> io::Result<TryAttachRead> {
        let Ok(mut reader) = self.reader.try_lock() else {
            return Ok(TryAttachRead::WouldBlock);
        };
        let mut buffer = [0_u8; ATTACH_READ_BUFFER_SIZE];
        let mut read_buffer = ReadBuf::new(&mut buffer);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        match Pin::new(&mut **reader).poll_read(&mut context, &mut read_buffer) {
            Poll::Ready(Ok(())) if read_buffer.filled().is_empty() => Ok(TryAttachRead::Closed),
            Poll::Ready(Ok(())) => {
                decoder.push_bytes(read_buffer.filled());
                Ok(TryAttachRead::Read)
            }
            Poll::Ready(Err(error)) => Err(error),
            Poll::Pending => Ok(TryAttachRead::WouldBlock),
        }
    }

    pub(super) async fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        self.write_all_with_timeout(bytes, ATTACH_WRITE_TIMEOUT)
            .await
    }

    async fn write_all_with_timeout(
        &self,
        bytes: &[u8],
        write_timeout: std::time::Duration,
    ) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut writer = self.writer.lock().await;
        match timeout(write_timeout, writer.write_all(bytes)).await {
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "attach client did not drain server output",
            )),
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) if is_peer_disconnect(&error) => Ok(()),
            Ok(Err(error)) => Err(error),
        }
    }

    fn from_split<T>(reader: ReadHalf<T>, writer: WriteHalf<T>) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        Self {
            reader: Mutex::new(Box::new(reader)),
            writer: Mutex::new(Box::new(writer)),
        }
    }
}

impl From<LocalStream> for AttachTransport {
    fn from(stream: LocalStream) -> Self {
        Self::from_io(stream)
    }
}

#[cfg(test)]
mod timeout_tests {
    use std::io;
    use std::time::{Duration, Instant};

    use super::AttachTransport;

    #[tokio::test]
    async fn saturated_non_reader_is_bounded_by_the_write_timeout() {
        let (server, _non_reader) = tokio::io::duplex(1);
        let transport = AttachTransport::from_io(server);
        let started = Instant::now();

        let error = transport
            .write_all_with_timeout(&vec![b'x'; 4096], Duration::from_millis(25))
            .await
            .expect_err("a saturated attach peer must time out");

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a non-reading attach peer must not hold shutdown indefinitely"
        );
    }
}
