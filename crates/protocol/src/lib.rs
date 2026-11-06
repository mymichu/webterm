//! Wire protocol shared by the browser client and the backend.
//!
//! Every WebSocket message is a binary frame whose first byte is a tag.
//! Raw terminal bytes are carried unmodified after the tag so that the hot
//! path (PTY output → screen) does no extra copying or encoding work.

const TAG_DATA: u8 = 0;
const TAG_RESIZE: u8 = 1;
const TAG_EXIT: u8 = 2;

/// Messages sent by the browser to the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMsg {
    /// Bytes typed/pasted by the user, to be written to the PTY.
    Input(Vec<u8>),
    /// The visible grid changed size.
    Resize { cols: u16, rows: u16 },
}

/// Messages sent by the backend to the browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMsg {
    /// Bytes produced by the PTY.
    Output(Vec<u8>),
    /// The shell process exited with the given status code.
    Exit(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError;

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("malformed protocol frame")
    }
}

impl std::error::Error for DecodeError {}

impl ClientMsg {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            ClientMsg::Input(data) => tagged(TAG_DATA, data),
            ClientMsg::Resize { cols, rows } => {
                let mut out = Vec::with_capacity(5);
                out.push(TAG_RESIZE);
                out.extend_from_slice(&cols.to_be_bytes());
                out.extend_from_slice(&rows.to_be_bytes());
                out
            }
        }
    }

    pub fn decode(frame: &[u8]) -> Result<Self, DecodeError> {
        match frame.split_first() {
            Some((&TAG_DATA, rest)) => Ok(ClientMsg::Input(rest.to_vec())),
            Some((&TAG_RESIZE, [c0, c1, r0, r1])) => Ok(ClientMsg::Resize {
                cols: u16::from_be_bytes([*c0, *c1]),
                rows: u16::from_be_bytes([*r0, *r1]),
            }),
            _ => Err(DecodeError),
        }
    }
}

impl ServerMsg {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            ServerMsg::Output(data) => tagged(TAG_DATA, data),
            ServerMsg::Exit(code) => {
                let mut out = Vec::with_capacity(5);
                out.push(TAG_EXIT);
                out.extend_from_slice(&code.to_be_bytes());
                out
            }
        }
    }

    pub fn decode(frame: &[u8]) -> Result<Self, DecodeError> {
        match frame.split_first() {
            Some((&TAG_DATA, rest)) => Ok(ServerMsg::Output(rest.to_vec())),
            Some((&TAG_EXIT, [a, b, c, d])) => Ok(ServerMsg::Exit(i32::from_be_bytes([*a, *b, *c, *d]))),
            _ => Err(DecodeError),
        }
    }
}

fn tagged(tag: u8, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 1);
    out.push(tag);
    out.extend_from_slice(data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for msg in [ClientMsg::Input(b"ls -la\r".to_vec()), ClientMsg::Resize { cols: 120, rows: 40 }] {
            assert_eq!(ClientMsg::decode(&msg.encode()), Ok(msg));
        }
        for msg in [ServerMsg::Output(vec![0x1b, b'[', b'm']), ServerMsg::Exit(-1)] {
            assert_eq!(ServerMsg::decode(&msg.encode()), Ok(msg));
        }
        assert!(ClientMsg::decode(&[]).is_err());
        assert!(ClientMsg::decode(&[TAG_RESIZE, 1]).is_err());
    }
}
