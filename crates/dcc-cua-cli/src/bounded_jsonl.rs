use std::io;

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

pub(crate) struct BoundedJsonlReader<R> {
    reader: R,
}

impl<R> BoundedJsonlReader<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self { reader }
    }
}

impl<R: AsyncBufRead + Unpin> BoundedJsonlReader<R> {
    pub(crate) async fn next_line(&mut self) -> io::Result<Option<String>> {
        let mut line = Vec::new();
        loop {
            let chunk = self.reader.fill_buf().await?;
            if chunk.is_empty() {
                return if line.is_empty() {
                    Ok(None)
                } else {
                    String::from_utf8(line)
                        .map(Some)
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
                };
            }
            let newline = chunk.iter().position(|byte| *byte == b'\n');
            let take = newline.map_or(chunk.len(), |index| index + 1);
            if line.len().saturating_add(take) > dcc_cua_protocol::MAX_JSON_FRAME_BYTES {
                self.reader.consume(take);
                if newline.is_none() {
                    self.discard_until_newline().await?;
                }
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "JSONL request exceeds maximum frame size of {} bytes",
                        dcc_cua_protocol::MAX_JSON_FRAME_BYTES
                    ),
                ));
            }
            line.extend_from_slice(&chunk[..take]);
            self.reader.consume(take);
            if newline.is_some() {
                return String::from_utf8(line)
                    .map(Some)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
            }
        }
    }

    async fn discard_until_newline(&mut self) -> io::Result<()> {
        loop {
            let chunk = self.reader.fill_buf().await?;
            if chunk.is_empty() {
                return Ok(());
            }
            let chunk_len = chunk.len();
            let take = chunk
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(chunk_len, |index| index + 1);
            let ends_with_newline = chunk.last() == Some(&b'\n');
            self.reader.consume(take);
            if take < chunk_len || ends_with_newline {
                return Ok(());
            }
        }
    }
}
