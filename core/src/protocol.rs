use anyhow::{Result,ensure};
use tokio::io::{AsyncBufRead,AsyncBufReadExt};

/// Bound memory before appending data, including a peer that never sends '\n'.
pub async fn line<R:AsyncBufRead+Unpin>(reader:&mut R,limit:usize)->Result<Option<Vec<u8>>>{
    let mut output=vec![];
    loop{
        let available=reader.fill_buf().await?;
        if available.is_empty(){return Ok(if output.is_empty(){None}else{Some(output)});}
        let count=available.iter().position(|&c|c==b'\n').map(|n|n+1).unwrap_or(available.len());
        let complete=available[count-1]==b'\n';
        ensure!(output.len()+count<=limit,"protocol line exceeds size limit");
        output.extend_from_slice(&available[..count]);reader.consume(count);
        if complete{return Ok(Some(output));}
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use tokio::io::BufReader;
    #[tokio::test]
    async fn split_lines_eof_and_size_limit()->Result<()>{
        let mut reader=BufReader::with_capacity(2,&b"abc\ndef"[..]);
        assert_eq!(line(&mut reader,4).await?,Some(b"abc\n".to_vec()));
        assert_eq!(line(&mut reader,4).await?,Some(b"def".to_vec()));
        assert_eq!(line(&mut reader,4).await?,None);
        let mut reader=BufReader::with_capacity(2,&b"abcde"[..]);
        assert!(line(&mut reader,4).await.is_err());
        let mut reader=BufReader::new(&b"abc\n"[..]);
        assert!(line(&mut reader,3).await.is_err());
        Ok(())
    }
}
