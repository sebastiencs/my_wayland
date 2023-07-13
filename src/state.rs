use std::{
    any::{Any, TypeId},
    collections::{BTreeMap, VecDeque},
    os::fd::RawFd,
};

use crate::{clients::RawEvent, io_uring::model::IoUringAction};

type Result<T> = std::result::Result<T, ()>;

pub struct Dispatcher {
    queue: VecDeque<AnyAction>,
    pending_models: VecDeque<(ActionId, AnyModel)>,
    to_remove: VecDeque<ActionId>,

    client_fd: Option<RawFd>,
}

impl Default for Dispatcher {
    fn default() -> Self {
        Self {
            queue: VecDeque::with_capacity(1024),
            pending_models: VecDeque::with_capacity(1024),
            to_remove: VecDeque::with_capacity(1024),
            client_fd: None,
        }
    }
}

impl Dispatcher {
    pub fn dispatch<A>(&mut self, action: A)
    where
        A: Sized + 'static,
    {
        assert_ne!(TypeId::of::<A>(), TypeId::of::<AnyAction>());
        self.queue.push_back(AnyAction::from(action));
    }

    pub fn dispatch_to<A, I>(&mut self, id: I, action: A)
    where
        I: Into<u64>,
        A: Sized + 'static,
    {
        assert_ne!(TypeId::of::<A>(), TypeId::of::<AnyAction>());
        self.queue.push_back(AnyAction::from_id(id, action));
    }

    pub fn redispatch_to<R>(&mut self, redispatcher: Redispatcher<R>, action: R) {
        if let Some(action) = redispatcher.make(action) {
            self.queue.push_back(action);
        };
    }

    pub fn add_model<I, M>(&mut self, id: I, model: M)
    where
        I: Into<u64>,
        M: Model,
    {
        let action_id = ActionId::of_with::<M::Action, _>(id);
        self.pending_models
            .push_back((action_id, Box::new(model).into_vtable()));
    }

    pub fn start_model<I, M>(&mut self, id: I, model: M)
    where
        I: Into<u64> + Copy,
        M: Model,
    {
        self.add_model(id, model);
        self.dispatch_to(id, M::START.unwrap());
    }

    pub fn stop_model<I, M>(&mut self, id: I)
    where
        I: Into<u64> + Copy,
        M: Model,
    {
        let action_id = ActionId::of_with::<M::Action, _>(id);
        self.to_remove.push_back(action_id);
    }

    pub fn send_raw(&mut self, event: RawEvent) {
        let action = IoUringAction::Write {
            fd: self.client_fd.unwrap(),
            offset: 0,
            buffer: event.into_bytes(),
            dispatch_to: Redispatcher::None,
        };
        self.dispatch(action);
    }

    pub fn set_client_fd(&mut self, fd: RawFd) {
        self.client_fd = Some(fd);
    }

    fn clear(&mut self) {
        self.clear_client_fd();
    }

    fn clear_client_fd(&mut self) {
        if self.client_fd.is_some() {
            self.client_fd = None;
        }
    }
}

pub enum Redispatcher<R> {
    Fun {
        ptr: fn(R) -> AnyAction,
    },
    WithId {
        ptr: fn(R) -> AnyAction,
        id: u64,
        /// For printing/debug purpose only
        /// So we know to which model we redispatch
        name: &'static str,
    },
    Into {
        ptr: fn(R) -> AnyAction,
        name: &'static str,
    },
    None,
}

impl<R> Clone for Redispatcher<R> {
    fn clone(&self) -> Self {
        match self {
            Self::Fun { ptr } => Self::Fun { ptr: *ptr },
            Self::WithId { ptr, id, name } => Self::WithId { ptr: *ptr, id: *id, name },
            Self::Into { ptr, name } => Self::Into { ptr: *ptr, name },
            Self::None => Self::None,
        }
    }
}

impl<R: std::fmt::Debug> std::fmt::Debug for Redispatcher<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn type_name(s: &str) -> &str {
            let index = s.rfind("::").map(|i| i + 2).unwrap_or(0);
            s.get(index..).unwrap()
        }

        match self {
            Self::Fun { ptr: _ } => f.write_str("ptr"),
            Self::WithId { id, ptr: _, name } => {
                f.write_fmt(format_args!("{}({:?})", type_name(name), id))
            }
            Self::Into { ptr: _, name } => f.write_fmt(format_args!("{}", type_name(name))),
            Self::None => f.write_str("None"),
        }
    }
}

impl<R> Redispatcher<R> {
    pub fn new(ptr: fn(R) -> AnyAction) -> Self {
        Self::Fun { ptr }
    }

    pub fn into<T>() -> Self
    where
        T: From<R> + 'static,
    {
        fn generated<R, T: From<R> + 'static>(result: R) -> AnyAction {
            AnyAction::from(T::from(result))
        }
        let ptr = generated::<R, T>;
        let name = std::any::type_name::<T>();
        Self::Into { ptr, name }
    }

    pub fn id<V>(id: V, ptr: fn(R) -> AnyAction) -> Self
    where
        V: Into<u64>,
    {
        let id = id.into();
        let name = std::any::type_name::<V>();
        Self::WithId { id, ptr, name }
    }
}

impl<R> Redispatcher<R> {
    pub fn make(self, value: R) -> Option<AnyAction> {
        match self {
            Self::Fun { ptr } => Some(ptr(value)),
            Self::Into { ptr, name: _ } => Some(ptr(value)),
            Self::WithId { id, ptr, name: _ } => {
                let mut action = ptr(value);
                action.id.specifier = Some(id);
                Some(action)
            }
            Self::None => None,
        }
    }
}

// pub type Redispatcher<R> = fn(R) -> AnyAction;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ActionId {
    type_id: TypeId,
    specifier: Option<u64>,
}

impl ActionId {
    pub fn of<T>() -> Self
    where
        T: 'static,
    {
        Self { type_id: TypeId::of::<T>(), specifier: None }
    }

    pub fn of_with<T, S>(spec: S) -> Self
    where
        T: 'static,
        S: Into<u64>,
    {
        Self { type_id: TypeId::of::<T>(), specifier: Some(spec.into()) }
    }
}

pub struct Runner {
    models: BTreeMap<ActionId, AnyModel>,
}

impl Default for Runner {
    fn default() -> Self {
        Self { models: BTreeMap::default() }
    }
}

impl Runner {
    pub fn add_model<M>(&mut self, model: Box<M>)
    where
        M: Model,
    {
        self.models
            .insert(ActionId::of::<M::Action>(), model.into_vtable());
    }

    pub fn run(&mut self, mut dispatcher: Dispatcher) {
        loop {
            while let Some(action) = dispatcher.queue.pop_front() {
                self.process_action(action, &mut dispatcher)
            }
            dispatcher.dispatch(IoUringAction::WaitForCompletion);
        }
    }

    fn process_action(&mut self, action: AnyAction, dispatcher: &mut Dispatcher) {
        assert_ne!(action.id, ActionId::of::<AnyAction>());

        let Some(model) = self.models.get_mut(action.action_id()) else {
            panic!("action not found1 {}", action.type_name);
        };
        (model.vtable.try_process)(&mut model.state, action, dispatcher).unwrap();
        self.post_process(dispatcher);
    }

    fn post_process(&mut self, dispatcher: &mut Dispatcher) {
        // Add new models
        while let Some((action_id, model)) = dispatcher.pending_models.pop_front() {
            self.models.insert(action_id, model);
        }
        self.maybe_stop_models(dispatcher);
    }

    fn maybe_stop_models(&mut self, dispatcher: &mut Dispatcher) {
        // Stop models
        // We use an index here because `try_stop` might add new ids to `dispatcher.to_remove`
        let mut index = 0;
        while let Some(id) = dispatcher.to_remove.get(index) {
            let model = self.models.get_mut(id).unwrap();
            (model.vtable.try_stop)(&mut model.state, dispatcher).unwrap();
            index += 1;
        }
        // Remove them in reverse order
        while let Some(id) = dispatcher.to_remove.pop_back() {
            self.models.remove(&id).unwrap();
        }
    }
}

pub struct AnyAction {
    id: ActionId,
    ptr: Box<dyn Any>,
    /// For printing/debug purpose only
    type_name: &'static str,
}

impl AnyAction {
    fn action_id(&self) -> &ActionId {
        &self.id
    }

    pub fn empty() -> Self {
        Self {
            id: ActionId::of::<()>(),
            ptr: Box::new(()),
            type_name: std::any::type_name::<()>(),
        }
    }

    pub fn from<T: 'static>(v: T) -> Self {
        // Panic when calling `AnyAction::from(AnyAction { .. })`
        assert_ne!(TypeId::of::<T>(), TypeId::of::<AnyAction>());
        Self {
            id: ActionId::of::<T>(),
            ptr: Box::new(v),
            type_name: std::any::type_name::<T>(),
        }
    }

    pub fn from_id<T, S>(id: S, v: T) -> Self
    where
        T: 'static,
        S: Into<u64>,
    {
        // Panic when calling `AnyAction::from(AnyAction { .. })`
        assert_ne!(TypeId::of::<T>(), TypeId::of::<AnyAction>());
        Self {
            id: ActionId::of_with::<T, S>(id),
            ptr: Box::new(v),
            type_name: std::any::type_name::<T>(),
        }
    }
}

pub struct AnyModel {
    state: Box<dyn Any>,
    vtable: ModelVTable,
}

struct ModelVTable {
    try_process:
        fn(state: &mut Box<dyn Any>, action: AnyAction, dispatcher: &mut Dispatcher) -> Result<()>,
    try_stop: fn(state: &mut Box<dyn Any>, dispatcher: &mut Dispatcher) -> Result<()>,
}

pub trait PrivateModel
where
    Self: Sized + 'static,
{
    fn into_vtable(self: Box<Self>) -> AnyModel;
    fn try_process(
        state: &mut Box<dyn Any>,
        action: AnyAction,
        dispatcher: &mut Dispatcher,
    ) -> Result<()>;
    fn try_stop(state: &mut Box<dyn Any>, dispatcher: &mut Dispatcher) -> Result<()>;
}

impl<T> PrivateModel for T
where
    T: Model,
{
    fn into_vtable(self: Box<Self>) -> AnyModel {
        let state = self;
        let vtable = ModelVTable { try_process: Self::try_process, try_stop: Self::try_stop };
        AnyModel { state, vtable }
    }

    fn try_process(
        state: &mut Box<dyn Any>,
        action: AnyAction,
        dispatcher: &mut Dispatcher,
    ) -> Result<()> {
        let Some(state) = state.downcast_mut::<Self>() else {
            eprintln!("state not found");
            return Ok(());
        };
        let Ok(action) = action.ptr.downcast::<T::Action>() else {
            eprintln!("action not found2");
            return Ok(());
        };
        state.process(*action, dispatcher)?;
        dispatcher.clear();
        Ok(())
    }

    fn try_stop(state: &mut Box<dyn Any>, dispatcher: &mut Dispatcher) -> Result<()> {
        let Some(state) = state.downcast_mut::<Self>() else {
            panic!("state not found");
        };
        let Some(action) = Self::STOP else {
            panic!("Model::STOP is not defined for {}", std::any::type_name::<T>());
        };
        state.process(action, dispatcher)?;
        dispatcher.clear();
        Ok(())
    }
}

pub trait Model
where
    Self: Sized + 'static,
{
    type InitParams: 'static;
    type Action: Sized + 'static;
    const START: Option<Self::Action> = None;
    const STOP: Option<Self::Action> = None;

    fn init(params: Self::InitParams) -> Box<Self>;
    fn process(&mut self, action: Self::Action, dispatcher: &mut Dispatcher) -> Result<()>;
}

mod backend {
    use super::*;

    enum BackendAction {
        NewDrmDevice,
        NewConnector,
        NewInputDevice,
        DevicesEnumerated,
    }

    struct Backend;
    struct BackendState;

    impl Model for BackendState {
        // type State = BackendState;
        type InitParams = ();
        type Action = BackendAction;

        fn init(_params: Self::InitParams) -> Box<Self> {
            todo!()
        }

        fn process(&mut self, action: Self::Action, _dispatcher: &mut Dispatcher) -> Result<()> {
            match action {
                Self::Action::NewDrmDevice => todo!(),
                Self::Action::NewConnector => todo!(),
                Self::Action::NewInputDevice => todo!(),
                Self::Action::DevicesEnumerated => todo!(),
            }
        }
    }
}

mod frontend {
    use super::*;

    enum FrontendAction {
        Push,
        Click,
    }

    // struct Frontend;
    struct FrontendState;

    impl Model for FrontendState {
        // type State = FrontendState;
        type InitParams = ();
        type Action = FrontendAction;

        fn init(_params: Self::InitParams) -> Box<Self> {
            todo!()
        }

        fn process(&mut self, action: Self::Action, _dispatcher: &mut Dispatcher) -> Result<()> {
            match action {
                Self::Action::Push => todo!(),
                Self::Action::Click => todo!(),
            }
        }
    }
}
