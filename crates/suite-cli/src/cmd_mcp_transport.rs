use super::*;

pub(crate) const MAX_MCP_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_MCP_HEADER_BYTES: usize = 64 * 1024;

pub(crate) fn render_command_preview(command: &str, args: &[String]) -> String {
    std::iter::once(command.to_string())
        .chain(args.iter().map(|arg| {
            if arg.contains(' ') {
                format!("{arg:?}")
            } else {
                arg.clone()
            }
        }))
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpMessageFraming {
    ContentLength,
    NewlineJson,
}

pub(crate) fn read_message(
    reader: &mut impl BufRead,
) -> Result<Option<(Value, McpMessageFraming)>> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = read_line_limited(reader, &mut line, MAX_MCP_MESSAGE_BYTES + 2)?;
        if read == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            if trimmed.len() > MAX_MCP_MESSAGE_BYTES {
                return Err(anyhow!(
                    "MCP message exceeds {} byte limit",
                    MAX_MCP_MESSAGE_BYTES
                ));
            }
            let value = serde_json::from_str(trimmed)?;
            return Ok(Some((value, McpMessageFraming::NewlineJson)));
        }
        return read_header_framed_message(reader, trimmed);
    }
}

fn read_header_framed_message(
    reader: &mut impl BufRead,
    first_line: &str,
) -> Result<Option<(Value, McpMessageFraming)>> {
    let mut content_length = None::<usize>;
    parse_header_line(first_line, &mut content_length)?;
    let mut header_bytes = first_line.len();
    if header_bytes > MAX_MCP_HEADER_BYTES {
        return Err(anyhow!(
            "MCP header exceeds {MAX_MCP_HEADER_BYTES} byte limit"
        ));
    }
    let mut line = String::new();
    loop {
        line.clear();
        let read = read_line_limited(reader, &mut line, MAX_MCP_HEADER_BYTES + 1)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "MCP Content-Length header ended before its blank-line terminator",
            )
            .into());
        }
        header_bytes = header_bytes.saturating_add(read);
        if header_bytes > MAX_MCP_HEADER_BYTES {
            return Err(anyhow!(
                "MCP header exceeds {MAX_MCP_HEADER_BYTES} byte limit"
            ));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            parse_header(name, value, &mut content_length)?;
        }
    }

    let content_length =
        content_length.ok_or_else(|| anyhow!("missing Content-Length header in MCP request"))?;
    if content_length > MAX_MCP_MESSAGE_BYTES {
        return Err(anyhow!(
            "MCP message length {content_length} exceeds {} byte limit",
            MAX_MCP_MESSAGE_BYTES
        ));
    }
    let mut body = vec![0_u8; content_length];
    reader.read_exact(&mut body)?;
    Ok(Some((
        serde_json::from_slice(&body)?,
        McpMessageFraming::ContentLength,
    )))
}

fn read_line_limited(reader: &mut impl BufRead, line: &mut String, limit: usize) -> Result<usize> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if bytes.len().saturating_add(take) > limit {
            return Err(anyhow!("MCP line exceeds {limit} byte limit"));
        }
        let complete = available[..take].ends_with(b"\n");
        bytes.extend_from_slice(&available[..take]);
        reader.consume(take);
        if complete {
            break;
        }
    }
    let read = bytes.len();
    *line = String::from_utf8(bytes).context("MCP framing line is not valid UTF-8")?;
    Ok(read)
}

fn parse_header_line(line: &str, content_length: &mut Option<usize>) -> Result<()> {
    let Some((name, value)) = line.split_once(':') else {
        return Err(anyhow!("missing Content-Length header in MCP request"));
    };
    parse_header(name, value, content_length)
}

fn parse_header(name: &str, value: &str, content_length: &mut Option<usize>) -> Result<()> {
    if name.eq_ignore_ascii_case("content-length") {
        *content_length = Some(value.trim().parse::<usize>()?);
    }
    Ok(())
}

pub(crate) fn write_message(
    writer: &mut impl Write,
    value: &Value,
    framing: McpMessageFraming,
) -> Result<()> {
    let body = serde_json::to_vec(value)?;
    if body.len() > MAX_MCP_MESSAGE_BYTES {
        return Err(anyhow!(
            "MCP message length {} exceeds {} byte limit",
            body.len(),
            MAX_MCP_MESSAGE_BYTES
        ));
    }
    match framing {
        McpMessageFraming::ContentLength => {
            write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
            writer.write_all(&body)?;
        }
        McpMessageFraming::NewlineJson => {
            writer.write_all(&body)?;
            writer.write_all(b"\n")?;
        }
    }
    writer.flush()?;
    Ok(())
}

pub(crate) async fn read_message_async<R>(
    reader: &mut R,
) -> Result<Option<(Value, McpMessageFraming)>>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut line = String::new();
    loop {
        line.clear();
        let read = read_line_limited_async(reader, &mut line, MAX_MCP_MESSAGE_BYTES + 2).await?;
        if read == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            if trimmed.len() > MAX_MCP_MESSAGE_BYTES {
                return Err(anyhow!(
                    "MCP message exceeds {} byte limit",
                    MAX_MCP_MESSAGE_BYTES
                ));
            }
            let value = serde_json::from_str(trimmed)?;
            return Ok(Some((value, McpMessageFraming::NewlineJson)));
        }
        return read_header_framed_message_async(reader, trimmed).await;
    }
}

async fn read_header_framed_message_async<R>(
    reader: &mut R,
    first_line: &str,
) -> Result<Option<(Value, McpMessageFraming)>>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let mut content_length = None::<usize>;
    parse_header_line(first_line, &mut content_length)?;
    let mut header_bytes = first_line.len();
    if header_bytes > MAX_MCP_HEADER_BYTES {
        return Err(anyhow!(
            "MCP header exceeds {MAX_MCP_HEADER_BYTES} byte limit"
        ));
    }
    let mut line = String::new();
    loop {
        line.clear();
        let read = read_line_limited_async(reader, &mut line, MAX_MCP_HEADER_BYTES + 1).await?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "MCP Content-Length header ended before its blank-line terminator",
            )
            .into());
        }
        header_bytes = header_bytes.saturating_add(read);
        if header_bytes > MAX_MCP_HEADER_BYTES {
            return Err(anyhow!(
                "MCP header exceeds {MAX_MCP_HEADER_BYTES} byte limit"
            ));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            parse_header(name, value, &mut content_length)?;
        }
    }

    let content_length =
        content_length.ok_or_else(|| anyhow!("missing Content-Length header in MCP request"))?;
    if content_length > MAX_MCP_MESSAGE_BYTES {
        return Err(anyhow!(
            "MCP message length {content_length} exceeds {} byte limit",
            MAX_MCP_MESSAGE_BYTES
        ));
    }
    let mut body = vec![0_u8; content_length];
    reader.read_exact(&mut body).await?;
    Ok(Some((
        serde_json::from_slice(&body)?,
        McpMessageFraming::ContentLength,
    )))
}

async fn read_line_limited_async<R>(
    reader: &mut R,
    line: &mut String,
    limit: usize,
) -> Result<usize>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;

    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            break;
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if bytes.len().saturating_add(take) > limit {
            return Err(anyhow!("MCP line exceeds {limit} byte limit"));
        }
        let complete = available[..take].ends_with(b"\n");
        bytes.extend_from_slice(&available[..take]);
        reader.consume(take);
        if complete {
            break;
        }
    }
    let read = bytes.len();
    *line = String::from_utf8(bytes).context("MCP framing line is not valid UTF-8")?;
    Ok(read)
}

pub(crate) async fn write_message_async<W>(
    writer: &mut W,
    value: &Value,
    framing: McpMessageFraming,
) -> Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;

    let body = serde_json::to_vec(value)?;
    if body.len() > MAX_MCP_MESSAGE_BYTES {
        return Err(anyhow!(
            "MCP message length {} exceeds {} byte limit",
            body.len(),
            MAX_MCP_MESSAGE_BYTES
        ));
    }
    match framing {
        McpMessageFraming::ContentLength => {
            writer
                .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
                .await?;
            writer.write_all(&body).await?;
        }
        McpMessageFraming::NewlineJson => {
            writer.write_all(&body).await?;
            writer.write_all(b"\n").await?;
        }
    }
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod async_tests {
    use super::*;

    #[test]
    fn sync_reader_rejects_oversized_content_length_before_allocating_body() {
        let input = format!("Content-Length: {}\r\n\r\n", MAX_MCP_MESSAGE_BYTES + 1);
        let error = read_message(&mut std::io::BufReader::new(input.as_bytes())).unwrap_err();

        assert!(error.to_string().contains("exceeds"));
    }

    #[test]
    fn sync_reader_rejects_eof_before_content_length_header_terminator() {
        let input = b"Content-Length: 2\r\nX-Test: incomplete";
        let error = read_message(&mut std::io::BufReader::new(&input[..])).unwrap_err();

        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::UnexpectedEof)
        );
    }

    #[test]
    fn sync_line_reader_stops_at_limit_without_buffering_unbounded_input() {
        let mut line = String::new();
        let error = read_line_limited(&mut std::io::BufReader::new(&b"12345"[..]), &mut line, 4)
            .unwrap_err();

        assert!(error.to_string().contains("4 byte limit"));
        assert!(line.is_empty());
    }

    #[test]
    fn async_reader_rejects_eof_before_content_length_header_terminator() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let error = runtime.block_on(async {
            let input = b"Content-Length: 2\r\nX-Test: incomplete";
            read_message_async(&mut tokio::io::BufReader::new(&input[..]))
                .await
                .unwrap_err()
        });

        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::UnexpectedEof)
        );
    }

    #[test]
    fn async_reader_rejects_truncated_content_length_frame() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let error = runtime.block_on(async {
            let input = b"Content-Length: 5\r\n\r\n{}";
            read_message_async(&mut tokio::io::BufReader::new(&input[..]))
                .await
                .unwrap_err()
        });

        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::UnexpectedEof)
        );
    }

    #[test]
    fn async_line_reader_stops_at_limit_without_buffering_unbounded_input() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut line = String::new();
        let error = runtime.block_on(async {
            read_line_limited_async(&mut tokio::io::BufReader::new(&b"12345"[..]), &mut line, 4)
                .await
                .unwrap_err()
        });

        assert!(error.to_string().contains("4 byte limit"));
        assert!(line.is_empty());
    }
}
