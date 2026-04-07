use std::any::Any;

use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    protocol_generated::{Constants, WaylandId},
    state::{Grab, GrabbedObject, ObjectId, State},
};

// We have to use such wrapper because rust doesn't support self arbitrary types
// with dynamic dispatch. So we make our own virtual table.
// We want `Object::on` self param to be a `GrabbedAny` so that it can be manually
// dropped during `Object::on` execution, to put the object back into `State`.
#[derive(Debug)]
pub struct ObjectWrapper {
    object: Box<dyn Any>,
    vtable: ObjectVTable,
}

impl ObjectWrapper {
    pub fn downcast_ref<T: Object>(&self) -> Option<&T> {
        (&*self.object).downcast_ref::<T>()
    }
    pub fn downcast_mut<T: Object>(&mut self) -> Option<&mut T> {
        (&mut *self.object).downcast_mut::<T>()
    }
    pub fn into_object(self) -> Box<dyn Object> {
        // self.object.downcast::<T>().unwrap()
        (self.vtable.into_object)(self.object)
    }
    pub fn into_raw(self) -> (Box<dyn Any>, ObjectVTable) {
        let Self { object, vtable } = self;
        (object, vtable)
    }
    pub fn from_raw((object, vtable): (Box<dyn Any>, ObjectVTable)) -> Self {
        Self { object, vtable }
    }
    pub fn on(
        self: Grab<ObjectId, ObjectWrapper>,
        client: &mut Client,
        state: &mut State,
        request: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        (self.vtable.try_on)(self, client, state, request, dispatcher)
    }
}
impl<T> From<T> for ObjectWrapper
where
    T: Object + PrivateObject,
{
    fn from(value: T) -> Self {
        Self {
            object: Box::new(value),
            vtable: ObjectVTable {
                try_on: T::try_on,
                as_object: T::as_object,
                as_object_mut: T::as_object_mut,
                into_object: T::into_object,
            },
        }
    }
}
impl core::ops::Deref for ObjectWrapper {
    type Target = dyn Object;
    fn deref(&self) -> &Self::Target {
        (self.vtable.as_object)(&self.object)
    }
}
impl core::ops::DerefMut for ObjectWrapper {
    fn deref_mut(&mut self) -> &mut Self::Target {
        (self.vtable.as_object_mut)(&mut self.object)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ObjectVTable {
    try_on: fn(
        this: Grab<ObjectId, ObjectWrapper>,
        client: &mut Client,
        state: &mut State,
        request: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult,
    as_object: fn(ptr: &Box<dyn Any>) -> &dyn Object,
    as_object_mut: fn(ptr: &mut Box<dyn Any>) -> &mut dyn Object,
    into_object: fn(ptr: Box<dyn Any>) -> Box<dyn Object>,
}

trait PrivateObject
where
    Self: Sized + 'static,
{
    fn try_on(
        this: Grab<ObjectId, ObjectWrapper>,
        client: &mut Client,
        state: &mut State,
        request: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult;

    fn as_object(ptr: &Box<dyn Any>) -> &dyn Object;
    fn as_object_mut(ptr: &mut Box<dyn Any>) -> &mut dyn Object;
    fn into_object(ptr: Box<dyn Any>) -> Box<dyn Object>;
}

impl<T> PrivateObject for T
where
    T: Object,
{
    fn try_on(
        this: Grab<ObjectId, ObjectWrapper>,
        client: &mut Client,
        state: &mut State,
        request: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        let grab = GrabbedObject::<Self>::new(this);
        grab.on(client, state, request, dispatcher)
    }
    fn as_object(ptr: &Box<dyn Any>) -> &dyn Object {
        (&**ptr).downcast_ref::<Self>().unwrap()
    }
    fn as_object_mut(ptr: &mut Box<dyn Any>) -> &mut dyn Object {
        (&mut **ptr).downcast_mut::<Self>().unwrap()
    }
    fn into_object(ptr: Box<dyn Any>) -> Box<dyn Object> {
        ptr.downcast::<Self>().unwrap()
    }
}

/// Wayland object implementation. Kept object-safe so we can store `Box<dyn Object>`; protocol
/// id pairing lives on [`ObjectWithId`] + [`IdToObject`].
pub trait Object
where
    Self: 'static,
{
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

// /// Wayland object implementation. Kept object-safe so we can store `Box<dyn Object>`; protocol
// /// id pairing lives on [`ObjectWithId`] + [`IdToObject`].
// pub trait Object
// where
//     Self: AsAny + 'static,
// {
//     fn id(&self) -> WaylandId;
//     fn on(
//         &mut self,
//         client: &mut Client,
//         state: &mut State,
//         request: RawRequest,
//         dispatcher: &mut ClientDispatcher,
//     ) -> RequestResult;

//     /// Silent compositor-side cleanup before tearing down all objects owned by a disconnected
//     /// client. Unlike normal protocol destruction, this should not try to talk to the client.
//     fn on_client_disconnect(
//         &mut self,
//         _client: &mut Client,
//         _state: &mut State,
//         _dispatcher: &mut ClientDispatcher,
//     ) {
//     }
// }

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

// impl dyn Object {
//     pub fn downcast_ref<T: Object>(&self) -> Option<&T> {
//         self.as_any().downcast_ref::<T>()
//     }

//     pub fn downcast_mut<T: Object>(&mut self) -> Option<&mut T> {
//         self.as_any_mut().downcast_mut::<T>()
//     }

//     pub fn downcast<T: Object>(self: Box<Self>) -> Result<Box<T>, Box<Self>> {
//         if (&*self).as_any().is::<T>() {
//             Ok(self
//                 .into_any()
//                 .downcast::<T>()
//                 .expect("downcast matches TypeId::is check"))
//         } else {
//             Err(self)
//         }
//     }
// }

// /// Convert `dyn Object` into concrete types
// /// We need to implement those methods on concrete types, to get
// /// the vtable of the concrete type.
// ///
// /// See https://stackoverflow.com/a/33687996
// pub trait AsAny: 'static {
//     fn as_any(&self) -> &dyn Any;
//     fn as_any_mut(&mut self) -> &mut dyn Any;
//     fn into_any(self: Box<Self>) -> Box<dyn Any>;
// }

// impl<T: 'static> AsAny for T {
//     fn as_any(&self) -> &dyn Any {
//         self
//     }
//     fn as_any_mut(&mut self) -> &mut dyn Any {
//         self
//     }
//     fn into_any(self: Box<Self>) -> Box<dyn Any> {
//         self
//     }
// }

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
