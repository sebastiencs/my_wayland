use std::any::Any;

use crate::clients::{Client, RawRequest};

pub struct AnyObject {
    pub object: Box<dyn Any>,
    pub vtable: ObjectVTable,
}

pub struct ObjectVTable {
    pub try_parse_and_handle: fn(
        this: &mut Box<dyn Any>,
        client: &mut Client,
        request: RawRequest,
    ) -> Result<(), ParserError>,
}

pub trait PrivateObject {
    fn into_vtable(self: Box<Self>) -> AnyObject;
    fn try_parse_and_handle(
        this: &mut Box<dyn Any>,
        client: &mut Client,
        request: RawRequest,
    ) -> Result<(), ParserError>;
}

impl<T> PrivateObject for T
where
    T: Object + 'static,
{
    fn into_vtable(self: Box<Self>) -> AnyObject {
        let object = self;
        let vtable = ObjectVTable {
            try_parse_and_handle: Self::try_parse_and_handle,
        };
        AnyObject { object, vtable }
    }

    fn try_parse_and_handle(
        this: &mut Box<dyn Any>,
        client: &mut Client,
        request: RawRequest,
    ) -> Result<(), ParserError> {
        let Some(this) = this.downcast_mut::<Self>() else {
            panic!("Wrong object");
        };
        let parsed = T::parse_request(request)?;
        this.on(client, parsed);
        Ok(())
    }
}

pub trait Object: Parser {
    fn on(&mut self, client: &mut Client, request: Self::Request);
}

pub trait Parser {
    type Request: 'static;

    fn parse_request(req: RawRequest) -> Result<Self::Request, ParserError>;
}

#[derive(Debug)]
pub enum ParserError {
    Malformed,
    InvalidRequest,
}
