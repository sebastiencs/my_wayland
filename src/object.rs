use crate::{
    clients::{Client, RawRequest},
    protocol_generated::WaylandId,
    runner::Dispatcher,
    state::State,
};

pub trait Object: 'static {
    fn id(&self) -> WaylandId;
    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        request: RawRequest,
        dispatcher: &mut Dispatcher,
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
