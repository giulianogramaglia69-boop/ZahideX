// SPDX-License-Identifier: GPL-3.0-or-later
//! Framing dei messaggi sul trasporto.
//!
//! Ogni frame è: `[magic "ZX01" (4 byte)] [lunghezza payload u32 LE] [payload postcard]`.
//! La lunghezza massima del payload è [`MAX_FRAME_BYTES`] (16 MiB).

use serde::{Serialize, de::DeserializeOwned};

use crate::error::ProtocolError;

/// Magic che apre ogni frame.
pub const MAGIC: [u8; 4] = *b"ZX01";

/// Lunghezza massima di un payload in byte.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Lunghezza dell'header (magic + lunghezza).
const HEADER_LEN: usize = 8;

/// Serializza un messaggio in un frame completo.
pub fn encode_frame<T: Serialize>(msg: &T) -> Result<Vec<u8>, ProtocolError> {
    let payload = postcard::to_allocvec(msg)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge { len: payload.len() });
    }
    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
    frame.extend_from_slice(&MAGIC);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decodifica un singolo frame da `bytes`, restituendo il messaggio e i byte consumati.
pub fn decode_frame<T: DeserializeOwned>(bytes: &[u8]) -> Result<(T, usize), ProtocolError> {
    let mut decoder = FrameDecoder::new();
    decoder.push(bytes);
    match decoder.next_message()? {
        Some(msg) => Ok((msg, bytes.len() - decoder.buffered_len())),
        None => Err(ProtocolError::Truncated {
            needed: needed_bytes(bytes),
            have: bytes.len(),
        }),
    }
}

/// Quanti byte servono per completare il frame contenuto (anche parzialmente) in `bytes`.
fn needed_bytes(bytes: &[u8]) -> usize {
    if bytes.len() < HEADER_LEN {
        return HEADER_LEN;
    }
    let len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    HEADER_LEN + len
}

/// Decodificatore incrementale: accetta pezzi di stream e restituisce
/// un messaggio alla volta quando un frame è completo.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    /// Crea un decodificatore vuoto.
    pub fn new() -> Self {
        Self::default()
    }

    /// Aggiunge byte letti dallo stream.
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Byte in attesa di decodifica.
    pub fn buffered_len(&self) -> usize {
        self.buf.len()
    }

    /// Restituisce il prossimo messaggio completo, se presente.
    pub fn next_message<T: DeserializeOwned>(&mut self) -> Result<Option<T>, ProtocolError> {
        if self.buf.len() < HEADER_LEN {
            return Ok(None);
        }
        if self.buf[..4] != MAGIC {
            let mut magic = [0u8; 4];
            magic.copy_from_slice(&self.buf[..4]);
            return Err(ProtocolError::InvalidMagic(magic));
        }
        let len = u32::from_le_bytes([self.buf[4], self.buf[5], self.buf[6], self.buf[7]]) as usize;
        if len > MAX_FRAME_BYTES {
            return Err(ProtocolError::FrameTooLarge { len });
        }
        let total = HEADER_LEN + len;
        if self.buf.len() < total {
            return Ok(None);
        }
        let msg = postcard::from_bytes(&self.buf[HEADER_LEN..total])?;
        self.buf.drain(..total);
        Ok(Some(msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{ClientMessage, ServerMessage};
    use crate::types::{BufferInfo, PixelFormat, Rect, WindowId};

    fn all_client_messages() -> Vec<ClientMessage> {
        vec![
            ClientMessage::Hello {
                versions: vec![1, 2],
                client_name: "test".into(),
            },
            ClientMessage::CreateWindow {
                width: 800,
                height: 600,
                title: "titolo".into(),
            },
            ClientMessage::DestroyWindow {
                window_id: WindowId(3),
            },
            ClientMessage::CommitBuffer {
                window_id: WindowId(3),
                buffer: BufferInfo {
                    width: 64,
                    height: 64,
                    stride: 256,
                    format: PixelFormat::Xrgb8888,
                },
                damage: Rect::new(0, 0, 64, 64),
            },
            ClientMessage::SetTitle {
                window_id: WindowId(3),
                title: "nuovo".into(),
            },
            ClientMessage::RequestResize {
                window_id: WindowId(3),
                width: 100,
                height: 100,
            },
            ClientMessage::SetFloating {
                window_id: WindowId(3),
                floating: true,
            },
            ClientMessage::Pong,
        ]
    }

    fn all_server_messages() -> Vec<ServerMessage> {
        vec![
            ServerMessage::HelloOk {
                version: 1,
                output_width: 1920,
                output_height: 1080,
            },
            ServerMessage::HelloError {
                reason: "no".into(),
            },
            ServerMessage::WindowCreated {
                window_id: WindowId(1),
                width: 800,
                height: 600,
            },
            ServerMessage::Configure {
                window_id: WindowId(1),
                width: 400,
                height: 300,
            },
            ServerMessage::Focus {
                window_id: Some(WindowId(1)),
            },
            ServerMessage::Focus { window_id: None },
            ServerMessage::PointerEnter {
                window_id: WindowId(1),
            },
            ServerMessage::PointerLeave {
                window_id: WindowId(1),
            },
            ServerMessage::PointerMotion {
                window_id: WindowId(1),
                x: 10,
                y: -5,
            },
            ServerMessage::PointerButton {
                window_id: WindowId(1),
                button: 272,
                pressed: true,
            },
            ServerMessage::Key {
                window_id: WindowId(1),
                keycode: 30,
                pressed: true,
            },
            ServerMessage::CloseWindow {
                window_id: WindowId(1),
            },
            ServerMessage::Ping,
            ServerMessage::Error {
                message: "boom".into(),
            },
        ]
    }

    #[test]
    fn roundtrip_all_client_messages() {
        for msg in all_client_messages() {
            let frame = encode_frame(&msg).unwrap();
            let (decoded, consumed): (ClientMessage, usize) = decode_frame(&frame).unwrap();
            assert_eq!(decoded, msg);
            assert_eq!(consumed, frame.len());
        }
    }

    #[test]
    fn roundtrip_all_server_messages() {
        for msg in all_server_messages() {
            let frame = encode_frame(&msg).unwrap();
            let (decoded, consumed): (ServerMessage, usize) = decode_frame(&frame).unwrap();
            assert_eq!(decoded, msg);
            assert_eq!(consumed, frame.len());
        }
    }

    #[test]
    fn decoder_handles_split_reads() {
        let msg = ClientMessage::CreateWindow {
            width: 320,
            height: 240,
            title: "split".into(),
        };
        let frame = encode_frame(&msg).unwrap();
        let mut decoder = FrameDecoder::new();
        for (i, byte) in frame.iter().enumerate() {
            decoder.push(std::slice::from_ref(byte));
            let decoded: Option<ClientMessage> = decoder.next_message().unwrap();
            if i + 1 < frame.len() {
                assert!(
                    decoded.is_none(),
                    "messaggio decodificato troppo presto al byte {i}"
                );
            } else {
                assert_eq!(decoded.as_ref(), Some(&msg));
            }
        }
        assert_eq!(decoder.buffered_len(), 0);
    }

    #[test]
    fn decoder_handles_two_frames_in_one_push() {
        let first = ServerMessage::Ping;
        let second = ServerMessage::Focus { window_id: None };
        let mut bytes = encode_frame(&first).unwrap();
        bytes.extend_from_slice(&encode_frame(&second).unwrap());
        let mut decoder = FrameDecoder::new();
        decoder.push(&bytes);
        assert_eq!(
            decoder.next_message::<ServerMessage>().unwrap(),
            Some(first)
        );
        assert_eq!(
            decoder.next_message::<ServerMessage>().unwrap(),
            Some(second)
        );
        assert_eq!(decoder.next_message::<ServerMessage>().unwrap(), None);
    }

    #[test]
    fn decoder_rejects_bad_magic() {
        let mut frame = encode_frame(&ServerMessage::Ping).unwrap();
        frame[0] = b'X';
        let err = decode_frame::<ServerMessage>(&frame).unwrap_err();
        assert!(matches!(err, ProtocolError::InvalidMagic(_)));
    }

    #[test]
    fn decoder_reports_truncated() {
        let frame = encode_frame(&ServerMessage::Ping).unwrap();
        let err = decode_frame::<ServerMessage>(&frame[..4]).unwrap_err();
        assert!(matches!(err, ProtocolError::Truncated { .. }));
    }

    #[test]
    fn encoder_rejects_oversized_payload() {
        let big = ClientMessage::Hello {
            versions: vec![1],
            client_name: "x".repeat(MAX_FRAME_BYTES + 1),
        };
        let err = encode_frame(&big).unwrap_err();
        assert!(matches!(err, ProtocolError::FrameTooLarge { .. }));
    }
}
