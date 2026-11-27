use crate::{AesCfb8, PacketReader, PacketWriter, ProtoError};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use std::io::{Read, Write};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const MAX_PACKET_SIZE: usize = 8 * 1024 * 1024;

/// Framed Minecraft Protocol 777 TCP stream supporting optional AES-128-CFB8 encryption
/// and optional zlib packet compression.
pub struct FramedConnection {
    stream: TcpStream,
    decryptor: Option<AesCfb8>,
    encryptor: Option<AesCfb8>,
    compression_threshold: Option<usize>,
}

impl FramedConnection {
    pub fn new(stream: TcpStream) -> Self {
        let _ = stream.set_nodelay(true);
        Self {
            stream,
            decryptor: None,
            encryptor: None,
            compression_threshold: None,
        }
    }

    pub fn enable_encryption(&mut self, shared_secret: [u8; 16]) {
        self.decryptor = Some(AesCfb8::new(shared_secret));
        self.encryptor = Some(AesCfb8::new(shared_secret));
    }

    pub fn set_compression_threshold(&mut self, threshold: Option<usize>) {
        self.compression_threshold = threshold;
    }

    async fn read_decrypted_byte(&mut self) -> Result<u8, ProtoError> {
        let mut b = [0u8; 1];
        match self.stream.read_exact(&mut b).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(ProtoError::Eof),
            Err(e) => return Err(ProtoError::Io(e)),
        }
        if let Some(dec) = &mut self.decryptor {
            dec.decrypt_in_place(&mut b);
        }
        Ok(b[0])
    }

    async fn read_decrypted_exact(&mut self, buf: &mut [u8]) -> Result<(), ProtoError> {
        match self.stream.read_exact(buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(ProtoError::Eof),
            Err(e) => return Err(ProtoError::Io(e)),
        }
        if let Some(dec) = &mut self.decryptor {
            dec.decrypt_in_place(buf);
        }
        Ok(())
    }

    async fn read_varint_async(&mut self) -> Result<i32, ProtoError> {
        let mut value: u32 = 0;
        let mut shift = 0;
        loop {
            let b = self.read_decrypted_byte().await?;
            value |= ((b & 0x7F) as u32) << shift;
            if (b & 0x80) == 0 {
                return Ok(value as i32);
            }
            shift += 7;
            if shift >= 35 {
                return Err(ProtoError::VarIntTooLong);
            }
        }
    }

    /// Read one complete packet frame and return the uncompressed `(packet_id + payload)` bytes.
    pub async fn read_raw_packet(&mut self) -> Result<Vec<u8>, ProtoError> {
        let frame_len = self.read_varint_async().await?;
        if frame_len < 0 {
            return Err(ProtoError::NegativeLength(frame_len));
        }
        if (frame_len as usize) > MAX_PACKET_SIZE {
            return Err(ProtoError::LengthExceeded(
                frame_len as usize,
                MAX_PACKET_SIZE,
            ));
        }
        let mut frame = vec![0u8; frame_len as usize];
        self.read_decrypted_exact(&mut frame).await?;

        if self.compression_threshold.is_some() {
            let mut r = PacketReader::new(&frame);
            let data_len = r.read_varint()?;
            if data_len < 0 {
                return Err(ProtoError::NegativeLength(data_len));
            }
            let rest = r.read_bytes(r.remaining())?;
            if data_len == 0 {
                Ok(rest.to_vec())
            } else {
                if (data_len as usize) > MAX_PACKET_SIZE {
                    return Err(ProtoError::LengthExceeded(
                        data_len as usize,
                        MAX_PACKET_SIZE,
                    ));
                }
                let mut dec = ZlibDecoder::new(rest);
                let mut out = Vec::with_capacity(data_len as usize);
                dec.read_to_end(&mut out)
                    .map_err(|e| ProtoError::Compression(e.to_string()))?;
                Ok(out)
            }
        } else {
            Ok(frame)
        }
    }

    /// Write one complete packet (`packet_id + payload` bytes), applying compression & encryption if active.
    pub async fn write_raw_packet(&mut self, raw_packet: &[u8]) -> Result<(), ProtoError> {
        let mut frame = PacketWriter::with_capacity(raw_packet.len() + 10);
        if let Some(threshold) = self.compression_threshold {
            if raw_packet.len() >= threshold {
                let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
                enc.write_all(raw_packet)
                    .map_err(|e| ProtoError::Compression(e.to_string()))?;
                let compressed = enc
                    .finish()
                    .map_err(|e| ProtoError::Compression(e.to_string()))?;
                let mut inner = PacketWriter::with_capacity(compressed.len() + 5);
                inner.write_varint(raw_packet.len() as i32);
                inner.write_raw(&compressed);
                let inner_bytes = inner.into_bytes();
                frame.write_varint(inner_bytes.len() as i32);
                frame.write_raw(&inner_bytes);
            } else {
                let mut inner = PacketWriter::with_capacity(raw_packet.len() + 1);
                inner.write_varint(0);
                inner.write_raw(raw_packet);
                let inner_bytes = inner.into_bytes();
                frame.write_varint(inner_bytes.len() as i32);
                frame.write_raw(&inner_bytes);
            }
        } else {
            frame.write_varint(raw_packet.len() as i32);
            frame.write_raw(raw_packet);
        }

        let mut wire = frame.into_bytes();
        if let Some(enc) = &mut self.encryptor {
            enc.encrypt_in_place(&mut wire);
        }
        self.stream.write_all(&wire).await?;
        self.stream.flush().await?;
        Ok(())
    }
}
