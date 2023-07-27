use crate::{
    clients::{Client, RawRequest},
    protocol_generated::WaylandId,
    runner::Dispatcher,
};

pub trait Object {
    fn id(&self) -> WaylandId;
    fn on(&mut self, client: &mut Client, request: RawRequest, dispatcher: &mut Dispatcher);
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
