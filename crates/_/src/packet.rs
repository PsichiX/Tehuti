use std::error::Error;

pub trait PacketSerializer<Message> {
    fn encode(&mut self, message: &Message, buffer: &mut Vec<u8>) -> Result<(), Box<dyn Error>>;
    fn decode(&mut self, buffer: &[u8]) -> Result<Message, Box<dyn Error>>;
}

pub struct RawBytesPacketSerializer;

impl PacketSerializer<Vec<u8>> for RawBytesPacketSerializer {
    fn encode(&mut self, message: &Vec<u8>, buffer: &mut Vec<u8>) -> Result<(), Box<dyn Error>> {
        buffer.extend_from_slice(message);
        Ok(())
    }

    fn decode(&mut self, buffer: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
        Ok(buffer.to_vec())
    }
}
