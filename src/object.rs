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
    const INTERFACE_NAME: &'static str;

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
}
