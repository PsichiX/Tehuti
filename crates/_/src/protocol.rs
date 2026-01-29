use crate::{
    channel::ChannelId,
    peer::{PeerId, PeerRoleId},
};
use std::io::{Error, ErrorKind, Read, Result, Write};

/// Control frames are low-level backend-side communication protocol for control
/// over a peer lifetime.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ProtocolControlFrame {
    Heartbeat { timestamp: u64 },
    CreatePeer(PeerId, PeerRoleId),
    DestroyPeer(PeerId),
}

impl ProtocolControlFrame {
    pub fn write(&self, stream: &mut dyn Write) -> Result<()> {
        match self {
            ProtocolControlFrame::Heartbeat { timestamp } => {
                stream.write_all(&[0])?;
                stream.write_all(&timestamp.to_le_bytes())?;
            }
            ProtocolControlFrame::CreatePeer(peer_id, role_id) => {
                stream.write_all(&[1])?;
                stream.write_all(&peer_id.id().to_le_bytes())?;
                stream.write_all(&role_id.id().to_le_bytes())?;
            }
            ProtocolControlFrame::DestroyPeer(peer_id) => {
                stream.write_all(&[2])?;
                stream.write_all(&peer_id.id().to_le_bytes())?;
            }
        }
        Ok(())
    }

    pub fn read(stream: &mut dyn Read) -> Result<ProtocolControlFrame> {
        let mut frame_type = [0u8; std::mem::size_of::<u8>()];
        stream.read_exact(&mut frame_type)?;
        match frame_type[0] {
            0 => {
                let mut timestamp_bytes = [0u8; std::mem::size_of::<u64>()];
                stream.read_exact(&mut timestamp_bytes)?;
                let timestamp = u64::from_le_bytes(timestamp_bytes);
                Ok(ProtocolControlFrame::Heartbeat { timestamp })
            }
            1 => {
                let mut peer_id_bytes = [0u8; std::mem::size_of::<u64>()];
                let mut role_id_bytes = [0u8; std::mem::size_of::<u64>()];
                stream.read_exact(&mut peer_id_bytes)?;
                stream.read_exact(&mut role_id_bytes)?;
                let peer_id = PeerId::new(u64::from_le_bytes(peer_id_bytes));
                let role_id = PeerRoleId::new(u64::from_le_bytes(role_id_bytes));
                Ok(ProtocolControlFrame::CreatePeer(peer_id, role_id))
            }
            2 => {
                let mut peer_id_bytes = [0u8; std::mem::size_of::<u64>()];
                stream.read_exact(&mut peer_id_bytes)?;
                let peer_id = PeerId::new(u64::from_le_bytes(peer_id_bytes));
                Ok(ProtocolControlFrame::DestroyPeer(peer_id))
            }
            _ => Err(Error::new(
                ErrorKind::InvalidData,
                "Unknown data frame type",
            )),
        }
    }
}

/// Packet frames are low-level backend-side communication protocol for
/// transporting data packets between peers over channels.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProtocolPacketFrame {
    pub peer_id: PeerId,
    pub channel_id: ChannelId,
    pub data: Vec<u8>,
}

impl ProtocolPacketFrame {
    pub fn write(&self, stream: &mut dyn Write) -> Result<()> {
        stream.write_all(&self.peer_id.id().to_le_bytes())?;
        stream.write_all(&self.channel_id.id().to_le_bytes())?;
        let data_len = self.data.len() as u32;
        stream.write_all(&data_len.to_le_bytes())?;
        stream.write_all(&self.data)?;
        Ok(())
    }

    pub fn read(stream: &mut dyn Read) -> Result<ProtocolPacketFrame> {
        let mut peer_id_bytes = [0u8; std::mem::size_of::<u64>()];
        let mut channel_id_bytes = [0u8; std::mem::size_of::<u64>()];
        let mut data_len_bytes = [0u8; std::mem::size_of::<u32>()];
        stream.read_exact(&mut peer_id_bytes)?;
        stream.read_exact(&mut channel_id_bytes)?;
        stream.read_exact(&mut data_len_bytes)?;
        let data_len = u32::from_le_bytes(data_len_bytes) as usize;
        let mut data = vec![0u8; data_len];
        stream.read_exact(&mut data)?;
        Ok(ProtocolPacketFrame {
            peer_id: PeerId::new(u64::from_le_bytes(peer_id_bytes)),
            channel_id: ChannelId::new(u64::from_le_bytes(channel_id_bytes)),
            data,
        })
    }
}

impl std::fmt::Debug for ProtocolPacketFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtocolPacketFrame")
            .field("peer_id", &self.peer_id)
            .field("channel_id", &self.channel_id)
            .field("data (size)", &self.data.len())
            .finish()
    }
}

/// Protocol frames are either control frames or packet frames.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProtocolFrame {
    Control(ProtocolControlFrame),
    Packet(ProtocolPacketFrame),
}

impl ProtocolFrame {
    pub fn write(&self, stream: &mut dyn Write) -> Result<()> {
        match self {
            ProtocolFrame::Control(control_frame) => {
                stream.write_all(&[1u8])?;
                control_frame.write(stream)?;
            }
            ProtocolFrame::Packet(packet_frame) => {
                stream.write_all(&[2u8])?;
                packet_frame.write(stream)?;
            }
        }
        Ok(())
    }

    pub fn read(stream: &mut dyn Read) -> Result<ProtocolFrame> {
        let mut frame_type = [0u8; std::mem::size_of::<u8>()];
        stream.read_exact(&mut frame_type)?;
        match frame_type[0] {
            1 => {
                let control_frame = ProtocolControlFrame::read(stream)?;
                Ok(ProtocolFrame::Control(control_frame))
            }
            2 => {
                let packet_frame = ProtocolPacketFrame::read(stream)?;
                Ok(ProtocolFrame::Packet(packet_frame))
            }
            _ => Err(Error::new(ErrorKind::InvalidData, "Unknown frame type")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_async() {
        fn is_send<T: Send>() {}

        is_send::<ProtocolControlFrame>();
        is_send::<ProtocolPacketFrame>();
    }

    #[test]
    fn test_protocol_heartbeat() {
        let frame = ProtocolControlFrame::Heartbeat {
            timestamp: 123456789,
        };
        let mut buffer = Vec::new();
        frame.write(&mut buffer).unwrap();
        let mut cursor = Cursor::new(buffer);
        let read_frame = ProtocolControlFrame::read(&mut cursor).unwrap();
        match read_frame {
            ProtocolControlFrame::Heartbeat { timestamp } => {
                assert_eq!(timestamp, 123456789);
            }
            _ => panic!("Expected Heartbeat frame"),
        }
    }

    #[test]
    fn test_protocol_create_peer() {
        let frame = ProtocolControlFrame::CreatePeer(PeerId::new(1), PeerRoleId::new(2));
        let mut buffer = Vec::new();
        frame.write(&mut buffer).unwrap();
        let mut cursor = Cursor::new(buffer);
        let read_frame = ProtocolControlFrame::read(&mut cursor).unwrap();
        match read_frame {
            ProtocolControlFrame::CreatePeer(peer_id, role_id) => {
                assert_eq!(peer_id.id(), 1);
                assert_eq!(role_id.id(), 2);
            }
            _ => panic!("Expected CreatePeer frame"),
        }
    }

    #[test]
    fn test_protocol_destroy_peer() {
        let frame = ProtocolControlFrame::DestroyPeer(PeerId::new(1));
        let mut buffer = Vec::new();
        frame.write(&mut buffer).unwrap();
        let mut cursor = Cursor::new(buffer);
        let read_frame = ProtocolControlFrame::read(&mut cursor).unwrap();
        match read_frame {
            ProtocolControlFrame::DestroyPeer(peer_id) => {
                assert_eq!(peer_id.id(), 1);
            }
            _ => panic!("Expected DestroyPeer frame"),
        }
    }

    #[test]
    fn test_protocol_packet() {
        let data = vec![1, 2, 3, 4, 5];
        let frame = ProtocolPacketFrame {
            peer_id: PeerId::new(1),
            channel_id: ChannelId::new(2),
            data: data.clone(),
        };
        let mut buffer = Vec::new();
        frame.write(&mut buffer).unwrap();
        let mut cursor = Cursor::new(buffer);
        let ProtocolPacketFrame {
            peer_id,
            channel_id,
            data: read_data,
        } = ProtocolPacketFrame::read(&mut cursor).unwrap();
        assert_eq!(peer_id.id(), 1);
        assert_eq!(channel_id.id(), 2);
        assert_eq!(read_data, data);
    }
}
