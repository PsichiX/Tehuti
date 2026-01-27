use crate::codec::Codec;
use std::{
    error::Error,
    io::{Cursor, Read, Write},
    marker::PhantomData,
};
use typid::ID;

pub struct RpcRequest<Input>
where
    Input: Codec + Sized,
{
    guid: ID<()>,
    procedure: String,
    input: Input::Value,
}

impl<Input: Codec + Sized> Clone for RpcRequest<Input>
where
    Input::Value: Clone,
{
    fn clone(&self) -> Self {
        Self {
            guid: self.guid,
            procedure: self.procedure.clone(),
            input: self.input.clone(),
        }
    }
}

pub struct RpcResponse<Output>
where
    Output: Codec + Sized,
{
    guid: ID<()>,
    procedure: String,
    output: Output::Value,
}

impl<Output: Codec + Sized> Clone for RpcResponse<Output>
where
    Output::Value: Clone,
{
    fn clone(&self) -> Self {
        Self {
            guid: self.guid,
            procedure: self.procedure.clone(),
            output: self.output.clone(),
        }
    }
}

pub enum RpcPartialDecoder {
    Request {
        guid: ID<()>,
        procedure: String,
        reader: Cursor<Vec<u8>>,
    },
    Response {
        guid: ID<()>,
        procedure: String,
        reader: Cursor<Vec<u8>>,
    },
}

impl RpcPartialDecoder {
    pub fn new(buffer: Vec<u8>) -> Result<Self, Box<dyn Error>> {
        let mut reader = Cursor::new(buffer);
        let mut kind_buf = [0u8; 1];
        reader.read_exact(&mut kind_buf)?;
        match kind_buf[0] {
            0 => {
                let mut guid_buf = [0u8; 16];
                reader.read_exact(&mut guid_buf)?;
                let guid = ID::from_bytes(guid_buf);
                let mut procedure_len_buf = [0u8; 2];
                reader.read_exact(&mut procedure_len_buf)?;
                let procedure_len = u16::from_le_bytes(procedure_len_buf) as usize;
                let mut procedure_buf = vec![0u8; procedure_len];
                reader.read_exact(&mut procedure_buf)?;
                let procedure = String::from_utf8(procedure_buf)?;
                Ok(Self::Request {
                    guid,
                    procedure,
                    reader,
                })
            }
            1 => {
                let mut guid_buf = [0u8; 16];
                reader.read_exact(&mut guid_buf)?;
                let guid = ID::from_bytes(guid_buf);
                let mut procedure_len_buf = [0u8; 2];
                reader.read_exact(&mut procedure_len_buf)?;
                let procedure_len = u16::from_le_bytes(procedure_len_buf) as usize;
                let mut procedure_buf = vec![0u8; procedure_len];
                reader.read_exact(&mut procedure_buf)?;
                let procedure = String::from_utf8(procedure_buf)?;
                Ok(Self::Response {
                    guid,
                    procedure,
                    reader,
                })
            }
            _ => Err("Invalid RPC kind".into()),
        }
    }

    #[allow(clippy::type_complexity)]
    pub fn complete<Output: Codec + Sized, Input: Codec + Sized>(
        mut self,
    ) -> Result<Rpc<Output, Input>, Box<dyn Error>> {
        match &mut self {
            Self::Request {
                guid,
                procedure,
                reader,
            } => {
                let input = Input::decode(reader)?;
                Ok(Rpc::Request(RpcRequest {
                    guid: *guid,
                    procedure: procedure.clone(),
                    input,
                }))
            }
            Self::Response {
                guid,
                procedure,
                reader,
            } => {
                let output = Output::decode(reader)?;
                Ok(Rpc::Response(RpcResponse {
                    guid: *guid,
                    procedure: procedure.clone(),
                    output,
                }))
            }
        }
    }

    pub fn guid(&self) -> ID<()> {
        match self {
            Self::Request { guid, .. } => *guid,
            Self::Response { guid, .. } => *guid,
        }
    }

    pub fn procedure(&self) -> &str {
        match self {
            Self::Request { procedure, .. } => procedure,
            Self::Response { procedure, .. } => procedure,
        }
    }
}

pub enum Rpc<Output, Input>
where
    Output: Codec + Sized,
    Input: Codec + Sized,
{
    Request(RpcRequest<Input>),
    Response(RpcResponse<Output>),
}

impl<Output: Codec + Sized, Input: Codec + Sized> Rpc<Output, Input> {
    pub fn new(procedure: impl ToString, input: Input::Value) -> Self {
        Self::Request(RpcRequest {
            guid: ID::default(),
            procedure: procedure.to_string(),
            input,
        })
    }

    pub fn encoded_request(
        guid: &ID<()>,
        procedure: &str,
        input: &Input::Value,
        buffer: &mut dyn Write,
    ) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&[0u8])?;
        buffer.write_all(guid.uuid().as_bytes())?;
        let procedure_bytes = procedure.as_bytes();
        let procedure_len = procedure_bytes.len() as u16;
        buffer.write_all(&procedure_len.to_le_bytes())?;
        buffer.write_all(procedure_bytes)?;
        Input::encode(input, buffer)?;
        Ok(())
    }

    pub fn encoded_response(
        guid: &ID<()>,
        procedure: &str,
        output: &Output::Value,
        buffer: &mut dyn Write,
    ) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&[1u8])?;
        buffer.write_all(guid.uuid().as_bytes())?;
        let procedure_bytes = procedure.as_bytes();
        let procedure_len = procedure_bytes.len() as u16;
        buffer.write_all(&procedure_len.to_le_bytes())?;
        buffer.write_all(procedure_bytes)?;
        Output::encode(output, buffer)?;
        Ok(())
    }

    #[allow(clippy::type_complexity)]
    pub fn decoded_request(
        buffer: &mut dyn Read,
    ) -> Result<(ID<()>, String, Input::Value), Box<dyn Error>> {
        let mut guid_buf = [0u8; 16];
        buffer.read_exact(&mut guid_buf)?;
        let guid = ID::from_bytes(guid_buf);
        let mut procedure_len_buf = [0u8; 2];
        buffer.read_exact(&mut procedure_len_buf)?;
        let procedure_len = u16::from_le_bytes(procedure_len_buf) as usize;
        let mut procedure_buf = vec![0u8; procedure_len];
        buffer.read_exact(&mut procedure_buf)?;
        let procedure = String::from_utf8(procedure_buf)?;
        let input = Input::decode(buffer)?;
        Ok((guid, procedure, input))
    }

    #[allow(clippy::type_complexity)]
    pub fn decoded_response(
        buffer: &mut dyn Read,
    ) -> Result<(ID<()>, String, Output::Value), Box<dyn Error>> {
        let mut guid_buf = [0u8; 16];
        buffer.read_exact(&mut guid_buf)?;
        let guid = ID::from_bytes(guid_buf);
        let mut procedure_len_buf = [0u8; 2];
        buffer.read_exact(&mut procedure_len_buf)?;
        let procedure_len = u16::from_le_bytes(procedure_len_buf) as usize;
        let mut procedure_buf = vec![0u8; procedure_len];
        buffer.read_exact(&mut procedure_buf)?;
        let procedure = String::from_utf8(procedure_buf)?;
        let output = Output::decode(buffer)?;
        Ok((guid, procedure, output))
    }

    pub fn is_request(&self) -> bool {
        matches!(self, Self::Request(_))
    }

    pub fn is_response(&self) -> bool {
        matches!(self, Self::Response(_))
    }

    pub fn guid(&self) -> ID<()> {
        match self {
            Self::Request(RpcRequest { guid, .. }) => *guid,
            Self::Response(RpcResponse { guid, .. }) => *guid,
        }
    }

    pub fn procedure(&self) -> &str {
        match self {
            Self::Request(RpcRequest { procedure, .. }) => procedure,
            Self::Response(RpcResponse { procedure, .. }) => procedure,
        }
    }

    pub fn encode(self, buffer: &mut dyn Write) -> Result<(), Box<dyn Error>> {
        <Self as Codec>::encode(&self, buffer)
    }

    pub fn decode(buffer: &mut dyn Read) -> Result<Self, Box<dyn Error>> {
        <Self as Codec>::decode(buffer)
    }

    #[allow(clippy::type_complexity)]
    pub fn call(self) -> Result<(RpcCall<Output, Input>, Input::Value), Box<dyn Error>> {
        match self {
            Self::Request(RpcRequest {
                guid,
                procedure,
                input,
            }) => Ok((
                RpcCall {
                    guid,
                    procedure,
                    _marker: PhantomData,
                },
                input,
            )),
            Self::Response(_) => Err("Cannot create RpcCall from Response".into()),
        }
    }

    pub fn result(self) -> Result<Output::Value, Box<dyn Error>> {
        match self {
            Self::Response(RpcResponse { output, .. }) => Ok(output),
            Self::Request(_) => Err("Cannot get result from Request".into()),
        }
    }
}

impl<Output: Codec + Sized, Input: Codec + Sized> Clone for Rpc<Output, Input>
where
    Input::Value: Clone,
    Output::Value: Clone,
{
    fn clone(&self) -> Self {
        match self {
            Self::Request(v) => Self::Request(v.clone()),
            Self::Response(v) => Self::Response(v.clone()),
        }
    }
}

impl<Output: Codec + Sized, Input: Codec + Sized> Codec for Rpc<Output, Input> {
    type Value = Self;

    fn encode(message: &Self::Value, buffer: &mut dyn Write) -> Result<(), Box<dyn Error>> {
        match message {
            Self::Request(RpcRequest {
                guid,
                procedure,
                input,
            }) => {
                Self::encoded_request(guid, procedure, input, buffer)?;
                Ok(())
            }
            Self::Response(RpcResponse {
                guid,
                procedure,
                output,
            }) => {
                Self::encoded_response(guid, procedure, output, buffer)?;
                Ok(())
            }
        }
    }

    fn decode(buffer: &mut dyn Read) -> Result<Self::Value, Box<dyn Error>> {
        let mut kind_buf = [0u8; 1];
        buffer.read_exact(&mut kind_buf)?;
        match kind_buf[0] {
            0 => {
                let (guid, procedure, input) = Self::decoded_request(buffer)?;
                Ok(Self::Request(RpcRequest {
                    guid,
                    procedure,
                    input,
                }))
            }
            1 => {
                let (guid, procedure, output) = Self::decoded_response(buffer)?;
                Ok(Self::Response(RpcResponse {
                    guid,
                    procedure,
                    output,
                }))
            }
            _ => Err("Invalid RPC kind".into()),
        }
    }
}

pub struct RpcCall<Output, Input>
where
    Output: Codec + Sized,
    Input: Codec + Sized,
{
    guid: ID<()>,
    procedure: String,
    _marker: PhantomData<fn() -> (Input, Output)>,
}

impl<Output: Codec + Sized, Input: Codec + Sized> RpcCall<Output, Input> {
    pub fn guid(&self) -> ID<()> {
        self.guid
    }

    pub fn procedure(&self) -> &str {
        &self.procedure
    }

    pub fn respond(self, output: Output::Value) -> Rpc<Output, Input> {
        Rpc::Response(RpcResponse {
            guid: self.guid,
            procedure: self.procedure,
            output,
        })
    }
}

impl<Output: Codec + Sized, Input: Codec + Sized> Clone for RpcCall<Output, Input> {
    fn clone(&self) -> Self {
        Self {
            guid: self.guid,
            procedure: self.procedure.clone(),
            _marker: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type RpcGreet = Rpc<bool, String>;

    fn greet(name: &str) -> bool {
        name == "Alice"
    }

    #[test]
    fn test_rpc() {
        ///// Machine A create and send RPC.
        let rpc = RpcGreet::new("greet", "Alice".to_string());
        let guid = rpc.guid();
        assert!(rpc.is_request());

        let mut buffer: Vec<u8> = Vec::new();
        rpc.encode(&mut buffer).unwrap();

        ///// Machine B receive RPC, execute it and send response.
        let rpc = RpcGreet::decode(&mut buffer.as_slice()).unwrap();
        assert_eq!(rpc.guid(), guid);
        assert!(rpc.is_request());

        let (call, input) = rpc.call().unwrap();
        assert_eq!(call.guid(), guid);
        assert_eq!(call.procedure(), "greet");
        assert_eq!(input.as_str(), "Alice");

        let rpc = call.respond(greet(&input));
        assert_eq!(rpc.guid(), guid);
        assert!(rpc.is_response());

        let mut buffer: Vec<u8> = Vec::new();
        rpc.encode(&mut buffer).unwrap();

        ///// Machine A receive RPC response.
        let rpc = RpcGreet::decode(&mut buffer.as_slice()).unwrap();
        assert_eq!(rpc.guid(), guid);
        assert_eq!(rpc.procedure(), "greet");
        assert!(rpc.is_response());
        let output = rpc.result().unwrap();
        assert!(output);
    }
}
