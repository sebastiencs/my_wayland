use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    protocol_generated::WaylandId,
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
