use std::any::Any;

use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    protocol_generated::{Constants, WaylandId},
    state::State,
};

pub trait Object: 'static {
    fn id(&self) -> WaylandId;
    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        request: RawRequest,
        dispatcher: &mut ClientDispatcher,
    );
    fn as_any(&mut self) -> &mut dyn Any;
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
    B: IdToObject
{
    type Object = (A::Object, B::Object);
}
