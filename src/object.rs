use std::any::Any;

use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    protocol_generated::{Constants, WaylandId},
    state::State,
};

pub trait Object: AsAny + 'static {
    fn id(&self) -> WaylandId;
    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        request: RawRequest,
        dispatcher: &mut ClientDispatcher,
    );
}

/// Convert `dyn Object` into concrete types
/// We need to implement those methods on concrete types, to get
/// the vtable of the concrete type.
///
/// See https://stackoverflow.com/a/33687996
pub trait AsAny: 'static {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
}

impl<T: 'static> AsAny for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

pub trait Parser: Constants {
    type Request: 'static;
    // const INTERFACE_NAME: &'static str;

    /// Helper that just log the request
    fn parse(req: RawRequest) -> Result<Self::Request, ParserError>
    where
        Self::Request: std::fmt::Debug,
    {
        let object_id = req.object_id;
        Self::parse_request(req)
            .inspect(|req| {
                log::trace!(
                    target: "in",
                    "[{}] {}::{:?}",
                    object_id,
                    Self::INTERFACE_NAME,
                    req
                )
            })
            .inspect_err(|req| {
                log::error!(
                    target: "in",
                    "[{}] {}: {:?}",
                    object_id,
                    Self::INTERFACE_NAME,
                    req
                )
            })
    }
    fn parse_request(req: RawRequest) -> Result<Self::Request, ParserError>;
}

#[derive(Debug)]
pub enum ParserError {
    Malformed,
    InvalidRequest,
    MissingFileDescriptor,
}

pub trait IdToObject {
    type Object: 'static;
}

impl<A, B> IdToObject for (A, B)
where
    A: IdToObject,
    B: IdToObject,
{
    type Object = (A::Object, B::Object);
}
