use serde::{Deserialize, Serialize};

/// Type for the single byte tag that precedes the payload.
pub type MessageTag = u8;

pub const TAG_REGISTER: MessageTag = 0x01;
pub const TAG_REGISTER_OK: MessageTag = 0x02;
pub const TAG_CONNECT: MessageTag = 0x03;
pub const TAG_TUNNEL_REQUEST: MessageTag = 0x04;
pub const TAG_TUNNEL_ACCEPT: MessageTag = 0x05;
pub const TAG_TUNNEL_READY: MessageTag = 0x06;
pub const TAG_TUNNEL_CLOSE: MessageTag = 0x07;
pub const TAG_STREAM_OPEN: MessageTag = 0x08;
pub const TAG_STREAM_CLOSE: MessageTag = 0x09;
pub const TAG_DATA: MessageTag = 0x0A;
pub const TAG_PING: MessageTag = 0x0B;
pub const TAG_PONG: MessageTag = 0x0C;
pub const TAG_ERROR: MessageTag = 0x0D;

/// Control messages in the tunnel protocol.
///
/// These are serialized using `bincode` inside the payload of a message.
/// `Data` messages are handled separately as raw bytes.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ControlMessage {
    Register,
    RegisterOk {
        agent_id: String,
    },
    Connect {
        target_id: String,
        remote_host: String,
        remote_port: u16,
    },
    TunnelRequest {
        session_id: String,
        remote_host: String,
        remote_port: u16,
    },
    TunnelAccept {
        session_id: String,
    },
    TunnelReady {
        session_id: String,
    },
    TunnelClose {
        session_id: String,
    },
    StreamOpen {
        session_id: String,
        stream_id: String,
    },
    StreamClose {
        session_id: String,
        stream_id: String,
    },
    Ping,
    Pong,
    Error {
        message: String,
    },
}

impl ControlMessage {
    /// Returns the corresponding 1-byte tag for this control message.
    pub fn tag(&self) -> MessageTag {
        match self {
            Self::Register => TAG_REGISTER,
            Self::RegisterOk { .. } => TAG_REGISTER_OK,
            Self::Connect { .. } => TAG_CONNECT,
            Self::TunnelRequest { .. } => TAG_TUNNEL_REQUEST,
            Self::TunnelAccept { .. } => TAG_TUNNEL_ACCEPT,
            Self::TunnelReady { .. } => TAG_TUNNEL_READY,
            Self::TunnelClose { .. } => TAG_TUNNEL_CLOSE,
            Self::StreamOpen { .. } => TAG_STREAM_OPEN,
            Self::StreamClose { .. } => TAG_STREAM_CLOSE,
            Self::Ping => TAG_PING,
            Self::Pong => TAG_PONG,
            Self::Error { .. } => TAG_ERROR,
        }
    }

    /// Serializes the control message into bytes: `[1 byte: tag][payload: bincode]`
    pub fn serialize(&self) -> Result<Vec<u8>, bincode::Error> {
        let tag = self.tag();
        let mut payload = bincode::serialize(self)?;
        let mut buf = Vec::with_capacity(1 + payload.len());
        buf.push(tag);
        buf.append(&mut payload);
        Ok(buf)
    }

    /// Deserializes a control message from a byte slice.
    ///
    /// The slice must start with the 1-byte tag.
    pub fn deserialize(buf: &[u8]) -> Result<Self, String> {
        if buf.is_empty() {
            return Err("Empty buffer".into());
        }
        // The tag is essentially duplicate info since bincode also serializes
        // the enum variant index, but keeping the tag explicitly satisfies
        // the designed binary protocol format.
        let tag = buf[0];
        if tag == TAG_DATA {
            return Err("Cannot deserialize Data message as ControlMessage".into());
        }

        let msg: Self = bincode::deserialize(&buf[1..]).map_err(|e| e.to_string())?;
        Ok(msg)
    }
}

/// Packs a raw DATA message into the defined binary protocol format.
///
/// The binary layout of a DATA message is constructed as follows:
/// - `[1 byte]` : The message tag representing `DATA` (`0x0A`).
/// - `[8 bytes]`: The `session_id`, uniquely identifying the active tunnel session.
/// - `[8 bytes]`: The `stream_id`, uniquely identifying the TCP stream within the session.
/// - `[n bytes]`: The actual data payload to be transmitted.
///
/// # Arguments
///
/// * `session_id` - An 8-byte array serving as the session identifier.
/// * `stream_id` - An 8-byte array serving as the individual stream identifier.
/// * `payload` - A byte slice containing the application data payload.
pub fn pack_data_message(session_id: [u8; 8], stream_id: [u8; 8], payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + 8 + 8 + payload.len());
    buf.push(TAG_DATA);
    buf.extend_from_slice(&session_id);
    buf.extend_from_slice(&stream_id);
    buf.extend_from_slice(payload);
    buf
}

pub fn unpack_data_message(buf: &[u8]) -> Option<([u8; 8], [u8; 8], &[u8])> {
    if buf.len() < 17 || buf[0] != TAG_DATA {
        return None;
    }
    let mut session_id = [0u8; 8];
    session_id.copy_from_slice(&buf[1..9]);

    let mut stream_id = [0u8; 8];
    stream_id.copy_from_slice(&buf[9..17]);

    let payload = &buf[17..];
    Some((session_id, stream_id, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_control_message_serialization() {
        let msg = ControlMessage::RegisterOk {
            agent_id: "A3F8-B2C1".to_string(),
        };
        let bytes = msg.serialize().unwrap();
        assert_eq!(bytes[0], TAG_REGISTER_OK);

        let decoded = ControlMessage::deserialize(&bytes).unwrap();
        match decoded {
            ControlMessage::RegisterOk { agent_id } => {
                assert_eq!(agent_id, "A3F8-B2C1");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_data_message() {
        let session = [1, 2, 3, 4, 5, 6, 7, 8];
        let stream = [8, 7, 6, 5, 4, 3, 2, 1];
        let payload = b"hello world";

        let packed = pack_data_message(session, stream, payload);
        assert_eq!(packed[0], TAG_DATA);

        let (s, st, p) = unpack_data_message(&packed).unwrap();
        assert_eq!(s, session);
        assert_eq!(st, stream);
        assert_eq!(p, payload);
    }
}
