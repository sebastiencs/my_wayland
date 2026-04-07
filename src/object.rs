use std::any::Any;

use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    protocol_generated::{Constants, WaylandId},
    state::{Grab, GrabbedObject, ObjectId, State},
};

/// Internal object-safe extension methods for `dyn Object`.
/// Needed because `Object::on(self: GrabbedObject<Self>, ..)` is not object-safe,
/// so the runtime needs a separate fixed-receiver bridge for dispatch and downcasting.
#[doc(hidden)]
pub trait ObjectDynExt: 'static {
    fn call_on(
        self: Box<Self>,
        grab: Grab<ObjectId, Box<dyn Object>>,
        client: &mut Client,
        state: &mut State,
        request: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult;

    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
}

/// Wayland object implementation. Kept object-safe so we can store `Box<dyn Object>`; protocol
/// id pairing lives on [`ObjectWithId`] + [`IdToObject`].
pub trait Object: ObjectDynExt {
    fn id(&self) -> WaylandId;
    fn on(
        self: GrabbedObject<Self>,
        client: &mut Client,
        state: &mut State,
        request: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult
    where
        Self: Sized;

    /// Silent compositor-side cleanup before tearing down all objects owned by a disconnected
    /// client. Unlike normal protocol destruction, this should not try to talk to the client.
    fn on_client_disconnect(
        &mut self,
        _client: &mut Client,
        _state: &mut State,
        _dispatcher: &mut ClientDispatcher,
    ) {
    }
}

impl<T> ObjectDynExt for T
where
    T: Object,
{
    fn call_on(
        self: Box<Self>,
        grab: Grab<ObjectId, Box<dyn Object>>,
        client: &mut Client,
        state: &mut State,
        request: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        let grabbed = GrabbedObject::<Self>::new(self, grab);
        <Self as Object>::on(grabbed, client, state, request, dispatcher)
    }

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

impl dyn Object {
    pub fn downcast_ref<T: Object>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }

    pub fn downcast_mut<T: Object>(&mut self) -> Option<&mut T> {
        self.as_any_mut().downcast_mut::<T>()
    }

    pub fn downcast<T: Object>(self: Box<Self>) -> Result<Box<T>, Box<Self>> {
        if self.as_any().is::<T>() {
            Ok(self.into_any().downcast::<T>().unwrap())
        } else {
            Err(self)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbortRequest {
    InvalidObject,
}

pub type RequestResult = Result<(), AbortRequest>;

pub trait PostAction: 'static {
    fn execute(
        self: Box<Self>,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    );
}

pub trait Parser: Constants {
    type Request: 'static;

    /// Decode the wire request only. Client-scoped object resolution happens in
    /// request handlers after parsing, once `Client` and `State` are available.
    fn parse_request(req: RawRequest) -> Result<Self::Request, ParserError>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParserError {
    Malformed,
    InvalidRequest,
    MissingFileDescriptor,
    /// String argument was not valid UTF-8
    InvalidString,
}

/// Compositor object type paired with a protocol id (see [`IdToObject`]).
pub trait ObjectWithId: Object {
    type Id: IdToObject<Object = Self>;
}

/// Maps a protocol id newtype to its compositor object type. The id type is always `Self`.
pub trait IdToObject: Copy + Into<WaylandId> + From<WaylandId> + 'static {
    type Object: ObjectWithId<Id = Self>;
}
