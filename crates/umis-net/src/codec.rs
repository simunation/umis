//! Request-response codec for `/simu/sync/1`: one length-prefixed byte frame per
//! request and per response. The payload is borsh, encoded by the caller; this
//! layer never interprets it.

use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::StreamProtocol;
use std::io;

pub const SYNC_PROTOCOL: StreamProtocol = StreamProtocol::new("/simu/sync/1");

/// Largest request frame accepted.
pub const MAX_REQUEST: usize = 1 << 20;
/// Largest response frame accepted (block batches, checkpoints).
pub const MAX_RESPONSE: usize = 64 << 20;

/// Largest single write on a stream.
const WRITE_CHUNK: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, Default)]
pub struct BytesCodec;

async fn read_frame<T: AsyncRead + Unpin + Send>(io: &mut T, max: usize) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    io.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("frame of {len} bytes exceeds limit {max}")));
    }
    let mut buf = vec![0u8; len];
    io.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn write_frame<T: AsyncWrite + Unpin + Send>(io: &mut T, data: &[u8]) -> io::Result<()> {
    let len = u32::try_from(data.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    io.write_all(&len.to_be_bytes()).await?;
    // libp2p-webrtc rejects a single write above its maximum message size
    // (16 KiB) instead of writing partially, so write in small pieces.
    // Flushing after each piece keeps its framer from coalescing them.
    for piece in data.chunks(WRITE_CHUNK) {
        io.write_all(piece).await?;
        io.flush().await?;
    }
    // The request-response handler closes the stream; closing here as well
    // breaks WebRTC data channels.
    io.flush().await
}

impl libp2p::request_response::Codec for BytesCodec {
    type Protocol = StreamProtocol;
    type Request = Vec<u8>;
    type Response = Vec<u8>;

    async fn read_request<T>(&mut self, _: &Self::Protocol, io: &mut T) -> io::Result<Vec<u8>>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_frame(io, MAX_REQUEST).await
    }

    async fn read_response<T>(&mut self, _: &Self::Protocol, io: &mut T) -> io::Result<Vec<u8>>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_frame(io, MAX_RESPONSE).await
    }

    async fn write_request<T>(&mut self, _: &Self::Protocol, io: &mut T, req: Vec<u8>) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        write_frame(io, &req).await
    }

    async fn write_response<T>(&mut self, _: &Self::Protocol, io: &mut T, res: Vec<u8>) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        write_frame(io, &res).await
    }
}
