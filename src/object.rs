use std::any::Any;

use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    protocol_generated::{Constants, WaylandId},
    state::State,
};

pub trait Object
where
    Self: AsAny + 'static,
{
    fn id(&self) -> WaylandId;
    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        request: RawRequest,
        dispatcher: &mut ClientDispatcher,
    );
}

impl dyn Object {
    pub fn downcast_ref<T: Object>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }

    pub fn downcast_mut<T: Object>(&mut self) -> Option<&mut T> {
        self.as_any_mut().downcast_mut::<T>()
    }

    pub fn downcast<T: Object>(self: Box<Self>) -> Result<Box<T>, Box<Self>> {
        if (&*self).as_any().is::<T>() {
            Ok(self.into_any().downcast::<T>().ok().unwrap())
        } else {
            Err(self)
        }
    }
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
