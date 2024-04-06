use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    protocol_generated::WaylandId,
    state::State,
    utils::strip_type_name,
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

    /// Helper that just log the request
    fn parse(req: RawRequest) -> Result<Self::Request, ParserError>
    where
        Self::Request: std::fmt::Debug,
    {
        let type_name = std::any::type_name::<Self>();
        Self::parse_request(req)
            .inspect(|req| {
                log::trace!(
                    target: "in",
                    "{}::{:?}",
                    strip_type_name(type_name),
                    req
                )
            })
            .inspect_err(|req| {
                log::error!(
                    target: "in",
                    "{}: {:?}",
                    strip_type_name(type_name),
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
