//! Length prefixed framing for stream transports.
//!
//! A WebSocket delivers whole messages, but a Unix domain socket delivers a
//! byte stream, so messages sent over one are prefixed with their length as a
//! big endian `u32`.

use crate::{Error, Result};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The largest message that will be written or read. Refusing anything larger
/// keeps a corrupt or hostile length prefix from causing a huge allocation.
pub const MAX_LEN: usize = 16 * 1024 * 1024;

/// Encodes `msg` and writes it with its length prefix.
pub async fn write<W, T>(writer: &mut W, msg: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes = crate::to_vec(msg)?;

    if bytes.len() > MAX_LEN {
        return Err(Error::FrameTooLarge(bytes.len()));
    }

    writer
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;

    Ok(())
}

/// Reads one message written by [`write`].
///
/// Returns `Ok(None)` when the peer closed the connection cleanly between
/// messages.
pub async fn read<R, T>(reader: &mut R) -> Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len_bytes = [0u8; 4];

    match reader.read_exact(&mut len_bytes).await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err.into()),
    }

    let len = u32::from_be_bytes(len_bytes) as usize;

    if len > MAX_LEN {
        return Err(Error::FrameTooLarge(len));
    }

    let mut bytes = vec![0u8; len];

    reader.read_exact(&mut bytes).await?;

    Ok(Some(crate::from_slice(&bytes)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AdminRequest, AdminResponse};

    #[tokio::test]
    async fn round_trips_several_messages_over_one_stream() {
        let mut stream = Vec::new();

        write(
            &mut stream,
            &AdminRequest::Refresh {
                hostname: Some("web-1".to_string()),
            },
        )
        .await
        .unwrap();
        write(&mut stream, &AdminRequest::Refresh { hostname: None })
            .await
            .unwrap();

        let mut reader = stream.as_slice();

        let first: AdminRequest = read(&mut reader).await.unwrap().unwrap();
        let second: AdminRequest = read(&mut reader).await.unwrap().unwrap();

        assert!(matches!(first, AdminRequest::Refresh { hostname: Some(h) } if h == "web-1"));
        assert!(matches!(second, AdminRequest::Refresh { hostname: None }));
    }

    #[tokio::test]
    async fn reports_a_clean_close_as_no_message() {
        let mut reader: &[u8] = &[];

        let msg: Option<AdminResponse> = read(&mut reader).await.unwrap();

        assert!(msg.is_none());
    }

    #[tokio::test]
    async fn rejects_an_oversized_length_prefix() {
        let stream = ((MAX_LEN + 1) as u32).to_be_bytes().to_vec();
        let mut reader = stream.as_slice();

        let err = read::<_, AdminResponse>(&mut reader).await.unwrap_err();

        assert!(matches!(err, Error::FrameTooLarge(_)));
    }
}
