use crate::codec::Codec;
use std::{
    error::Error,
    io::{Read, Write},
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
    output: Output::Value,
}

impl<Output: Codec + Sized> Clone for RpcResponse<Output>
where
    Output::Value: Clone,
{
    fn clone(&self) -> Self {
        Self {
            guid: self.guid,
            output: self.output.clone(),
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

    pub fn procedure(&self) -> Option<&str> {
        match self {
            Self::Request(RpcRequest { procedure, .. }) => Some(procedure),
            Self::Response(_) => None,
        }
    }

    pub fn encode(self, buffer: &mut dyn Write) -> Result<(), Box<dyn Error>> {
        match self {
            Self::Request(RpcRequest {
                guid,
                procedure,
                input,
            }) => {
                buffer.write_all(&[0u8])?;
                buffer.write_all(guid.uuid().as_bytes())?;
                let method_bytes = procedure.as_bytes();
                let method_len = method_bytes.len() as u16;
                buffer.write_all(&method_len.to_be_bytes())?;
                buffer.write_all(method_bytes)?;
                Input::encode(&input, buffer)?;
                Ok(())
            }
            Self::Response(RpcResponse { guid, output }) => {
                buffer.write_all(&[1u8])?;
                buffer.write_all(guid.uuid().as_bytes())?;
                Output::encode(&output, buffer)?;
                Ok(())
            }
        }
    }

    pub fn decode(buffer: &mut dyn Read) -> Result<Self, Box<dyn Error>> {
        let mut kind_buf = [0u8; 1];
        buffer.read_exact(&mut kind_buf)?;
        match kind_buf[0] {
            0 => {
                let mut guid_buf = [0u8; 16];
                buffer.read_exact(&mut guid_buf)?;
                let guid = ID::from_bytes(guid_buf);
                let mut method_len_buf = [0u8; 2];
                buffer.read_exact(&mut method_len_buf)?;
                let method_len = u16::from_be_bytes(method_len_buf) as usize;
                let mut method_buf = vec![0u8; method_len];
                buffer.read_exact(&mut method_buf)?;
                let procedure = String::from_utf8(method_buf)?;
                let input = Input::decode(buffer)?;
                Ok(Self::Request(RpcRequest {
                    guid,
                    procedure,
                    input,
                }))
            }
            1 => {
                let mut guid_buf = [0u8; 16];
                buffer.read_exact(&mut guid_buf)?;
                let guid = ID::from_bytes(guid_buf);
                let output = Output::decode(buffer)?;
                Ok(Self::Response(RpcResponse { guid, output }))
            }
            _ => Err("Invalid RPC kind".into()),
        }
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
    use crate::codec::postcard::PostcardCodec;

    type RpcGreet = Rpc<PostcardCodec<bool>, PostcardCodec<String>>;

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
        assert!(rpc.is_response());
        let output = rpc.result().unwrap();
        assert!(output);
    }
}
